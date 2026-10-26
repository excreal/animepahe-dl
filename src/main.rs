mod animepahe;
mod capture;
mod http;
mod session;

use gtk::prelude::*;
use javascriptcore::ValueExt;
use serde_json::json;
use std::{
    cell::Cell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};
use webkit2gtk::{LoadEvent, URIResponseExt, WebResourceExt, WebView, WebViewExt};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut raw_args: Vec<String> = std::env::args().skip(1).collect();
    if raw_args.len() == 1 && matches!(raw_args[0].as_str(), "--version" | "-V") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if raw_args.is_empty()
        || (raw_args.len() == 1 && matches!(raw_args[0].as_str(), "--help" | "-h"))
    {
        print_help();
        return Ok(());
    }
    let anime_mode = raw_args.first().is_some_and(|arg| arg == "animepahe")
        || raw_args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "-a" | "-s"
                    | "-e"
                    | "-r"
                    | "-o"
                    | "-l"
                    | "-f"
                    | "-d"
                    | "--name"
                    | "--slug"
                    | "--episodes"
                    | "--resolution"
                    | "--language"
                    | "--force"
            )
        });
    if anime_mode {
        if raw_args.first().is_some_and(|arg| arg == "animepahe") {
            raw_args.remove(0);
        }
        let mut session_dir = None;
        let mut capture_enabled = false;
        let mut filtered = Vec::new();
        let mut i = 0;
        while i < raw_args.len() {
            if raw_args[i] == "--session-dir" {
                i += 1;
                let path = raw_args
                    .get(i)
                    .filter(|path| !path.starts_with('-') && !path.trim().is_empty())
                    .ok_or("--session-dir requires a path")?;
                session_dir = Some(PathBuf::from(path));
            } else if raw_args[i] == "--capture" {
                capture_enabled = true;
            } else {
                filtered.push(raw_args[i].clone());
            }
            i += 1;
        }
        if filtered
            .iter()
            .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
        {
            animepahe::print_help();
            return Ok(());
        }
        let session_dir = match session_dir {
            Some(path) => path,
            None => session::default_session_dir()?,
        };
        let mut session = session::Session::open(&session_dir)?;
        let capture = capture::Capture::new(capture_enabled)?;
        if capture_enabled {
            println!("Capture: {}", capture.dir.display());
        }
        return animepahe::run(&filtered, capture, &mut session);
    }
    let mut args = raw_args.into_iter();
    let mut url = None;
    let mut session_dir = None;
    let mut import = None;
    let mut import_only = false;
    let mut http_only = false;
    let mut capture_enabled = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--url" => url = Some(next_cli_value(&mut args, "--url requires a URL")?),
            "--session-dir" => {
                session_dir = Some(PathBuf::from(next_cli_value(
                    &mut args,
                    "--session-dir requires a path",
                )?))
            }
            "--import-session" => {
                import = Some(PathBuf::from(next_cli_value(
                    &mut args,
                    "--import-session requires a capture path",
                )?))
            }
            "--import-only" => import_only = true,
            "--http-only" => http_only = true,
            "--capture" => capture_enabled = true,
            "--help" | "-h" => {
                print_generic_help();
                return Ok(());
            }
            _ => return Err(format!("Unknown argument: {arg}").into()),
        }
    }
    if import_only && import.is_none() {
        return Err("--import-only requires --import-session CAPTURE".into());
    }
    if !import_only {
        let provided_url = url
            .as_deref()
            .ok_or("No URL provided; specify --url URL or use the animepahe command")?;
        let parsed = url::Url::parse(provided_url)?;
        if !["http", "https"].contains(&parsed.scheme()) {
            return Err("Only HTTP(S) URLs are supported".into());
        }
    }
    let session_dir = match session_dir {
        Some(path) => path,
        None => session::default_session_dir()?,
    };
    let mut session = session::Session::open(&session_dir)?;
    if let Some(path) = import {
        session.import_capture(&path)?;
        println!("Imported session from {}", path.display());
    }
    if import_only {
        println!("Reusable session saved in {}", session.dir.display());
        return Ok(());
    }
    let url = url.ok_or("No URL provided; specify --url URL")?;
    let capture = capture::Capture::new(capture_enabled)?;
    if capture_enabled {
        println!("Capturing to {}", capture.dir.display());
    }
    println!("Fetching {url} using the saved session");
    let mut page = http::fetch(&url, &capture, &session, "http")?;
    if !page.challenge {
        if !(200..300).contains(&page.status) {
            let message = if capture_enabled {
                format!(
                    "HTTP {} without a CAPTCHA; response saved to {}",
                    page.status,
                    capture.dir.display()
                )
            } else {
                format!(
                    "HTTP {} without a CAPTCHA; rerun with --capture to save diagnostics",
                    page.status
                )
            };
            return Err(message.into());
        }
        animepahe::enrich_play_page(
            &url,
            "http",
            capture.clone(),
            &mut session,
            &mut page.details,
        )?;
        println!(
            "Page: {} (HTTP {})",
            page.details["title"].as_str().unwrap_or("Untitled"),
            page.status
        );
        capture.event("exit", json!({"mode":"http","browser_started":false}));
        if capture_enabled {
            println!(
                "Page details saved to {}. No browser started.",
                capture.dir.join("page-details.json").display()
            );
        }
        return Ok(());
    }
    if http_only {
        return Err("CAPTCHA detected; --http-only prevented the browser from starting.".into());
    }
    println!("CAPTCHA detected. Opening WebKit for manual verification.");
    gtk::init().map_err(|error| {
        format!("A challenge requires a desktop session, but GTK could not initialize: {error}")
    })?;
    let completed = run_browser(capture.clone(), &session, &url)?;
    if !completed {
        let message = if capture_enabled {
            format!(
                "Browser closed or stopped before destination completion. Capture saved to {}",
                capture.dir.display()
            )
        } else {
            "Browser closed or stopped before destination completion. Rerun with --capture to save diagnostics.".to_owned()
        };
        return Err(message.into());
    }
    session.import_capture(&capture.dir)?;
    println!(
        "Session cookies, profile, and user agent saved in {}",
        session.dir.display()
    );
    // Check whether this browser session is also accepted by the HTTP client.
    match http::fetch(&url, &capture, &session, "http-after-browser") {
        Ok(mut page) if !page.challenge && (200..300).contains(&page.status) => {
            animepahe::enrich_play_page(
                &url,
                "http-after-browser",
                capture.clone(),
                &mut session,
                &mut page.details,
            )?;
            println!("Saved session works over HTTP; future runs will try it first.")
        }
        Ok(_) => {
            http::save_browser_details(&capture)?;
            println!("The HTTP client was not granted the destination page. Browser page details and session are saved; future runs will retry HTTP once before opening a challenge.");
        }
        Err(error) => {
            http::save_browser_details(&capture)?;
            eprintln!("Browser succeeded, but HTTP reuse could not be checked: {error}");
        }
    }
    Ok(())
}

fn print_help() {
    println!("animepahe-dl {}", env!("CARGO_PKG_VERSION"));
    println!("\nUsage:\n  animepahe-dl animepahe [ANIME OPTIONS] [GENERAL OPTIONS]\n  animepahe-dl [PAGE OPTIONS]\n  animepahe-dl --import-session CAPTURE --import-only [--session-dir PATH]\n  animepahe-dl --version\n\nAnimePahe options (also accepted directly without `animepahe`):\n  -a, --name NAME        search for an anime title\n  -s, --slug SLUG        select an anime by its session slug\n  -e, --episodes RANGE   numbers, comma-separated list, ranges, or *\n  -r, --resolution HEIGHT requested resolution (config default: best)\n  -o, --language CODE    audio language (config default: original)\n  -l                     print playlist URLs without downloading\n  -f, --force            replace existing episode files\n  -d                     log request URLs and statuses\n  --capture              save request and browser diagnostics\n  --session-dir PATH     session storage location\n\nPage inspector options (requires an explicit --url):\n  --url URL              inspect an HTTP(S) page; required for page inspection\n  --http-only            do not open WebKit when a CAPTCHA is detected\n  --capture              save request and browser diagnostics\n  --session-dir PATH     session storage location\n  --import-session PATH  import browser session data from a capture\n  --import-only          import session only; requires --import-session\n\nGlobal options:\n  -h, --help             show this help\n  -V, --version          print the version\n\nRunning with no arguments only prints this help; it makes no request.\nFor focused help: `animepahe-dl animepahe --help` or `animepahe-dl --url URL --help`.\nThe default session is stored under $XDG_CONFIG_HOME/animepahe-dl/session,\nor ~/.config/animepahe-dl/session when XDG_CONFIG_HOME is unset.");
}

fn print_generic_help() {
    println!("Page inspector\n\nUsage: animepahe-dl --url URL [OPTIONS]\n       animepahe-dl --import-session CAPTURE --import-only [--session-dir PATH]\n\nOptions:\n  --url URL              inspect an HTTP(S) page (required unless importing only)\n  --session-dir PATH     session storage location\n  --http-only            do not open WebKit when a CAPTCHA is detected\n  --capture              save request and browser diagnostics\n  --import-session PATH  import browser session data from a capture\n  --import-only          import session only; requires --import-session\n  -h, --help             show this help\n\nThe default session is stored under $XDG_CONFIG_HOME/animepahe-dl/session,\nor ~/.config/animepahe-dl/session when XDG_CONFIG_HOME is unset.");
}

fn next_cli_value(
    args: &mut impl Iterator<Item = String>,
    message: &'static str,
) -> Result<String, Box<dyn std::error::Error>> {
    args.next()
        .filter(|value| !value.starts_with('-') && !value.trim().is_empty())
        .ok_or_else(|| message.into())
}

pub(crate) fn run_browser(
    capture: Rc<capture::Capture>,
    session: &session::Session,
    url: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let main_loop = gtk::glib::MainLoop::new(None, false);
    let finishing = Rc::new(Cell::new(false));
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Page — solve the CAPTCHA manually");
    window.set_default_size(960, 680);
    window.set_position(gtk::WindowPosition::Center);
    window.unmaximize();
    let webview = session.browser()?;
    capture.attach(&webview);
    let ready_checks = Rc::new(Cell::new(0));
    let weak_window = window.downgrade();
    let load_capture = capture.clone();
    let load_checks = ready_checks.clone();
    webview.connect_load_changed(move |view, event| {
        load_capture.event("load", json!({"event":format!("{event:?}"), "uri":view.uri().map(|s|s.to_string()), "title":view.title().map(|s|s.to_string())}));
        if event == LoadEvent::Started { load_checks.set(0); }
        if event == LoadEvent::Finished {
            let title = view.title().unwrap_or_else(|| "Untitled".into());
            load_capture.snapshot(view, "loaded");
            if let Some(window) = weak_window.upgrade() { window.set_title(&format!("{title} — animepahe-dl")); }
        }
    });
    let error_capture = capture.clone();
    webview.connect_load_failed(move |_, _, uri, error| {
        error_capture.event("load_failed", json!({"uri":uri,"error":error.to_string()}));
        eprintln!("Failed to load {uri}: {error}");
        false
    });
    let exit_loop = main_loop.clone();
    let exit_capture = capture.clone();
    webview.connect_web_process_terminated(move |_, reason| {
        exit_capture.event(
            "process_terminated",
            json!({"reason":format!("{reason:?}")}),
        );
        // MainLoop::quit is safe even after the loop has stopped.
        exit_loop.quit();
    });
    window.add(&webview);
    let close_capture = capture.clone();
    let close_finishing = finishing.clone();
    let close_loop = main_loop.clone();
    let weak_view = webview.downgrade();
    let weak_window = window.downgrade();
    window.connect_delete_event(move |_, _| {
        if let Some(view) = weak_view.upgrade() {
            finish(
                &view,
                &close_capture,
                &close_finishing,
                &close_loop,
                "window_closed",
            );
        }
        if let Some(window) = weak_window.upgrade() {
            window.hide();
        }
        gtk::glib::Propagation::Stop
    });
    let checking = Rc::new(Cell::new(false));
    let weak_view = webview.downgrade();
    let poll_capture = capture.clone();
    let poll_loop = main_loop.clone();
    let poll_finishing = finishing.clone();
    let completed = Rc::new(Cell::new(false));
    let poll_completed = completed.clone();
    let poll = gtk::glib::timeout_add_local(Duration::from_secs(1), move || {
        let Some(view) = weak_view.upgrade() else {
            return gtk::glib::ControlFlow::Continue;
        };
        if poll_finishing.get() {
            return gtk::glib::ControlFlow::Continue;
        }
        if !checking.replace(true) {
            poll_capture.snapshot(&view, "poll");
            let ready_checks = ready_checks.clone();
            let checking = checking.clone();
            let capture = poll_capture.clone();
            let finishing = poll_finishing.clone();
            let main_loop = poll_loop.clone();
            let weak_view = view.downgrade();
            let completed = poll_completed.clone();
            let script = format!(
                "{}; JSON.stringify(window.__webkitPageState)",
                include_str!("page_ready.js")
            );
            view.evaluate_javascript(
                &script,
                None,
                None,
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    checking.set(false);
                    let Some(view) = weak_view.upgrade() else {
                        return;
                    };
                    let state: serde_json::Value = result.as_ref().ok().and_then(|value|serde_json::from_str(&value.to_str()).ok()).unwrap_or_else(||json!({"state":"unknown", "reason":"State inspection failed"}));
                    let response = view.main_resource().and_then(|r|r.response());
                    let status = response.as_ref().map(|r|r.status_code());
                    let non_html = response.as_ref().and_then(|r|r.mime_type()).is_some_and(|mime| {
                        let mime = mime.to_ascii_lowercase();
                        !mime.starts_with("text/html") && !mime.starts_with("application/xhtml+xml")
                    });
                    let mut mitigated = false;
                    if let Some(headers) = response.and_then(|r|r.http_headers()) {
                        headers.foreach(|name,value| {if name.eq_ignore_ascii_case("cf-mitigated") && value.eq_ignore_ascii_case("challenge") {mitigated=true;}});
                    }
                    let ready = (state["ready"] == true || non_html) && status.is_some_and(|status|(200..300).contains(&status)) && !mitigated && !view.is_loading();
                    capture.event(
                        "completion_check",
                        json!({"ready":ready, "state":state, "main_status":status, "cf_mitigated":mitigated, "error":result.err().map(|e|e.to_string())}),
                    );
                    if (state["ready"] == true || non_html) && status.is_some_and(|status|status>=400) && !mitigated && !view.is_loading() {
                        eprintln!("Destination returned HTTP {} without a detected CAPTCHA.",status.unwrap_or_default());
                        finish(&view,&capture,&finishing,&main_loop,"http_error");
                        return;
                    }
                    ready_checks.set(if ready { ready_checks.get() + 1 } else { 0 });
                    if ready_checks.get() >= 2 {
                        completed.set(true);
                        if capture.persistent() {
                            println!("Destination page finished loading. Saving captures and exiting.");
                        } else {
                            println!("Request completed. Closing browser.");
                        }
                        finish(&view, &capture, &finishing, &main_loop, "complete");
                    }
                },
            );
        }
        gtk::glib::ControlFlow::Continue
    });
    println!("Solve the CAPTCHA manually in the window. Waiting without a time limit.");
    capture.event("start", json!({"url":url,"user_agent":session.user_agent}));
    webview.load_uri(url);
    window.show_all();
    main_loop.run();
    poll.remove();
    webview.terminate_web_process();
    window.hide();
    // The GTK loop has stopped and this function owns the top-level window.
    unsafe { window.destroy() };
    capture.event("exit", json!({"pending":capture.pending.get()}));
    if capture.persistent() {
        println!("Capture saved to {}", capture.dir.display());
    }
    Ok(completed.get())
}

fn finish(
    view: &WebView,
    capture: &Rc<capture::Capture>,
    finishing: &Cell<bool>,
    main_loop: &gtk::glib::MainLoop,
    reason: &str,
) {
    if finishing.replace(true) {
        return;
    }
    capture.event("finishing", json!({"reason":reason}));
    capture.finalize(view);
    let started = Instant::now();
    let capture = capture.clone();
    let main_loop = main_loop.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
        if capture.pending.get() == 0 || started.elapsed() >= Duration::from_secs(10) {
            capture.event(
                "capture_finished",
                json!({"pending":capture.pending.get(), "timed_out":capture.pending.get()>0}),
            );
            main_loop.quit();
            gtk::glib::ControlFlow::Break
        } else {
            gtk::glib::ControlFlow::Continue
        }
    });
}

#[cfg(test)]
pub mod tests {
    pub fn captures_page_and_exits_without_shutdown_panic() {
        use super::*;
        use std::io::{Read, Write};
        gtk::init().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for incoming in listener.incoming().take(2) {
                let mut stream = incoming.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 8192];
                let size = stream.read(&mut request).unwrap();
                requests.push(String::from_utf8_lossy(&request[..size]).into_owned());
                let script = String::from_utf8_lossy(&request[..size]).starts_with("GET /app.js ");
                let (mime, body) = if script {
                    ("application/javascript", "console.log('fixture console');")
                } else {
                    ("text/html", "<title>Fixture</title><p>Captured fixture</p><script src='/app.js'></script>")
                };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nSet-Cookie: capture_fixture=present; Path=/; HttpOnly\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        let capture = capture::Capture::new(true).unwrap();
        let main_loop = gtk::glib::MainLoop::new(None, false);
        let finishing = Rc::new(Cell::new(false));
        let session = session::Session::open(&capture.dir.join("fixture-session")).unwrap();
        session::private_write(&session.dir.join("cookies.txt"),b"# Netscape HTTP Cookie File\n#HttpOnly_127.0.0.1\tFALSE\t/\tFALSE\t0\trestored_browser\tyes\n").unwrap();
        let view = session.browser().unwrap();
        capture.attach(&view);
        let load_loop = main_loop.clone();
        let load_capture = capture.clone();
        view.connect_load_changed(move |view, event| {
            if event == LoadEvent::Finished {
                finish(view, &load_capture, &finishing, &load_loop, "fixture");
            }
        });
        let exit_loop = main_loop.clone();
        view.connect_web_process_terminated(move |_, _| exit_loop.quit());
        view.load_uri(&url);
        main_loop.run();
        view.terminate_web_process();
        assert_eq!(capture.pending.get(), 0);
        assert!(capture.dir.join("page.mhtml").exists());
        assert!(capture.dir.join("cookies.json").exists());
        let snapshot = std::fs::read_dir(&capture.dir)
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().ends_with("-final.json"))
            .unwrap();
        let snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(snapshot.path()).unwrap()).unwrap();
        assert!(snapshot["html"]
            .as_str()
            .unwrap()
            .contains("Captured fixture"));
        assert!(snapshot["console"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message.to_string().contains("fixture console")));
        let events = std::fs::read_to_string(capture.dir.join("events.jsonl")).unwrap();
        let events: Vec<serde_json::Value> = events
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(events
            .iter()
            .any(|entry| entry["event"] == "sent_request" && entry["data"]["method"] == "GET"));
        assert!(events.iter().any(|entry| entry["event"] == "response"
            && entry["data"]["mime"] == "application/javascript"));
        assert!(events.iter().any(|entry| entry["event"] == "response"
            && entry["data"]["headers"]
                .to_string()
                .to_lowercase()
                .contains("content-type")));
        let bodies: Vec<_> = events
            .iter()
            .filter(|entry| entry["event"] == "body")
            .map(|entry| {
                std::fs::read_to_string(capture.dir.join(entry["data"]["file"].as_str().unwrap()))
                    .unwrap()
            })
            .collect();
        assert!(bodies.iter().any(|body| body.contains("Captured fixture")));
        assert!(bodies.iter().any(|body| body.contains("fixture console")));
        let cookie_files: Vec<_> = events
            .iter()
            .filter(|entry| entry["event"] == "cookies")
            .map(|entry| {
                std::fs::read_to_string(capture.dir.join(entry["data"]["file"].as_str().unwrap()))
                    .unwrap()
            })
            .collect();
        assert!(cookie_files
            .iter()
            .any(|cookies| cookies.contains("capture_fixture")));
        let requests = server.join().unwrap();
        assert!(requests
            .iter()
            .all(|request| request.contains("restored_browser=yes")));
        assert!(requests
            .iter()
            .all(|request| request.contains(&session.user_agent)));
        session.save_capture_cookies(&capture.dir).unwrap();
        let cookies = std::fs::read_to_string(session.dir.join("cookies.txt")).unwrap();
        assert!(cookies.contains("restored_browser\tyes"));
        assert!(cookies.contains("capture_fixture\tpresent"));
        drop(session);
        std::fs::remove_dir_all(&capture.dir).unwrap();
    }

    #[test]
    fn quitting_after_loop_stops_is_safe() {
        let context = gtk::glib::MainContext::new();
        let main_loop = gtk::glib::MainLoop::new(Some(&context), false);
        main_loop.quit();
        let callback_loop = main_loop.clone();
        context.spawn_local(async move {
            callback_loop.quit();
        });
        main_loop.run();
        assert!(!main_loop.is_running());
        main_loop.quit();
    }
}
