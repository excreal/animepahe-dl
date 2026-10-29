use super::{
    capture::Capture,
    session::{private_write, Session},
};
use scraper::{Html, Selector};
use serde_json::{json, Value};
use std::{fs, io, process::Command};

pub struct Page {
    pub challenge: bool,
    pub status: u16,
    pub details: Value,
}

pub fn save_browser_details(capture: &Capture) -> io::Result<()> {
    let snapshot = fs::read_dir(&capture.dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with("-final.json"))
        .max_by_key(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .split('-')
                .nth(1)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        })
        .ok_or_else(|| io::Error::other("No final browser snapshot available"))?;
    let snapshot: Value = serde_json::from_slice(&fs::read(snapshot.path())?)?;
    let mut page = inspect(
        snapshot["url"].as_str().unwrap_or(""),
        200,
        &[],
        snapshot["html"].as_str().unwrap_or(""),
    );
    page.details["javascript_executed"] = json!(true);
    page.details["source"] = json!("browser_snapshot");
    private_write(
        &capture.dir.join("page-details.json"),
        serde_json::to_string_pretty(&page.details)?.as_bytes(),
    )
}

/// No GTK initialization or WebKit process is needed on this path.
pub fn fetch(url: &str, capture: &Capture, session: &Session, label: &str) -> io::Result<Page> {
    let dir = capture.dir.join(label);
    fs::create_dir_all(&dir)?;
    capture.event(
        "http_start",
        json!({"url":url,"directory":label,"user_agent":session.user_agent}),
    );
    let output = Command::new("curl")
        .args([
            "--disable",
            "--silent",
            "--show-error",
            "--location",
            "--max-redirs",
            "5",
            "--proto",
            "=http,https",
            "--proto-redir",
            "=http,https",
            "--connect-timeout",
            "15",
            "--max-time",
            "45",
            "--compressed",
            "--trace-time",
        ])
        .arg("--trace-ascii")
        .arg(dir.join("trace.txt"))
        .arg("--dump-header")
        .arg(dir.join("headers.txt"))
        .arg("--output")
        .arg(dir.join("body.html"))
        .arg("--cookie")
        .arg(session.dir.join("cookies.txt"))
        .arg("--cookie-jar")
        .arg(session.dir.join("cookies.txt"))
        .arg("--user-agent")
        .arg(&session.user_agent)
        .args(["--write-out", "%{json}", "--url", url])
        .output()?;
    private_write(&dir.join("curl.json"), &output.stdout)?;
    if !output.status.success() {
        private_write(&dir.join("error.txt"), &output.stderr)?;
        capture.event(
            "http_error",
            json!({"directory":label,"error":String::from_utf8_lossy(&output.stderr)}),
        );
        return Err(io::Error::other(format!(
            "HTTP fetch failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let status = metadata["http_code"].as_u64().unwrap_or(0) as u16;
    let final_url = metadata["url_effective"].as_str().unwrap_or(url);
    let raw_headers = fs::read_to_string(dir.join("headers.txt"))?;
    let headers = final_headers(&raw_headers);
    let body = fs::read(dir.join("body.html"))?;
    let page = inspect(final_url, status, &headers, &String::from_utf8_lossy(&body));
    private_write(
        &dir.join("page-details.json"),
        serde_json::to_string_pretty(&page.details)?.as_bytes(),
    )?;
    private_write(
        &capture.dir.join("page-details.json"),
        serde_json::to_string_pretty(&page.details)?.as_bytes(),
    )?;
    capture.event("http_result",json!({"directory":label,"url":final_url,"status":status,"challenge":page.challenge,"title":page.details["title"],"body_bytes":body.len()}));
    Ok(page)
}

pub(crate) fn final_headers(raw: &str) -> Vec<(String, String)> {
    let normalized = raw.replace("\r\n", "\n");
    normalized
        .rsplit("\n\n")
        .find(|block| block.starts_with("HTTP/"))
        .unwrap_or("")
        .lines()
        .skip(1)
        .filter_map(|line| {
            line.split_once(':')
                .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        })
        .collect()
}

pub(crate) fn challenge_header_present(raw: &str) -> bool {
    raw.lines()
        .filter_map(|line| line.split_once(':'))
        .any(|(name, value)| {
            name.eq_ignore_ascii_case("cf-mitigated")
                && value.trim().eq_ignore_ascii_case("challenge")
        })
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("Static HTML selector")
}

pub(crate) fn inspect(url: &str, status: u16, headers: &[(String, String)], body: &str) -> Page {
    let html = Html::parse_document(body);
    let title = html
        .select(&selector("title"))
        .next()
        .map(|e| e.text().collect::<String>())
        .unwrap_or_default();
    let lower = title.to_ascii_lowercase();
    let challenged_header = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("cf-mitigated") && v.eq_ignore_ascii_case("challenge")
    });
    let challenged_title = [
        "just a moment",
        "attention required",
        "verify you are human",
        "security verification",
    ]
    .iter()
    .any(|s| lower.contains(s));
    // A background Cloudflare jsd script on a normal page is not a CAPTCHA.
    let solved_token=html.select(&selector("[name='cf-turnstile-response'], [name='g-recaptcha-response'], [name='h-captcha-response']")).any(|field| !field.value().attr("value").unwrap_or("").trim().is_empty() || !field.text().collect::<String>().trim().is_empty());
    let challenged_ui = !solved_token && html
        .select(&selector(
            ".cf-turnstile, .g-recaptcha, .h-captcha, #aws-waf-captcha-container, #challenge-form, iframe[src*='challenges.cloudflare.com'], iframe[src*='hcaptcha.com'], iframe[src*='google.com/recaptcha/']",
        ))
        .any(|element| !std::iter::once(element).chain(element.ancestors().filter_map(scraper::ElementRef::wrap)).any(|ancestor| ancestor.value().attr("hidden").is_some() || ancestor.value().attr("style").is_some_and(|style| {let style=style.replace(' ',"").to_ascii_lowercase();style.contains("display:none")||style.contains("visibility:hidden")})));
    let challenge = challenged_header || challenged_title || challenged_ui;
    let base = url::Url::parse(url).ok();
    let absolute = |value: &str| {
        base.as_ref()
            .and_then(|b| b.join(value).ok())
            .map(|u| u.to_string())
            .unwrap_or_else(|| value.to_owned())
    };
    let forms:Vec<_>=html.select(&selector("form")).map(|form|json!({
        "method":form.value().attr("method").unwrap_or("GET").to_ascii_uppercase(),
        "action":absolute(form.value().attr("action").unwrap_or(url)),
        "inputs":form.select(&selector("input")).map(|input|json!({"name":input.value().attr("name"),"type":input.value().attr("type"),"value":input.value().attr("value")})).collect::<Vec<_>>()
    })).collect();
    let assets: Vec<_> = html
        .select(&selector("script[src], link[href], img[src]"))
        .filter_map(|e| e.value().attr("src").or_else(|| e.value().attr("href")))
        .map(absolute)
        .collect();
    let text = html
        .select(&selector("body"))
        .next()
        .map(|body| body.text().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let rows: Vec<_> = html
        .select(&selector("table tr"))
        .map(|row| {
            row.select(&selector("th,td"))
                .map(|cell| cell.text().collect::<Vec<_>>().join(" ").trim().to_owned())
                .collect::<Vec<_>>()
        })
        .filter(|row| !row.is_empty())
        .collect();
    let mut fields = serde_json::Map::new();
    for label in html.select(&selector("strong, b")) {
        let name = label.text().collect::<String>().trim().to_owned();
        if let Some(parent) = label.parent().and_then(scraper::ElementRef::wrap) {
            let value = parent.text().collect::<Vec<_>>().join(" ");
            let Some(value) = value
                .trim()
                .strip_prefix(&name)
                .and_then(|text| text.trim_start().strip_prefix(':'))
            else {
                continue;
            };
            let value = value.trim().to_owned();
            fields.insert(name.to_ascii_lowercase().replace(' ', "_"), json!(value));
        }
    }
    if let Some(bytes) = html
        .select(&selector("abbr[title]"))
        .filter_map(|e| e.value().attr("title"))
        .filter(|text| text.to_ascii_lowercase().contains("bytes"))
        .find_map(|text| text.split_whitespace().next()?.parse::<u64>().ok())
    {
        fields.insert("size_bytes".into(), json!(bytes));
    }
    let metadata:Vec<_>=html.select(&selector("meta[content]")).map(|e|json!({"name":e.value().attr("name").or_else(||e.value().attr("property")),"content":e.value().attr("content")})).collect();
    let headings:Vec<_>=html.select(&selector("h1,h2,h3,h4,h5,h6")).map(|e|json!({"level":e.value().name(),"text":e.text().collect::<Vec<_>>().join(" ").trim()})).collect();
    let links:Vec<_>=html.select(&selector("a[href]")).map(|e|json!({"url":absolute(e.value().attr("href").unwrap_or("")),"text":e.text().collect::<Vec<_>>().join(" ").trim()})).collect();
    Page {
        challenge,
        status,
        details: json!({"url":url,"status":status,"state":if challenge {"challenge"} else if (200..300).contains(&status) {"fetched"} else {"http_error"},"title":title,"headers":headers,"forms":forms,"labelled_details":fields,"file_details":fields,"table_rows":rows,"metadata":metadata,"headings":headings,"links":links,"assets":assets,"text":text,"javascript_executed":false}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_animepahe_page_details() {
        let page = inspect(
            "https://animepahe.ru/",
            200,
            &[],
            r#"<title>AnimePahe</title><meta name="description" content="Anime catalogue"><h1>Latest episodes</h1><a href="/series/example">A series</a><p><strong>Category</strong>: Anime</p>"#,
        );
        assert!(!page.challenge);
        assert_eq!(page.details["state"], "fetched");
        assert_eq!(
            page.details["links"][0]["url"],
            "https://animepahe.ru/series/example"
        );
        assert_eq!(page.details["metadata"][0]["content"], "Anime catalogue");
        assert_eq!(page.details["labelled_details"]["category"], "Anime");
        assert!(
            inspect(
                "https://animepahe.ru/",
                403,
                &[("cf-mitigated".into(), "challenge".into())],
                "<title>Just a moment...</title>"
            )
            .challenge
        );
        assert!(
            !inspect(
                "https://animepahe.ru/",
                200,
                &[],
                "<title>AnimePahe</title><div class='h-captcha' hidden></div>"
            )
            .challenge
        );
    }
    #[test]
    fn extracts_kwik_file_metadata_from_captured_markup() {
        let page = inspect(
            "https://kwik.cx/f/test",
            200,
            &[],
            r#"<title>fixture.mp4 :: Kwik</title><div id="app"><div class="column"><strong>File name</strong>: fixture.mp4</div><div class="column"><strong>Size</strong>: <abbr title="42914847 Bytes">40.93 MB</abbr></div><div class="column"><p><strong>MD5</strong>: <code>fixture-hash</code></p></div></div>"#,
        );
        assert_eq!(page.details["file_details"]["file_name"], "fixture.mp4");
        assert_eq!(page.details["file_details"]["size_bytes"], 42914847);
        assert_eq!(page.details["file_details"]["md5"], "fixture-hash");
    }
    #[test]
    fn distinguishes_challenge_response_from_normal_cloudflare_scripts() {
        let header = vec![("cf-mitigated".into(), "challenge".into())];
        assert!(
            inspect(
                "https://kwik.cx/f/test",
                403,
                &header,
                "<title>Loading</title>"
            )
            .challenge
        );
        assert!(
            inspect(
                "https://kwik.cx/f/test",
                403,
                &[],
                "<title>Just a moment...</title>"
            )
            .challenge
        );
        assert!(
            !inspect(
                "https://kwik.cx/f/test",
                403,
                &[],
                "<title>Forbidden</title>"
            )
            .challenge
        );
        let page=inspect("https://kwik.cx/f/test",200,&[],"<title>file.mp4 :: Kwik</title><form method='POST' action='/d/test'><input name='_token' value='fixture'></form><script src='/cdn-cgi/challenge-platform/scripts/jsd/main.js'></script>");
        assert!(!page.challenge);
        assert_eq!(page.details["forms"][0]["action"], "https://kwik.cx/d/test");
        assert_eq!(page.details["forms"][0]["inputs"][0]["value"], "fixture");
        assert_eq!(final_headers("HTTP/1.1 302 Found\r\nLocation: /next\r\n\r\nHTTP/2 200\r\nX-Result: final\r\n\r\n"),vec![("X-Result".into(),"final".into())]);
    }

    #[test]
    fn http_fetch_reuses_saved_agent_and_cookies_without_a_display() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let root = std::env::temp_dir().join(format!("webkit-http-test-{}", std::process::id()));
        let session = Session::open(&root).unwrap();
        private_write(
            &session.dir.join("cookies.txt"),
            b"# Netscape HTTP Cookie File\n127.0.0.1\tFALSE\t/\tFALSE\t0\tsession\treusable\n",
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = vec![0; 16384];
            let n = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]).to_string();
            let body="<title>HTTP fixture</title><table><tr><td>Name</td><td>fixture.mp4</td></tr></table>";
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nSet-Cookie: renewed=yes; Path=/; HttpOnly\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            request
        });
        let capture = Capture::new(true).unwrap();
        let page = fetch(
            &format!("http://{address}/"),
            &capture,
            &session,
            "http-test",
        )
        .unwrap();
        let request = server.join().unwrap();
        assert!(request.contains(&format!("User-Agent: {}", session.user_agent)));
        assert!(request.contains("session=reusable"));
        assert_eq!(page.status, 200);
        assert!(!page.challenge);
        assert_eq!(page.details["table_rows"][0][1], "fixture.mp4");
        assert!(fs::read_to_string(session.dir.join("cookies.txt"))
            .unwrap()
            .contains("renewed\tyes"));
        assert!(
            fs::read_to_string(capture.dir.join("http-test/headers.txt"))
                .unwrap()
                .contains("Set-Cookie")
        );
        assert!(capture.dir.join("http-test/trace.txt").exists());
        let capture_dir = capture.dir.clone();
        drop(capture);
        drop(session);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(capture_dir).unwrap();
    }
}
