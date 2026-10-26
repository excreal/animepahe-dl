use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    fs::{self, File},
    io::Write,
    path::PathBuf,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

use gtk::glib::ObjectExt;
use javascriptcore::ValueExt;
use serde_json::{json, Value};
use webkit2gtk::*;

pub struct Capture {
    pub dir: PathBuf,
    persistent: bool,
    events: RefCell<File>,
    sequence: Cell<u64>,
    pub pending: Cell<usize>,
    cookie_uris: RefCell<BTreeSet<String>>,
}

impl Capture {
    pub fn new(persistent: bool) -> std::io::Result<Rc<Self>> {
        let root = if persistent {
            PathBuf::from("captures")
        } else {
            std::env::temp_dir().join("animepahe-dl-captures")
        };
        let dir = root.join(format!("{}-{}", now(), std::process::id()));
        fs::create_dir_all(dir.join("resources"))?;
        set_private_dir(&root)?;
        set_private_dir(&dir)?;
        let dir = fs::canonicalize(dir)?;
        Ok(Rc::new(Self {
            events: RefCell::new(File::create(dir.join("events.jsonl"))?),
            dir,
            persistent,
            sequence: Cell::new(0),
            pending: Cell::new(0),
            cookie_uris: RefCell::new(BTreeSet::new()),
        }))
    }

    pub fn persistent(&self) -> bool {
        self.persistent
    }

    pub fn id(&self) -> u64 {
        let id = self.sequence.get() + 1;
        self.sequence.set(id);
        id
    }

    pub fn event(&self, kind: &str, data: Value) {
        if let Err(error) = writeln!(
            self.events.borrow_mut(),
            "{}",
            json!({"time_ms": now(), "event": kind, "data": data})
        ) {
            eprintln!("Could not write capture event: {error}");
        }
    }

    pub fn write(&self, name: &str, data: &[u8]) {
        if let Err(error) = fs::write(self.dir.join(name), data) {
            self.event(
                "write_error",
                json!({"file": name, "error": error.to_string()}),
            );
            eprintln!("Capture write failed for {name}: {error}");
        }
    }

    pub fn attach(self: &Rc<Self>, view: &WebView) {
        if !self.persistent {
            let capture = self.clone();
            view.connect_resource_load_started(move |_, _, request| {
                if let Some(uri) = request.uri() {
                    if uri.starts_with("http://") || uri.starts_with("https://") {
                        capture.cookie_uris.borrow_mut().insert(uri.to_string());
                    }
                }
            });
            return;
        }
        if let Some(manager) = view.user_content_manager() {
            manager.add_script(&UserScript::new(
                r#"window.__webkitCapture = [];
                for (const level of ['log','info','warn','error','debug']) {
                    const original = console[level];
                    console[level] = function(...args) {
                        window.__webkitCapture.push({time: Date.now(), level, args: args.map(value => {try {return String(value)} catch {return '[unprintable]'}})});
                        return original.apply(this, args);
                    };
                }
                addEventListener('error', event => window.__webkitCapture.push({time: Date.now(), level:'error', message:event.message, file:event.filename, line:event.lineno}));
                addEventListener('unhandledrejection', event => window.__webkitCapture.push({time: Date.now(), level:'unhandledrejection', message:String(event.reason)}));"#,
                UserContentInjectedFrames::TopFrame, UserScriptInjectionTime::Start, &[], &[],
            ));
        }
        let capture = self.clone();
        view.connect_resource_load_started(move |_, resource, request| {
            if let Some(uri) = request.uri() {
                if uri.starts_with("http://") || uri.starts_with("https://") {capture.cookie_uris.borrow_mut().insert(uri.to_string());}
            }
            let id = capture.id();
            let mut headers = vec![];
            if let Some(h) = request.http_headers() { h.foreach(|name, value| headers.push(json!([name, value]))); }
            capture.event("request", json!({"id":id, "uri":request.uri().map(|s| s.to_string()), "headers":headers, "method": request.http_method().map(|s| s.to_string())}));
            let sent_capture = capture.clone();
            // The generated binding incorrectly treats the optional redirect response
            // as non-null. Decode the signal values ourselves for initial requests.
            resource.connect_local("sent-request", false, move |values| {
                let request = values.get(1).and_then(|v| v.get::<URIRequest>().ok());
                let response = values.get(2).and_then(|v| v.get::<Option<URIResponse>>().ok()).flatten();
                let mut headers = vec![];
                if let Some(h) = request.as_ref().and_then(|r|r.http_headers()) { h.foreach(|name,value|headers.push(json!([name,value]))); }
                let mut redirect_headers = vec![];
                if let Some(h) = response.as_ref().and_then(|r|r.http_headers()) { h.foreach(|name,value|redirect_headers.push(json!([name,value]))); }
                sent_capture.event("sent_request", json!({"id":id, "uri":request.as_ref().and_then(|r|r.uri()).map(|s|s.to_string()), "method":request.as_ref().and_then(|r|r.http_method()).map(|s|s.to_string()), "headers":headers, "redirect_status":response.map(|r|r.status_code()), "redirect_headers":redirect_headers}));
                None
            });
            let failed_capture = capture.clone();
            resource.connect_failed(move |_, error| failed_capture.event("resource_failed", json!({"id":id, "error":error.to_string()})));
            let done_capture = capture.clone();
            resource.connect_finished(move |resource| {
                if let Some(response) = resource.response() {
                    let mut headers = vec![];
                    if let Some(h) = response.http_headers() { h.foreach(|name, value| headers.push(json!([name, value]))); }
                    done_capture.event("response", json!({"id":id, "uri":response.uri().map(|s| s.to_string()), "status":response.status_code(), "mime":response.mime_type().map(|s| s.to_string()), "headers":headers}));
                }
                done_capture.pending.set(done_capture.pending.get() + 1);
                let capture = done_capture.clone();
                WebResourceExt::data(resource, None::<&gtk::gio::Cancellable>, move |result| {
                    match result {
                        Ok(bytes) => {
                            let file = format!("resources/{id}.bin");
                            capture.write(&file, &bytes);
                            capture.event("body", json!({"id":id, "file":file, "bytes":bytes.len()}));
                        }
                        Err(error) => capture.event("body_error", json!({"id":id, "error":error.to_string()})),
                    }
                    capture.pending.set(capture.pending.get() - 1);
                });
            });
        });
    }

    pub fn snapshot(self: &Rc<Self>, view: &WebView, label: &str) {
        if !self.persistent {
            return;
        }
        let id = self.id();
        let label = label.to_owned();
        self.pending.set(self.pending.get() + 1);
        let capture = self.clone();
        view.evaluate_javascript(
            include_str!("capture_page.js"),
            None,
            None,
            None::<&gtk::gio::Cancellable>,
            move |result| {
                match result {
                    Ok(value) => {
                        let file = format!("page-{id}-{label}.json");
                        capture.write(&file, value.to_str().as_bytes());
                        capture.event("snapshot", json!({"file":file}));
                    }
                    Err(error) => {
                        capture.event("snapshot_error", json!({"error":error.to_string()}))
                    }
                }
                capture.pending.set(capture.pending.get() - 1);
            },
        );
    }

    pub fn finalize(self: &Rc<Self>, view: &WebView) {
        if self.persistent {
            self.snapshot(view, "final");
            self.pending.set(self.pending.get() + 1);
            let capture = self.clone();
            view.save_to_file(
                &gtk::gio::File::for_path(self.dir.join("page.mhtml")),
                SaveMode::Mhtml,
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    capture.event(
                        "archive",
                        json!({"error":result.err().map(|e|e.to_string())}),
                    );
                    capture.pending.set(capture.pending.get() - 1);
                },
            );
        }
        if let Some(manager) = view.context().and_then(|context| context.cookie_manager()) {
            let uris: Vec<_> = self.cookie_uris.borrow().iter().cloned().collect();
            for (index, uri) in uris.into_iter().enumerate() {
                self.pending.set(self.pending.get() + 1);
                let capture = self.clone();
                let callback_uri = uri.clone();
                manager.cookies(&uri, None::<&gtk::gio::Cancellable>, move |result| {
                match result {
                    Ok(cookies) => {
                        let cookies: Vec<_> = cookies.into_iter().map(|mut c| json!({"name":c.name().map(|s|s.to_string()), "value":c.value().map(|s|s.to_string()), "domain":c.domain().map(|s|s.to_string()), "path":c.path().map(|s|s.to_string()), "http_only":c.is_http_only(), "secure":c.is_secure(), "expires_unix":c.expires().map(|date|date.to_unix())})).collect();
                        let file = if index == 0 {"cookies.json".to_owned()} else {format!("cookies-{}.json", capture.id())};
                        capture.write(&file, json!(cookies).to_string().as_bytes());
                        capture.event("cookies", json!({"uri":callback_uri, "file":file}));
                    }
                    Err(error) => capture.event("cookie_error", json!({"error":error.to_string()})),
                }
                capture.pending.set(capture.pending.get() - 1);
            });
            }
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if !self.persistent {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

fn now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(unix)]
fn set_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_dir(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}
