use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use serde_json::{json, Value};
use webkit2gtk::*;

const DEFAULT_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/60.5 Safari/605.1.15";

pub struct Session {
    pub dir: PathBuf,
    pub user_agent: String,
    _lock: File,
}

impl Session {
    pub fn set_user_agent(&mut self, value: &str) -> io::Result<()> {
        if value.trim().is_empty() || value.contains(['\r', '\n']) {
            return Err(io::Error::other("Invalid user agent"));
        }
        private_write(&self.dir.join("user-agent.txt"), value.as_bytes())?;
        self.user_agent = value.to_owned();
        Ok(())
    }
}

pub fn app_config_dir() -> io::Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(base.join("animepahe-dl"))
}

pub fn default_session_dir() -> io::Result<PathBuf> {
    let app_dir = app_config_dir()?;
    let session_dir = app_dir.join("session");
    migrate_legacy_state(&app_dir, &session_dir)?;
    load_app_config()?;
    Ok(session_dir)
}

pub fn load_app_config() -> io::Result<Value> {
    let app_dir = app_config_dir()?;
    fs::create_dir_all(&app_dir)?;
    fs::set_permissions(&app_dir, fs::Permissions::from_mode(0o700))?;
    let config_file = app_dir.join("config.json");
    if !config_file.exists() {
        let legacy_config = PathBuf::from("config.json");
        if legacy_config.is_file() {
            private_write(&config_file, &fs::read(&legacy_config)?)?;
            fs::remove_file(legacy_config)?;
        } else {
            private_write(&config_file, include_bytes!("../config.default.json"))?;
        }
    }

    let mut config: Value = serde_json::from_slice(&fs::read(&config_file)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let defaults: Value = serde_json::from_str(include_str!("../config.default.json"))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let config_object = config.as_object_mut().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "config.json must be an object")
    })?;
    let default_object = defaults
        .as_object()
        .ok_or_else(|| io::Error::other("embedded default config must be an object"))?;
    let mut changed = false;
    for (key, value) in default_object {
        if !config_object.contains_key(key) {
            config_object.insert(key.clone(), value.clone());
            changed = true;
        }
    }
    if changed {
        let bytes = serde_json::to_vec_pretty(&config)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        private_write(&config_file, &bytes)?;
    }
    Ok(config)
}

fn migrate_legacy_state(app_dir: &Path, session_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(app_dir)?;
    fs::set_permissions(app_dir, fs::Permissions::from_mode(0o700))?;
    let legacy_session = PathBuf::from(".session");
    if legacy_session.is_dir() && !session_dir.exists() {
        match fs::rename(&legacy_session, session_dir) {
            Ok(()) => {}
            Err(_) => copy_session_dir(&legacy_session, session_dir)?,
        }
    }

    Ok(())
}

fn copy_session_dir(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_session_dir(&entry.path(), &target)?;
        } else if kind.is_file() && entry.file_name() != "lock" {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

impl Session {
    pub fn open(path: &Path) -> io::Result<Self> {
        fs::create_dir_all(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        let dir = fs::canonicalize(path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(dir.join("lock"))?;
        lock.try_lock()
            .map_err(|e| io::Error::other(format!("Session is already in use: {e}")))?;
        let ua_file = dir.join("user-agent.txt");
        let user_agent = match fs::read_to_string(&ua_file) {
            Ok(value) if !value.trim().is_empty() => value.trim().to_owned(),
            Ok(_) => return Err(io::Error::other("Saved user agent is empty")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                private_write(&ua_file, DEFAULT_UA.as_bytes())?;
                DEFAULT_UA.to_owned()
            }
            Err(error) => return Err(error),
        };
        if !dir.join("cookies.txt").exists() {
            private_write(&dir.join("cookies.txt"), b"# Netscape HTTP Cookie File\n")?;
        }
        Ok(Self {
            dir,
            user_agent,
            _lock: lock,
        })
    }

    pub fn import_capture(&mut self, capture: &Path) -> io::Result<()> {
        let events = fs::read_to_string(capture.join("events.jsonl"))?;
        let events: Vec<Value> = events
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        // Preserve the browser's actual user agent, not an invented replacement.
        if let Some(ua) = events
            .iter()
            .filter_map(|e| e["data"]["headers"].as_array())
            .flat_map(|headers| headers.iter())
            .find_map(|h| {
                (h[0].as_str()?.eq_ignore_ascii_case("user-agent"))
                    .then(|| h[1].as_str().map(str::to_owned))
                    .flatten()
            })
        {
            private_write(&self.dir.join("user-agent.txt"), ua.as_bytes())?;
            self.user_agent = ua;
        }
        self.save_capture_cookies(capture)?;
        private_write(&self.dir.join("session.json"), json!({"source_capture":capture, "user_agent":self.user_agent, "cookie_format":"Netscape", "updated_unix":now()}).to_string().as_bytes())
    }

    pub fn save_capture_cookies(&self, capture: &Path) -> io::Result<()> {
        let mut cookies = BTreeMap::new();
        let scopes: Vec<_> = fs::read_to_string(capture.join("events.jsonl"))?
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|event| event["event"] == "cookies")
            .filter_map(|event| {
                event["data"]["uri"]
                    .as_str()
                    .and_then(|uri| url::Url::parse(uri).ok())
            })
            .collect();
        // Refresh observed cookie scopes without erasing sessions on other sites.
        for cookie in read_jar(&self.dir.join("cookies.txt"))? {
            let observed = scopes.iter().any(|uri| {
                let domain = cookie.domain.trim_start_matches('.');
                let host = uri.host_str().unwrap_or("");
                let host_matches = host.eq_ignore_ascii_case(domain)
                    || (cookie.include_subdomains
                        && host
                            .to_ascii_lowercase()
                            .ends_with(&format!(".{}", domain.to_ascii_lowercase())));
                let path = uri.path();
                let path_matches = path == cookie.path
                    || (path.starts_with(&cookie.path)
                        && (cookie.path.ends_with('/')
                            || path[cookie.path.len()..].starts_with('/')));
                host_matches && path_matches && (!cookie.secure || uri.scheme() == "https")
            });
            if observed {
                continue;
            }
            let value = json!({"domain":cookie.domain,"path":cookie.path,"name":cookie.name,"value":cookie.value,"secure":cookie.secure,"http_only":cookie.http_only,"expires_unix":cookie.expires});
            cookies.insert((cookie.domain, cookie.path, cookie.name), value);
        }
        for entry in fs::read_dir(capture)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "cookies.json" || (name.starts_with("cookies-") && name.ends_with(".json")) {
                let values: Vec<Value> = serde_json::from_slice(&fs::read(entry.path())?)?;
                for value in values {
                    let key = (
                        value["domain"].as_str().unwrap_or("").to_owned(),
                        value["path"].as_str().unwrap_or("/").to_owned(),
                        value["name"].as_str().unwrap_or("").to_owned(),
                    );
                    cookies.insert(key, value);
                }
            }
        }
        let mut jar = String::from("# Netscape HTTP Cookie File\n# Saved by animepahe-dl, including session and HTTP-only cookies\n");
        for ((domain, path, name), value) in cookies {
            let expires = value["expires_unix"].as_i64().unwrap_or(0);
            if domain.is_empty() || name.is_empty() || (expires > 0 && expires <= now()) {
                continue;
            }
            let text = value["value"].as_str().unwrap_or("");
            if [&domain, &path, &name, text]
                .iter()
                .any(|s| s.contains(['\t', '\n', '\r']))
            {
                continue;
            }
            jar.push_str(&format!(
                "{}{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                if value["http_only"] == true {
                    "#HttpOnly_"
                } else {
                    ""
                },
                domain,
                if domain.starts_with('.') {
                    "TRUE"
                } else {
                    "FALSE"
                },
                path,
                if value["secure"] == true {
                    "TRUE"
                } else {
                    "FALSE"
                },
                expires,
                name,
                text
            ));
        }
        private_write(&self.dir.join("cookies.txt"), jar.as_bytes())
    }

    // Call only after the HTTP request has positively detected a challenge.
    pub fn browser(&self) -> Result<WebView, Box<dyn std::error::Error>> {
        let data_manager = WebsiteDataManager::builder()
            .base_data_directory(self.dir.join("browser-data").to_string_lossy().into_owned())
            .base_cache_directory(
                self.dir
                    .join("browser-cache")
                    .to_string_lossy()
                    .into_owned(),
            )
            .build();
        let context = WebContext::with_website_data_manager(&data_manager);
        let view = WebView::with_context(&context);
        let settings = view.settings().ok_or("WebKit settings unavailable")?;
        settings.set_user_agent(Some(&self.user_agent));
        let manager = context
            .cookie_manager()
            .ok_or("WebKit cookie manager unavailable")?;
        manager.set_persistent_storage(
            self.dir
                .join("browser-cookies.sqlite")
                .to_str()
                .ok_or("Invalid session path")?,
            CookiePersistentStorage::Sqlite,
        );
        // The HTTP jar is authoritative, including deletions received by curl.
        let cookies = read_jar(&self.dir.join("cookies.txt"))?;
        gtk::glib::MainContext::default().block_on(async {
            gtk::gio::GioFuture::new(&data_manager, |manager, cancellable, send| {
                manager.clear(
                    WebsiteDataTypes::COOKIES,
                    gtk::glib::TimeSpan::from_seconds(0),
                    Some(cancellable),
                    move |result| send.resolve(result),
                );
            })
            .await?;
            for mut cookie in cookies {
                let domain = if cookie.include_subdomains && !cookie.domain.starts_with('.') {
                    format!(".{}", cookie.domain)
                } else {
                    cookie.domain.clone()
                };
                let mut value =
                    soup::Cookie::new(&cookie.name, &cookie.value, &domain, &cookie.path, -1);
                value.set_secure(cookie.secure);
                value.set_http_only(cookie.http_only);
                if cookie.expires > 0 {
                    value.set_expires(&gtk::glib::DateTime::from_unix_utc(cookie.expires)?);
                }
                manager.add_cookie_future(&mut value).await?;
                cookie.value.clear();
            }
            Ok::<(), Box<dyn std::error::Error>>(())
        })?;
        Ok(view)
    }
}

struct Cookie {
    domain: String,
    include_subdomains: bool,
    path: String,
    secure: bool,
    expires: i64,
    name: String,
    value: String,
    http_only: bool,
}

fn read_jar(path: &Path) -> io::Result<Vec<Cookie>> {
    let mut cookies = vec![];
    for line in fs::read_to_string(path)?.lines() {
        let http_only = line.starts_with("#HttpOnly_");
        if line.starts_with('#') && !http_only {
            continue;
        }
        let fields: Vec<_> = line
            .trim_start_matches("#HttpOnly_")
            .splitn(7, '\t')
            .collect();
        if fields.len() != 7 {
            continue;
        }
        let expires = fields[4].parse().unwrap_or(0);
        if expires > 0 && expires <= now() {
            continue;
        }
        cookies.push(Cookie {
            domain: fields[0].into(),
            include_subdomains: fields[1] == "TRUE",
            path: fields[2].into(),
            secure: fields[3] == "TRUE",
            expires,
            name: fields[5].into(),
            value: fields[6].into(),
            http_only,
        });
    }
    Ok(cookies)
}

pub fn private_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refreshing_one_site_preserves_other_sites_and_unvisited_paths() {
        let root =
            std::env::temp_dir().join(format!("webkit-multisite-test-{}", std::process::id()));
        let capture = root.join("capture");
        fs::create_dir_all(&capture).unwrap();
        let session = Session::open(&root.join("session")).unwrap();
        private_write(&session.dir.join("cookies.txt"),b"# Netscape HTTP Cookie File\nfirst.test\tFALSE\t/\tFALSE\t0\tdeleted\told\nfirst.test\tFALSE\t/private\tFALSE\t0\tprivate\tkeep\nother.test\tFALSE\t/\tTRUE\t0\tclearance\tkeep\n").unwrap();
        fs::write(
            capture.join("events.jsonl"),
            "{\"event\":\"cookies\",\"data\":{\"uri\":\"https://first.test/\"}}\n",
        )
        .unwrap();
        fs::write(capture.join("cookies.json"), "[]").unwrap();
        session.save_capture_cookies(&capture).unwrap();
        let cookies = read_jar(&session.dir.join("cookies.txt")).unwrap();
        assert_eq!(cookies.len(), 2);
        assert!(cookies
            .iter()
            .any(|cookie| cookie.domain == "other.test" && cookie.name == "clearance"));
        assert!(cookies.iter().any(|cookie| cookie.path == "/private"));
        drop(session);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn imports_session_cookies_user_agent_and_expiry() {
        let root = std::env::temp_dir().join(format!("webkit-session-test-{}", std::process::id()));
        let capture = root.join("capture");
        fs::create_dir_all(&capture).unwrap();
        fs::write(
            capture.join("events.jsonl"),
            "{\"data\":{\"headers\":[[\"User-Agent\",\"fixture-agent\"]]}}\n",
        )
        .unwrap();
        fs::write(capture.join("cookies.json"),json!([
            {"domain":".example.test","path":"/","name":"clearance","value":"example-value","secure":true,"http_only":true,"expires_unix":now()+3600},
            {"domain":"example.test","path":"/","name":"session","value":"session-value","expires_unix":null},
            {"domain":"example.test","path":"/","name":"expired","value":"old","expires_unix":1}
        ]).to_string()).unwrap();
        let mut session = Session::open(&root.join("session")).unwrap();
        session.import_capture(&capture).unwrap();
        assert_eq!(session.user_agent, "fixture-agent");
        let cookies = read_jar(&session.dir.join("cookies.txt")).unwrap();
        assert_eq!(cookies.len(), 2);
        assert!(cookies
            .iter()
            .any(|c| c.http_only && c.secure && c.include_subdomains));
        assert!(cookies
            .iter()
            .any(|c| c.name == "session" && c.expires == 0));
        assert!(Session::open(&root.join("session")).is_err());
        drop(session);
        let session = Session::open(&root.join("session")).unwrap();
        assert_eq!(session.user_agent, "fixture-agent");
        drop(session);
        fs::remove_dir_all(root).unwrap();
    }
}
