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
    let config_file = app_dir.join("config.toml");
    let legacy_app_config = app_dir.join("config.json");
    let legacy_project_config = PathBuf::from("config.json");
    let defaults_text = include_str!("../config.default.toml");
    let mut config = if config_file.is_file() {
        parse_toml_config(&fs::read_to_string(&config_file)?)?
    } else if legacy_app_config.is_file() {
        serde_json::from_slice(&fs::read(&legacy_app_config)?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
    } else if legacy_project_config.is_file() {
        serde_json::from_slice(&fs::read(&legacy_project_config)?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
    } else {
        parse_toml_config(defaults_text)?
    };
    let defaults = parse_toml_config(defaults_text)?;
    let config_object = config.as_object_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "config must be a key/value object",
        )
    })?;
    let default_object = defaults
        .as_object()
        .ok_or_else(|| io::Error::other("embedded default config must be an object"))?;
    let mut changed = !config_file.is_file();
    changed |= config_object.remove("ua").is_some();
    changed |= config_object.remove("cf").is_some();
    for (key, value) in default_object {
        if !config_object.contains_key(key) {
            config_object.insert(key.clone(), value.clone());
            changed = true;
        }
    }
    if changed {
        let rendered = render_toml_config(&config, defaults_text)?;
        private_write(&config_file, rendered.as_bytes())?;
        if legacy_project_config.is_file() {
            fs::remove_file(legacy_project_config)?;
        }
        if legacy_app_config.is_file() {
            fs::remove_file(legacy_app_config)?;
        }
    }
    Ok(config)
}

fn parse_toml_config(text: &str) -> io::Result<Value> {
    let mut values = serde_json::Map::new();
    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, raw_value) = line.split_once('=').ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid config line {}", line_number + 1),
            )
        })?;
        let raw_value = strip_toml_comment(raw_value).trim();
        let value = serde_json::from_str(raw_value).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid config value on line {}: {error}", line_number + 1),
            )
        })?;
        values.insert(key.trim().to_owned(), value);
    }
    Ok(Value::Object(values))
}

fn strip_toml_comment(value: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' && in_string {
            escaped = true;
        } else if character == '"' {
            in_string = !in_string;
        } else if character == '#' && !in_string {
            return &value[..index];
        }
    }
    value
}

fn render_toml_config(config: &Value, template: &str) -> io::Result<String> {
    let object = config.as_object().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "config must be a key/value object",
        )
    })?;
    let mut output = String::new();
    let mut written = std::collections::BTreeSet::new();
    for line in template.lines() {
        if let Some((key, _)) = line.split_once('=') {
            let key = key.trim();
            if let Some(value) = object.get(key) {
                let value = serde_json::to_string(value)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                output.push_str(&format!("{key} = {value}\n"));
                written.insert(key.to_owned());
                continue;
            }
        }
        output.push_str(line);
        output.push('\n');
    }
    for (key, value) in object {
        if !written.contains(key) {
            let value = serde_json::to_string(value)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            output.push_str(&format!("\n# User-defined option.\n{key} = {value}\n"));
        }
    }
    Ok(output)
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
        let all_events: Vec<Value> = fs::read_to_string(capture.join("events.jsonl"))?
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect();
        let mut cookie_events: Vec<Value> = all_events
            .iter()
            .filter(|event| event["event"] == "cookies")
            .cloned()
            .collect();
        let current_generation = all_events
            .iter()
            .filter(|event| event["event"] == "browser_start")
            .filter_map(|event| event["data"]["browser_generation"].as_u64())
            .max();
        let current_generation = current_generation.or_else(|| {
            cookie_events
                .iter()
                .filter_map(|event| event["data"]["browser_generation"].as_u64())
                .max()
        });
        let mut legacy_cookie_file = false;
        if let Some(generation) = current_generation {
            cookie_events
                .retain(|event| event["data"]["browser_generation"].as_u64() == Some(generation));
        } else if cookie_events.is_empty() {
            // Older imported captures only have cookies.json and no cookie events.
            legacy_cookie_file = capture.join("cookies.json").exists();
        } else {
            // Legacy captures reused cookies.json and left numbered snapshots behind.
            // Only import the current contents of that file, never historical numbered files.
            cookie_events.retain(|event| {
                event["data"]["file"].as_str().unwrap_or("cookies.json") == "cookies.json"
            });
            legacy_cookie_file = cookie_events.is_empty() && capture.join("cookies.json").exists();
        }
        cookie_events.sort_by(|left, right| {
            left["data"]["index"]
                .as_u64()
                .unwrap_or(0)
                .cmp(&right["data"]["index"].as_u64().unwrap_or(0))
                .then_with(|| {
                    left["data"]["file"]
                        .as_str()
                        .unwrap_or("cookies.json")
                        .cmp(right["data"]["file"].as_str().unwrap_or("cookies.json"))
                })
        });
        let scopes: Vec<_> = cookie_events
            .iter()
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
        let cookie_files: Vec<String> = if legacy_cookie_file {
            vec!["cookies.json".to_owned()]
        } else {
            cookie_events
                .iter()
                .map(|event| {
                    event["data"]["file"]
                        .as_str()
                        .unwrap_or("cookies.json")
                        .to_owned()
                })
                .collect()
        };
        for name in cookie_files {
            let values: Vec<Value> = serde_json::from_slice(&fs::read(capture.join(name))?)?;
            for value in values {
                let key = (
                    value["domain"].as_str().unwrap_or("").to_owned(),
                    value["path"].as_str().unwrap_or("/").to_owned(),
                    value["name"].as_str().unwrap_or("").to_owned(),
                );
                cookies.insert(key, value);
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
    fn imports_only_latest_browser_cookie_batch_in_stable_order() {
        let root =
            std::env::temp_dir().join(format!("webkit-cookie-batches-{}", std::process::id()));
        let capture = root.join("capture");
        fs::create_dir_all(&capture).unwrap();
        let session = Session::open(&root.join("session")).unwrap();

        let stale = json!([{"domain":"first.test","path":"/","name":"stale","value":"old"}]);
        let current_first =
            json!([{"domain":"first.test","path":"/","name":"winner","value":"first"}]);
        let current_last =
            json!([{"domain":"first.test","path":"/","name":"winner","value":"last"}]);
        fs::write(capture.join("cookies-1-0.json"), stale.to_string()).unwrap();
        fs::write(capture.join("cookies-2-0.json"), current_first.to_string()).unwrap();
        fs::write(capture.join("cookies-2-1.json"), current_last.to_string()).unwrap();
        let events = [
            json!({"event":"cookies","data":{"uri":"https://first.test/","file":"cookies-1-0.json","browser_generation":1,"index":0}}),
            json!({"event":"cookies","data":{"uri":"https://first.test/","file":"cookies-2-1.json","browser_generation":2,"index":1}}),
            json!({"event":"cookies","data":{"uri":"https://first.test/","file":"cookies-2-0.json","browser_generation":2,"index":0}}),
        ];
        fs::write(
            capture.join("events.jsonl"),
            events
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();

        session.save_capture_cookies(&capture).unwrap();
        let cookies = read_jar(&session.dir.join("cookies.txt")).unwrap();
        assert!(!cookies.iter().any(|cookie| cookie.name == "stale"));
        assert_eq!(
            cookies
                .iter()
                .find(|cookie| cookie.name == "winner")
                .unwrap()
                .value,
            "last"
        );
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
