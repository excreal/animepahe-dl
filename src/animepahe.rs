use std::{
    fs,
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    rc::Rc,
    sync::mpsc,
    thread,
    time::Duration,
};

use scraper::{Html, Selector};
use serde_json::{json, Value};
use url::Url;

use crate::{
    capture::Capture,
    http,
    session::{private_write, Session},
};

const DEFAULT_HOST: &str = "https://animepahe.pw";
const PLAYLIST_HOST: &str = "https://glot.io/api/mux";
const MEDIA_REFERER: &str = "https://kwik.cx/";

/// Add series episode counts and play-page audio options to generic page details.
pub fn enrich_play_page(
    url: &str,
    http_label: &str,
    capture: Rc<Capture>,
    session: &mut Session,
    details: &mut Value,
) -> Result<bool, Box<dyn std::error::Error>> {
    let parsed = Url::parse(url)?;
    let Some(segments) = parsed
        .path_segments()
        .map(|items| items.collect::<Vec<_>>())
    else {
        return Ok(false);
    };
    if parsed
        .host_str()
        .is_none_or(|host| host != "animepahe.pw" && !host.ends_with(".animepahe.pw"))
        || segments.len() < 3
        || segments[0] != "play"
    {
        return Ok(false);
    }
    let anime_id = segments[1];
    let page_body = fs::read(capture.dir.join(http_label).join("body.html"))?;
    let languages = audio_languages(&String::from_utf8_lossy(&page_body));
    let host = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap());
    let mut network = Network {
        session,
        capture: capture.clone(),
        debug: false,
    };
    let source = fetch_episodes(&mut network, &host, anime_id)?;
    let count = source["data"].as_array().map_or(0, Vec::len);
    let episode = episode_label(&String::from_utf8_lossy(&page_body), url);
    details["animepahe"] = json!({
        "anime_id": anime_id,
        "episode_count": count,
        "current_episode": episode,
        "audio_languages": languages,
        "english_dub": languages.iter().any(|value| value == "English"),
    });
    private_write(
        &capture.dir.join(http_label).join("page-details.json"),
        serde_json::to_vec_pretty(details)?.as_slice(),
    )?;
    private_write(
        &capture.dir.join("page-details.json"),
        serde_json::to_vec_pretty(details)?.as_slice(),
    )?;
    capture.event(
        "animepahe_info",
        json!({"anime_id":anime_id,"episode_count":count,"current_episode":episode,"audio_languages":languages}),
    );
    println!("AnimePahe: {} episode(s) available", count);
    if let Some(episode) = episode {
        println!("Current page: episode {episode}");
    }
    if languages.is_empty() {
        println!("Audio languages: none found in the page source");
    } else {
        println!("Audio languages: {}", languages.join(", "));
        println!(
            "English dub: {}",
            if languages.iter().any(|value| value == "English") {
                "available"
            } else {
                "not listed"
            }
        );
    }
    Ok(true)
}

fn audio_languages(html: &str) -> Vec<String> {
    let document = Html::parse_document(html);
    let mut languages = std::collections::BTreeSet::new();
    for button in document.select(&next_selector("button[data-src][data-audio]")) {
        if button.value().attr("data-av1") == Some("1") {
            continue;
        }
        let Some(audio) = button.value().attr("data-audio") else {
            continue;
        };
        if button
            .value()
            .attr("data-src")
            .is_some_and(|url| url.contains("kwik"))
        {
            languages.insert(language_name(audio));
        }
    }
    languages.into_iter().collect()
}

fn language_name(code: &str) -> String {
    match code.trim().to_ascii_lowercase().as_str() {
        "eng" | "en" | "english" => "English".to_owned(),
        "jpn" | "ja" | "jp" | "japanese" => "Japanese".to_owned(),
        "kor" | "ko" | "korean" => "Korean".to_owned(),
        "spa" | "es" | "spanish" => "Spanish".to_owned(),
        "por" | "pt" | "portuguese" => "Portuguese".to_owned(),
        "ger" | "de" | "german" => "German".to_owned(),
        "fre" | "fr" | "french" => "French".to_owned(),
        other => other.to_owned(),
    }
}

fn episode_label(html: &str, url: &str) -> Option<String> {
    let target = Url::parse(url).ok()?;
    let document = Html::parse_document(html);
    document
        .select(&next_selector("a[href]"))
        .find(|anchor| {
            anchor
                .value()
                .attr("href")
                .is_some_and(|href| target.join(href).is_ok_and(|candidate| candidate == target))
        })
        .map(|anchor| anchor.text().collect::<String>().trim().to_owned())
        .filter(|label| !label.is_empty())
        .and_then(|label| label.split_whitespace().last().map(str::to_owned))
}

#[derive(Default)]
struct Options {
    name: Option<String>,
    slug: Option<String>,
    episodes: Option<String>,
    resolution: Option<String>,
    language: Option<String>,
    links_only: bool,
    debug: bool,
    force: bool,
}

struct Network<'a> {
    session: &'a mut Session,
    capture: Rc<Capture>,
    debug: bool,
}

struct Response {
    request_id: u64,
    status: u16,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}

impl Network<'_> {
    fn request(
        &mut self,
        method: &str,
        url: &str,
        referer: &str,
        body: Option<&[u8]>,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        for attempt in 0..=1 {
            let response = self.raw_request(method, url, referer, body)?;
            let content = String::from_utf8_lossy(&response.body);
            let saved_headers = fs::read_to_string(self.capture.dir.join(format!(
                "animepahe/requests/{}/headers.txt",
                response.request_id
            )))?;
            let is_challenge = http::challenge_header_present(&saved_headers)
                || http::inspect(url, response.status, &response.headers, &content).challenge;
            self.capture.event("animepahe_response",json!({"url":url,"status":response.status,"challenge":is_challenge,"body_bytes":response.body.len()}));
            if !is_challenge {
                return Ok(response);
            }
            if attempt > 0 {
                let message = if self.capture.persistent() {
                    format!("CAPTCHA persisted at {url} after manual verification; response saved in {}", self.capture.dir.display())
                } else {
                    format!("CAPTCHA persisted at {url} after manual verification; rerun with --capture to save diagnostics")
                };
                return Err(message.into());
            }
            eprintln!("CAPTCHA detected during {method} request to {url}; opening the page for manual verification.");
            self.solve_challenge(url)?;
        }
        unreachable!()
    }

    fn solve_challenge(&mut self, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        gtk::init()
            .map_err(|e| format!("A graphical session is required to solve this CAPTCHA: {e}"))?;
        if !crate::run_browser(self.capture.clone(), self.session, url)? {
            return Err("CAPTCHA browser closed before the page finished loading".into());
        }
        self.session.import_capture(&self.capture.dir)?;
        Ok(())
    }

    fn raw_request(
        &mut self,
        method: &str,
        url: &str,
        referer: &str,
        body: Option<&[u8]>,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let id = self.capture.id();
        let relative = format!("animepahe/requests/{id}");
        fs::create_dir_all(self.capture.dir.join(&relative))?;
        let mut command = Command::new("curl");
        command
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
                "20",
                "--max-time",
                "120",
                "--compressed",
            ])
            .arg("--trace-time")
            .arg("--trace-ascii")
            .arg(self.capture.dir.join(&relative).join("trace.txt"))
            .arg("--dump-header")
            .arg(self.capture.dir.join(&relative).join("headers.txt"))
            .arg("--output")
            .arg(self.capture.dir.join(&relative).join("body.bin"))
            .arg("--cookie")
            .arg(self.session.dir.join("cookies.txt"))
            .arg("--cookie-jar")
            .arg(self.session.dir.join("cookies.txt"))
            .arg("--user-agent")
            .arg(&self.session.user_agent)
            .arg("--referer")
            .arg(referer)
            .arg("--request")
            .arg(method)
            .arg("--url")
            .arg(url);
        if body.is_some() {
            command
                .arg("--header")
                .arg("Content-Type: application/json")
                .arg("--data-binary")
                .arg("@-")
                .stdin(Stdio::piped());
        }
        if self.debug {
            eprintln!("[debug] {method} {url} (request {id})");
        }
        let mut child = command.spawn()?;
        if let Some(bytes) = body {
            child
                .stdin
                .take()
                .ok_or("curl stdin unavailable")?
                .write_all(bytes)?;
        }
        let result = child.wait_with_output()?;
        if !result.status.success() {
            private_write(
                &self.capture.dir.join(&relative).join("error.txt"),
                &result.stderr,
            )?;
            self.capture.event(
                "animepahe_request_error",
                json!({"url":url,"error":String::from_utf8_lossy(&result.stderr)}),
            );
            return Err(format!(
                "Request failed for {url}: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            )
            .into());
        }
        let raw_headers = fs::read_to_string(self.capture.dir.join(&relative).join("headers.txt"))?;
        let status =
            final_status(&raw_headers).ok_or("Could not read HTTP status from response")?;
        let headers = http::final_headers(&raw_headers);
        let bytes = fs::read(self.capture.dir.join(&relative).join("body.bin"))?;
        self.capture.event("animepahe_request",json!({"id":id,"method":method,"url":url,"status":status,"headers":headers,"challenge_header":http::challenge_header_present(&raw_headers),"body_file":format!("{relative}/body.bin"),"body_bytes":bytes.len()}));
        if self.debug {
            eprintln!("[debug] HTTP {status} {} bytes", bytes.len());
        }
        Ok(Response {
            request_id: id,
            status,
            body: bytes,
            headers,
        })
    }
}

fn final_status(raw: &str) -> Option<u16> {
    raw.replace("\r\n", "\n")
        .split("\n\n")
        .filter(|block| block.starts_with("HTTP/"))
        .last()?
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

pub fn run(
    args: &[String],
    capture: Rc<Capture>,
    session: &mut Session,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_args(args)?;
    if args.iter().any(|x| x == "-h" || x == "--help") {
        print_help();
        return Ok(());
    }
    check_dependencies(&options)?;
    let host = std::env::var("ANIMEPAHE_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_owned());
    let host = host.trim_end_matches('/').to_owned();
    let mut network = Network {
        session,
        capture: capture.clone(),
        debug: options.debug,
    };
    if let Ok(bytes) = fs::read("config.json") {
        let config: Value = serde_json::from_slice(&bytes)?;
        if let Some(ua) = config["ua"].as_str().filter(|ua| !ua.trim().is_empty()) {
            network.session.set_user_agent(ua)?;
        }
        if let Some(clearance) = config["cf"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
        {
            if !fs::read_to_string(network.session.dir.join("cookies.txt"))?
                .contains("\tcf_clearance\t")
            {
                let expires = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs()
                    + 1800;
                let mut jar = fs::read_to_string(network.session.dir.join("cookies.txt"))?;
                jar.push_str(&format!("#HttpOnly_animepahe.pw\tFALSE\t/\tTRUE\t{expires}\tcf_clearance\t{clearance}\n"));
                private_write(&network.session.dir.join("cookies.txt"), jar.as_bytes())?;
            }
        }
    }
    let anime_list = PathBuf::from("anime.list");
    let catalogue = if let Some(name) = options.name.as_deref() {
        let mut url = Url::parse(&format!("{host}/api"))?;
        url.query_pairs_mut()
            .append_pair("m", "search")
            .append_pair("q", name);
        let response = network.request("GET", url.as_str(), &format!("{host}/"), None)?;
        ensure_success(&response)?;
        let data: Value = serde_json::from_slice(&response.body)?;
        if data["total"].as_u64().unwrap_or(0) == 0 {
            return Err("No anime matched that search".into());
        }
        let choices: Vec<_> = data["data"]
            .as_array()
            .ok_or("Search response has no data array")?
            .iter()
            .filter_map(|entry| {
                Some((
                    entry["session"].as_str()?.to_owned(),
                    entry["title"].as_str()?.to_owned(),
                    release_year(&entry["year"]),
                ))
            })
            .collect();
        choose_search_result(&choices)?
    } else {
        let choices = if anime_list.exists() {
            read_catalogue(&anime_list)?
        } else {
            fetch_catalogue(&mut network, &host, &anime_list)?
        };
        if let Some(slug) = options.slug.as_deref() {
            choices
                .into_iter()
                .find(|item| item.0 == slug)
                .ok_or("Anime slug not found in anime.list")?
        } else {
            choose_anime(&choices, None)?
        }
    };
    let anime_name = safe_name(&catalogue.1);
    let directory = Path::new(&anime_name);
    fs::create_dir_all(directory)?;
    let source_path = directory.join(".source.json");
    let source = fetch_episodes(&mut network, &host, &catalogue.0)?;
    private_write(&source_path, &serde_json::to_vec_pretty(&source)?)?;
    let episodes = select_episodes(&source, options.episodes.as_deref())?;
    for episode in episodes {
        let destination = directory.join(format!("{episode}.mp4"));
        if !options.links_only && !options.force {
            let output_path = absolute_path(&destination)?;
            if output_path.exists() {
                if valid_media(&output_path)? {
                    cleanup_download_artifacts(&output_path)?;
                }
                println!("Episode {episode} exists; skipped. Use --force to replace it.");
                continue;
            }
        }
        let episode_page = episode_page_for(&source, &episode)?;
        let play_url = format!("{host}/play/{}/{}", catalogue.0, episode_page);
        let html = network.request("GET", &play_url, &format!("{host}/"), None)?;
        ensure_success(&html)?;
        let source_url = choose_source(
            &String::from_utf8_lossy(&html.body),
            options.resolution.as_deref(),
            options.language.as_deref(),
        )
        .ok_or_else(|| format!("No Kwik source found for episode {episode}"))?;
        let playlist = resolve_playlist(&mut network, &source_url, &format!("{host}/"))?;
        if options.links_only {
            println!("{playlist}");
            continue;
        }
        eprintln!("Downloading episode {episode}...");
        io::stderr().flush()?;
        download_video(
            &mut network,
            &playlist,
            &destination,
            MEDIA_REFERER,
            options.force,
        )?;
        println!("Downloaded episode {episode}: {}", destination.display());
    }
    Ok(())
}

fn parse_args(args: &[String]) -> Result<Options, Box<dyn std::error::Error>> {
    let mut output = Options::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-h" | "--help" => {}
            "-l" => output.links_only = true,
            "-d" => output.debug = true,
            "-f" | "--force" => output.force = true,
            "-a" | "--name" => output.name = Some(next(args, &mut i, arg)?),
            "-s" | "--slug" => output.slug = Some(next(args, &mut i, arg)?),
            "-e" | "--episodes" => output.episodes = Some(next(args, &mut i, arg)?),
            "-r" | "--resolution" => output.resolution = Some(next(args, &mut i, arg)?),
            "-o" | "--language" => output.language = Some(next(args, &mut i, arg)?),
            _ => return Err(format!("Unknown animepahe option: {arg}").into()),
        }
        i += 1;
    }
    Ok(output)
}

fn next(
    args: &[String],
    i: &mut usize,
    option: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    *i += 1;
    args.get(*i)
        .cloned()
        .filter(|x| !x.starts_with('-'))
        .ok_or_else(|| format!("{option} requires a value").into())
}

fn print_help() {
    println!("AnimePahe downloader\n\nUsage: cargo run -- animepahe [--capture] [-a name | -s slug] [-e 1,3-5,*] [-r 1080] [-o eng] [-l] [-f] [-d]\n\n --capture     save request and browser diagnostics under captures/\n -a NAME       search anime title\n -s SLUG       select an anime session/slug\n -e EPISODES   comma-separated episode numbers, ranges, or *\n -r HEIGHT     requested resolution, default 1080p\n -o LANGUAGE   audio language\n -l            print playlist URL without downloading\n -f, --force   replace existing episode files\n -d            log request URLs and statuses\n -h, --help    show this help\n\nThe animepahe-dl.sh options are also accepted directly, e.g. cargo run -- -a title.");
}

fn check_dependencies(options: &Options) -> Result<(), Box<dyn std::error::Error>> {
    let mut available = Vec::new();
    let mut missing = Vec::new();
    check_program("curl", &["--version"], &mut available, &mut missing);
    if options.name.is_some() || options.slug.is_none() {
        check_program("fzf", &["--version"], &mut available, &mut missing);
    }
    if !options.links_only {
        check_program("yt-dlp", &["--version"], &mut available, &mut missing);
        check_program("python3", &["--version"], &mut available, &mut missing);
        check_program("ffmpeg", &["-version"], &mut available, &mut missing);
        check_program("ffprobe", &["-version"], &mut available, &mut missing);
        match Command::new("python3")
            .args([
                "-c",
                "from yt_dlp.dependencies import Cryptodome; assert Cryptodome.AES",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) if status.success() => available.push("python-pycryptodomex".to_owned()),
            _ => missing.push(
                "python-pycryptodomex (Arch: sudo pacman -S python-pycryptodomex)".to_owned(),
            ),
        }
    }
    if !missing.is_empty() {
        return Err(format!("Missing required dependencies: {}", missing.join(", ")).into());
    }
    println!("Dependency check passed.");
    Ok(())
}

fn check_program(
    program: &str,
    args: &[&str],
    available: &mut Vec<String>,
    missing: &mut Vec<String>,
) {
    match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => available.push(program.to_owned()),
        _ => missing.push(program.to_owned()),
    }
}

fn next_selector(value: &str) -> Selector {
    Selector::parse(value).expect("fixed AnimePahe selector")
}

fn fetch_catalogue(
    network: &mut Network<'_>,
    host: &str,
    path: &Path,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let response = network.request("GET", &format!("{host}/anime"), &format!("{host}/"), None)?;
    ensure_success(&response)?;
    let html = Html::parse_document(&String::from_utf8_lossy(&response.body));
    let mut choices = std::collections::BTreeMap::new();
    for anchor in html.select(&next_selector("a[href*='/anime/'][title]")) {
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let slug = href
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("");
        let title = anchor.value().attr("title").unwrap_or("").trim();
        if !slug.is_empty() && !title.is_empty() {
            choices.insert(slug.to_owned(), title.to_owned());
        }
    }
    let choices: Vec<_> = choices.into_iter().collect();
    if choices.is_empty() {
        return Err("Anime catalog contained no anime links".into());
    }
    private_write(
        path,
        choices
            .iter()
            .map(|(slug, title)| format!("[{slug}] {title}\n"))
            .collect::<String>()
            .as_bytes(),
    )?;
    Ok(choices)
}

fn read_catalogue(path: &Path) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    fs::read_to_string(path)?
        .lines()
        .filter_map(|line| {
            let close = line.find("] ")?;
            Some((
                line.get(1..close)?.to_owned(),
                line[close + 2..].trim().to_owned(),
            ))
        })
        .collect::<Vec<_>>()
        .pipe(Ok)
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

fn choose_anime(
    choices: &[(String, String)],
    slug: Option<&str>,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    if let Some(slug) = slug {
        return choices
            .iter()
            .find(|choice| choice.0 == slug)
            .cloned()
            .ok_or_else(|| format!("Anime slug not found: {slug}").into());
    }
    let input = choices
        .iter()
        .map(|(slug, title)| format!("[{slug}] {title}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut child = Command::new("fzf")
        .args(["--select-1", "--exit-0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("fzf stdin unavailable")?
        .write_all(input.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err("Anime selection cancelled".into());
    }
    let line = String::from_utf8(output.stdout)?;
    let close = line.find("] ").ok_or("Could not parse fzf selection")?;
    let slug = line[1..close].to_owned();
    let title = line[close + 2..].trim().to_owned();
    Ok((slug, title))
}

fn choose_search_result(
    choices: &[(String, String, Option<i32>)],
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let input = choices
        .iter()
        .map(|(slug, title, year)| {
            let year = year.map_or_else(|| "year unknown".to_owned(), |year| year.to_string());
            format!("[{slug}] {year} — {title}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut child = Command::new("fzf")
        .args(["--select-1", "--exit-0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("fzf stdin unavailable")?
        .write_all(input.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err("Anime selection cancelled".into());
    }
    let line = String::from_utf8(output.stdout)?;
    let close = line.find("] ").ok_or("Could not parse fzf selection")?;
    let slug = &line[1..close];
    choices
        .iter()
        .find(|choice| choice.0 == slug)
        .map(|(slug, title, _)| (slug.clone(), title.clone()))
        .ok_or_else(|| "Selected anime was not in the search results".into())
}

fn release_year(value: &Value) -> Option<i32> {
    value
        .as_i64()
        .and_then(|year| i32::try_from(year).ok())
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn fetch_episodes(
    network: &mut Network<'_>,
    host: &str,
    slug: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let get_page =
        |network: &mut Network<'_>, page: u64| -> Result<Value, Box<dyn std::error::Error>> {
            let mut url = Url::parse(&format!("{host}/api"))?;
            url.query_pairs_mut()
                .append_pair("m", "release")
                .append_pair("id", slug)
                .append_pair("sort", "episode_asc")
                .append_pair("page", &page.to_string());
            let response =
                network.request("GET", url.as_str(), &format!("{host}/anime/{slug}"), None)?;
            ensure_success(&response)?;
            Ok(serde_json::from_slice(&response.body)?)
        };
    let first = get_page(network, 1)?;
    let last = first["last_page"]
        .as_u64()
        .or_else(|| first["last_page"].as_str()?.parse().ok())
        .filter(|page| *page > 0)
        .ok_or("Episode response has no last_page")?;
    let mut episodes = first["data"]
        .as_array()
        .cloned()
        .ok_or("Episode response has no data array")?;
    for page in 2..=last {
        let data = get_page(network, page)?;
        episodes.extend(
            data["data"]
                .as_array()
                .cloned()
                .ok_or("Episode page has no data array")?,
        );
    }
    Ok(json!({"data":episodes}))
}

fn select_episodes(
    source: &Value,
    expression: Option<&str>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let available: std::collections::BTreeSet<u64> = source["data"]
        .as_array()
        .ok_or("No episode data")?
        .iter()
        .filter_map(|e| {
            e["episode"]
                .as_u64()
                .or_else(|| e["episode"].as_str()?.parse().ok())
        })
        .collect();
    if available.is_empty() {
        return Err("No episodes are available".into());
    }
    let Some(expression) = expression else {
        for episode in &available {
            eprintln!("Episode {episode}");
        }
        eprint!("Which episode(s) to download: ");
        io::stderr().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        return select_episodes(source, Some(answer.trim()));
    };
    let mut selected = std::collections::BTreeSet::new();
    for item in expression
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if item == "*" {
            selected.extend(available.iter().copied());
            continue;
        }
        if let Some((start, end)) = item.split_once('-') {
            let start: u64 = start.parse()?;
            let end: u64 = end.parse()?;
            if start > end {
                return Err(format!("Invalid episode range: {item}").into());
            }
            selected.extend(available.range(start..=end).copied());
        } else {
            let episode: u64 = item.parse()?;
            if available.contains(&episode) {
                selected.insert(episode);
            } else {
                eprintln!("Episode {episode} not found; skipping.");
            }
        }
    }
    if selected.is_empty() {
        return Err("No requested episodes are available".into());
    }
    Ok(selected.into_iter().map(|n| n.to_string()).collect())
}

fn episode_page_for(source: &Value, episode: &str) -> Result<String, Box<dyn std::error::Error>> {
    source["data"]
        .as_array()
        .ok_or("No episode data")?
        .iter()
        .find(|row| {
            row["episode"]
                .as_u64()
                .map(|n| n.to_string())
                .or_else(|| row["episode"].as_str().map(str::to_owned))
                .as_deref()
                == Some(episode)
        })
        .and_then(|row| row["session"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("Episode {episode} has no session id").into())
}

fn choose_source(html: &str, resolution: Option<&str>, language: Option<&str>) -> Option<String> {
    let document = Html::parse_document(html);
    let mut sources = vec![];
    for button in document.select(&next_selector("button[data-src]")) {
        if button.value().attr("data-av1") == Some("1") {
            continue;
        }
        let href = button.value().attr("data-src")?;
        if !href.to_ascii_lowercase().contains("kwik") {
            continue;
        }
        let resolution_value = button
            .value()
            .attr("data-resolution")
            .unwrap_or("0")
            .trim_end_matches('p')
            .parse::<u32>()
            .unwrap_or(0);
        sources.push((
            href.to_owned(),
            resolution_value,
            button.value().attr("data-audio").unwrap_or("").to_owned(),
        ));
    }
    if sources.is_empty() {
        return None;
    }
    let filtered = |height: Option<u32>, audio: Option<&str>| {
        sources
            .iter()
            .filter(|source| {
                height.is_none_or(|h| source.1 == h)
                    && audio.is_none_or(|a| {
                        language_name(&source.2).eq_ignore_ascii_case(&language_name(a))
                    })
            })
            .max_by_key(|source| source.1)
            .map(|source| source.0.clone())
    };
    let auto_english = language.is_none()
        && sources
            .iter()
            .any(|source| language_name(&source.2).eq_ignore_ascii_case("English"));
    let preferred_language = language.or(auto_english.then_some("eng"));
    let selected = filtered(
        resolution.and_then(|r| r.trim_end_matches('p').parse().ok()),
        preferred_language,
    );
    if auto_english {
        return selected
            .or_else(|| filtered(None, preferred_language))
            .or_else(|| {
                filtered(
                    resolution.and_then(|r| r.trim_end_matches('p').parse().ok()),
                    None,
                )
            })
            .or_else(|| filtered(None, None));
    }
    selected
        .or_else(|| {
            filtered(
                resolution.and_then(|r| r.trim_end_matches('p').parse().ok()),
                None,
            )
        })
        .or_else(|| filtered(None, language))
        .or_else(|| filtered(None, None))
}

fn resolve_playlist(
    network: &mut Network<'_>,
    source_url: &str,
    referer: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let response = network.request("GET", source_url, referer, None)?;
    ensure_success(&response)?;
    let html = Html::parse_document(&String::from_utf8_lossy(&response.body));
    for script in html.select(&next_selector("script")) {
        let source = script.text().collect::<String>();
        if !source.contains("eval") {
            continue;
        }
        let request = json!({"action":"run","data":{"image":"glot/javascript:latest","payload":{"runInstructions":{"buildCommands":[],"runCommand":"node main.js"},"files":[{"name":"main.js","content":source}],"stdin":null}}});
        let encoded = serde_json::to_vec(&request)?;
        let resolved =
            network.request("POST", PLAYLIST_HOST, "https://glot.io/", Some(&encoded))?;
        ensure_success(&resolved)?;
        if let Some(url) = find_playlist(&resolved.body) {
            return Ok(url);
        }
    }
    Err("Could not extract an m3u8 playlist from the player response".into())
}

fn find_playlist(bytes: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    fn strings(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::String(text) => out.push(text.clone()),
            Value::Array(items) => {
                for item in items {
                    strings(item, out)
                }
            }
            Value::Object(items) => {
                for item in items.values() {
                    strings(item, out)
                }
            }
            _ => {}
        }
    }
    let mut candidates = vec![String::from_utf8_lossy(bytes).into_owned()];
    strings(&value, &mut candidates);
    for text in candidates {
        let mut offset = 0;
        while let Some(start) = text[offset..].find("https://") {
            let absolute = offset + start;
            let remaining = &text[absolute..];
            if let Some(end) = remaining.find(".m3u8") {
                let end = end + 6;
                let value = remaining[..end]
                    .trim_matches(|c: char| !c.is_ascii_alphanumeric() && !":/?&=._%-".contains(c));
                if value.starts_with("https://") {
                    return Some(value.to_owned());
                }
                offset = absolute + end;
            } else {
                break;
            }
        }
    }
    None
}

fn download_video(
    network: &mut Network<'_>,
    playlist: &str,
    destination: &Path,
    referer: &str,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let output_path = absolute_path(destination)?;
    if force {
        cleanup_download_artifacts(&output_path)?;
    }
    for attempt in 0..=1 {
        let episode = destination
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("episode");
        let output_dir = network
            .capture
            .dir
            .join("animepahe")
            .join("yt-dlp")
            .join(episode);
        fs::create_dir_all(&output_dir)?;
        let log = output_dir.join(format!("{episode}.log"));
        network.capture.event(
            "yt_dlp_start",
            json!({
                "playlist": playlist,
                "referer": referer,
                "impersonation": "chrome",
                "cookies": "session cookie jar"
            }),
        );
        let mut command = Command::new("yt-dlp");
        command
            .arg("--cookies")
            .arg(network.session.dir.join("cookies.txt"))
            .arg("--referer")
            .arg(referer)
            .args([
                "--write-pages",
                "--print-traffic",
                "--impersonate",
                "chrome",
                "--downloader",
                "m3u8:native",
                "--concurrent-fragments",
                "8",
                "--remux-video",
                "mp4",
                "--no-warnings",
                "--newline",
            ])
            .arg("--output")
            .arg(&output_path)
            .arg(playlist)
            .current_dir(&output_dir);
        if force {
            command.arg("--force-overwrites");
        }
        let output = run_with_progress(&mut command)?;
        let combined = [output.stdout.as_slice(), output.stderr.as_slice()].concat();
        private_write(&log, &combined)?;
        if output.status.success() {
            if valid_media(&output_path)? {
                cleanup_download_artifacts(&output_path)?;
                return Ok(());
            }
            return Err(format!(
                "yt-dlp finished, but ffprobe could not validate {}. See {}",
                output_path.display(),
                log.display()
            )
            .into());
        }
        let mut dumped_challenge = false;
        for entry in fs::read_dir(&output_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() || entry.path() == log {
                continue;
            }
            let bytes = fs::read(entry.path())?;
            let body = String::from_utf8_lossy(&bytes);
            let challenge = http::inspect(playlist, 403, &[], &body).challenge;
            network.capture.event("yt_dlp_page_dump", json!({"file":entry.path().strip_prefix(&network.capture.dir).unwrap_or(&entry.path()).to_string_lossy(),"bytes":bytes.len(),"challenge":challenge}));
            dumped_challenge |= challenge;
        }
        if dumped_challenge {
            if attempt == 0 {
                eprintln!(
                    "yt-dlp captured a CAPTCHA response; opening WebKit for manual verification."
                );
                network.solve_challenge(playlist)?;
                continue;
            }
            return Err(format!(
                "yt-dlp still received a CAPTCHA response. See {}",
                log.display()
            )
            .into());
        }
        if hls_manifest_uses_aes(&output_dir)? && !native_hls_aes_available() {
            network.capture.event(
                "yt_dlp_error",
                json!({
                    "playlist": playlist,
                    "reason": "aes128_without_pycryptodomex",
                    "log": log.strip_prefix(&network.capture.dir).unwrap_or(&log).to_string_lossy()
                }),
            );
            return Err(format!(
                "This HLS playlist uses AES-128, but yt-dlp's native AES support is missing. Install pycryptodomex (Arch: sudo pacman -S python-pycryptodomex), then retry. Without it, yt-dlp delegates the network read to ffmpeg, which received HTTP 403. See {}",
                log.display()
            )
            .into());
        }
        network.capture.event(
            "yt_dlp_error",
            json!({
                "playlist": playlist,
                "exit": output.status.code(),
                "log": log.strip_prefix(&network.capture.dir).unwrap_or(&log).to_string_lossy(),
                "hls_downloader": "native"
            }),
        );
        return Err(format!(
            "yt-dlp failed while downloading or remuxing the HLS media. See {}",
            log.display()
        )
        .into());
    }
    unreachable!()
}

fn run_with_progress(command: &mut Command) -> io::Result<Output> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("yt-dlp stdout was not captured"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("yt-dlp stderr was not captured"))?;
    let (sender, receiver) = mpsc::channel();
    let stdout_reader = thread::spawn({
        let sender = sender.clone();
        move || forward_output(stdout, true, sender)
    });
    let stderr_reader = thread::spawn(move || forward_output(stderr, false, sender));
    let mut output_stdout = Vec::new();
    let mut output_stderr = Vec::new();
    let mut status = None;
    let mut progress = ProgressRenderer::new();
    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok((is_stdout, line)) => {
                progress.render(&line);
                let target = if is_stdout {
                    &mut output_stdout
                } else {
                    &mut output_stderr
                };
                target.extend_from_slice(line.as_bytes());
                target.push(b'\n');
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if status.is_none() {
            status = child.try_wait()?;
        }
    }
    let status = match status {
        Some(status) => status,
        None => child.wait()?,
    };
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
    progress.finish();
    Ok(Output {
        status,
        stdout: output_stdout,
        stderr: output_stderr,
    })
}

fn forward_output<R: io::Read + Send + 'static>(
    pipe: R,
    is_stdout: bool,
    sender: mpsc::Sender<(bool, String)>,
) {
    for line in io::BufReader::new(pipe).lines() {
        match line {
            Ok(line) => {
                if sender.send((is_stdout, line)).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

struct ProgressRenderer {
    terminal: bool,
    last_bucket: Option<u8>,
    rendered: bool,
}

impl ProgressRenderer {
    fn new() -> Self {
        Self {
            terminal: io::stderr().is_terminal(),
            last_bucket: None,
            rendered: false,
        }
    }

    fn render(&mut self, line: &str) {
        let Some((percent, mut metrics)) = parse_progress(line) else {
            return;
        };
        if percent >= 100.0 {
            metrics = "download received; verifying/remuxing".to_owned();
        }
        let bucket = (percent.clamp(0.0, 100.0) / 5.0).floor() as u8;
        if !self.terminal && self.last_bucket == Some(bucket) {
            return;
        }
        self.last_bucket = Some(bucket);
        const WIDTH: usize = 30;
        let filled = (percent.clamp(0.0, 100.0) / 100.0 * WIDTH as f32).round() as usize;
        let bar = format!("{}{}", "=".repeat(filled), " ".repeat(WIDTH - filled));
        if self.terminal {
            eprint!("\r[{bar}] {percent:5.1}% {metrics:<32}\x1b[K");
            let _ = io::stderr().flush();
            self.rendered = true;
        } else {
            eprintln!("[{bar}] {percent:5.1}% {metrics}");
        }
    }

    fn finish(&mut self) {
        if self.rendered {
            eprintln!();
            self.rendered = false;
        }
    }
}

fn parse_progress(line: &str) -> Option<(f32, String)> {
    let payload = line.split_once("[download]")?.1.trim_start();
    let (percentage, trailing) = payload.split_once('%')?;
    let percent = percentage.split_whitespace().last()?.parse::<f32>().ok()?;
    if !(0.0..=100.0).contains(&percent) {
        return None;
    }
    let trailing = trailing.trim();
    let eta = trailing
        .find(" ETA ")
        .and_then(|index| trailing[index + 5..].split_whitespace().next())
        .map(|eta| format!("ETA {eta}"))
        .unwrap_or_default();
    Some((percent, eta))
}

fn hls_manifest_uses_aes(directory: &Path) -> io::Result<bool> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|ext| ext == "dump")
        {
            let bytes = fs::read(entry.path())?;
            if String::from_utf8_lossy(&bytes).contains("#EXT-X-KEY:METHOD=AES-128") {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn native_hls_aes_available() -> bool {
    Command::new("python3")
        .args([
            "-c",
            "from yt_dlp.dependencies import Cryptodome; assert Cryptodome and Cryptodome.AES",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn cleanup_download_artifacts(path: &Path) -> io::Result<usize> {
    let Some(filename) = path.file_name().and_then(|value| value.to_str()) else {
        return Ok(0);
    };
    let part_prefix = format!("{filename}.part");
    let state_name = format!("{filename}.ytdl");
    let Some(parent) = path.parent() else {
        return Ok(0);
    };
    let mut removed = 0;
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == state_name || name.starts_with(&part_prefix) {
            match fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(removed)
}

fn valid_media(path: &Path) -> io::Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.len() < 1_000_000 {
        return Ok(false);
    }
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Ok(false);
    }
    let duration = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0);
    Ok(duration.is_finite() && duration > 0.0)
}

fn ensure_success(response: &Response) -> Result<(), Box<dyn std::error::Error>> {
    if !(200..300).contains(&response.status) {
        return Err(format!("HTTP {} from AnimePahe request", response.status).into());
    }
    Ok(())
}

fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, ' ' | '(' | ')' | '+' | '-' | ',') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_owned();
    if cleaned.is_empty() {
        "anime".into()
    } else {
        cleaned
    }
}

#[allow(dead_code)]
fn _capture_type(_: &Capture) {}
