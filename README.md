# animepahe-dl

AnimePahe episode downloader and HTTP-first page inspector with manual WebKit
fallback for CAPTCHA challenges. Running `animepahe-dl` without arguments
prints help and makes no network request.

The generic page inspector requires an explicit URL. It fetches page details
over HTTP and starts WebKit only if the response contains a CAPTCHA/challenge.

```sh
cargo run -- --help
```

```sh
cargo run -- --url https://idrama.dramafren.org/
```

Generic page inspection works as follows:

1. Load `~/.config/animepahe-dl/session/cookies.txt` and
   `~/.config/animepahe-dl/session/user-agent.txt`.
2. Fetch over HTTP using curl. With `--capture`, save response HTML, raw
   headers, traffic trace, and `page-details.json`. Otherwise, temporary
   request data is removed when the command exits.
3. On a normal response, exit without initializing GTK or creating WebKit.
4. On a detected challenge, initialize GTK, restore the browser profile/cookies
   and the same user agent, and show the browser for you to solve it manually.
5. Wait for a complete HTTP(S) document with no detected CAPTCHA and a successful
   HTTP response for two consecutive checks, save requested diagnostics, export
   updated cookies, close the browser, and check whether the session also works
   over HTTP.

An ordinary HTTP error or DNS failure is saved and reported without opening a
browser. A challenge is detected by `cf-mitigated: challenge`, a challenge title,
or an unresolved Turnstile/reCAPTCHA/hCaptcha/challenge widget in the response HTML. A normal Cloudflare
JavaScript Detections script alone is not treated as a CAPTCHA. No hostname,
title suffix, download form, or site-specific DOM layout is required for
completion. Detection recognizes known challenge signals; an unfamiliar custom
CAPTCHA may need another signal added.

Only static HTML is parsed on the HTTP path. Asset URLs are listed but assets
are not downloaded or executed. The captured Kwik page supplies its file name,
size, MIME type, media details, and hashes in HTML. JavaScript builds its download
form, so that form may be absent from browser-free details. The output records
whether JavaScript was executed.

For an AnimePahe `/play/<anime-id>/<episode-id>` URL, the app also retrieves
the series episode API pages and reads audio-language options from the play
page's static source buttons. It prints the episode count, current episode,
available languages, and whether English is listed, and stores them in the
`animepahe` object in `page-details.json`.

## AnimePahe downloader port

The `animepahe-dl.sh` workflow is available as a Rust CLI. It supports title
search or an anime slug, interactive `fzf` selection, episode lists and ranges,
resolution/audio selection, playlist-only output, and yt-dlp downloads:

```sh
cargo run -- animepahe --help
cargo run -- animepahe -a "Anime title" -e '1,3-5' -r 1080 -o eng
cargo run -- -s anime-session-id -e '*' -l
cargo run -- animepahe -s anime-session-id -e 1 --force
cargo run -- animepahe -a "Anime title" -e 1 --capture
```

Existing episode files are skipped; `--force` replaces them.

It uses `~/.config/animepahe-dl/session/user-agent.txt` and
`~/.config/animepahe-dl/session/cookies.txt` for curl requests, including
search, catalogue, episode, play, and Kwik player pages. An optional
`~/.config/animepahe-dl/config.json` with `ua` and `cf` values seeds the saved
user agent and `cf_clearance` cookie. Existing project-local `.session/` and
`config.json` files are migrated on first run. Each curl request saves traces, raw
headers, status metadata, and the response body under
`~/.config/animepahe-dl/captures/<run>/animepahe/requests/`. Detected CAPTCHA responses open WebKit for
manual completion, import the updated session, and retry the request.

Playlist extraction preserves the shell script's `glot.io` JavaScript runner
step. yt-dlp receives the saved cookies, Chrome impersonation (which supplies
the matching Chrome user agent), and the Kwik referrer required by the media
host. Its native HLS downloader carries those request settings to segments;
ffmpeg only remuxes the downloaded local media. A live progress bar is parsed
from yt-dlp's newline output. Its HTTP traffic log and
`--write-pages` response
dumps are saved under `~/.config/animepahe-dl/captures/<run>/animepahe/yt-dlp/<episode>/`; detected
challenge pages there open WebKit and trigger one download retry. The `yt-dlp`
handoff is a separate network client, so those dumps/logs provide its available
diagnostics while curl requests retain the full raw trace and headers.

The first run creates `~/.config/animepahe-dl/config.json` (or the equivalent
under `$XDG_CONFIG_HOME`) from the shipped defaults. Edit it to set the default
audio language, quality, output directory, filename template, fragment
concurrency, AnimePahe host, user agent, or Cloudflare clearance cookie. The
default audio is `original`, selected from the active audio source (or the first
listed source if none is active); `quality` defaults to `best`.

The config keys are `animepahe_host`, `ua`, `cf`, `audio_language`, `quality`,
`output_directory`, `filename_template`, and `concurrent_fragments`. The
filename template supports `{anime}`, `{episode}`, `{episode:02}`, and
`{quality}`. Set `audio_language` to `original` or a code such as `eng`, `jpn`,
or `kor`; set `quality` to `best` or a height such as `720`.

Downloads use the configured filename template. By default, a series folder
contains files named `Anime Name e01 720p.mp4`, with the resolution taken from
the selected source. The filename has no year. CLI `-o` and `-r` options
override the config for one run.

The downloader checks its required commands before making requests: curl,
yt-dlp, Python with yt-dlp's AES support (`python-pycryptodomex`), ffmpeg, and
ffprobe. It checks fzf when interactive selection is needed. Playlist-only
mode skips the download-only dependencies. On Arch Linux, install the crypto
dependency with `sudo pacman -S python-pycryptodomex`.

## Reusable session

The reusable session is stored in `~/.config/animepahe-dl/session/`. Future
runs reuse it automatically. This folder contains:

- `cookies.txt`: shared Netscape cookie jar, including HTTP-only and session cookies.
- `user-agent.txt`: the exact user agent recorded by the browser.
- `session.json`: import/update information.
- `browser-data/`, `browser-cache/`, and `browser-cookies.sqlite`: persistent
  WebKit profile data created when the browser is needed.
- `lock`: prevents simultaneous runs from overwriting this session.

Refreshing cookies for one site preserves saved cookies for other sites and
unvisited paths. Cookies remain scoped to their domains when curl/WebKit send them.

The session directory has permissions `700`, newly written cookie/agent files
have permissions `600`, and the legacy project-local `.session/` is excluded
from Git. Cookies are
imported into WebKit before navigation. Refreshed browser cookies, including
session cookies, are exported for the next HTTP request. Browser local storage
uses the persistent profile; per-tab sessionStorage remains per-tab.

Session reuse is attempted, not guaranteed: the site can expire or reject
clearance. HTTP and WebKit are different clients even with matching cookies
and user agents. If HTTP still receives a challenge after successful manual
verification, the app saves the rendered page and session, reports that HTTP
reuse was not accepted, and stops rather than repeatedly requesting solutions.

Import another successful capture without making a network request:

```sh
cargo run -- --import-session ~/.config/animepahe-dl/captures/<run> --import-only
```

## Options

```sh
# Never launch a browser; return an error when challenged.
cargo run -- --url https://example.org/ --http-only

# Use a separate cookie/profile directory.
cargo run -- --url https://example.org/ --session-dir /tmp/animepahe-session

# Fetch a different site using the same generic detection.
cargo run -- --url https://idrama.dramafren.org/
```

## Captures

Captures are disabled by default. Add `--capture` to a page fetch or downloader
command to save request and browser diagnostics in
`~/.config/animepahe-dl/captures/<timestamp>-<pid>/` (or under
`$XDG_CONFIG_HOME`). Without it, temporary request data is removed at the end
of the run.

- `page-details.json`: latest successful static details, or rendered browser
  details if HTTP reuse is still challenged after manual verification.
- `http/body.html`, `http/headers.txt`, `http/trace.txt`, `http/curl.json`, and
  `http/page-details.json`: HTTP body, raw response headers, outgoing/incoming
  curl traffic, connection metadata, and parsed page details. Redirect headers
  are retained. Network errors are saved to `http/error.txt`.
- `http-after-browser/`: the single HTTP reuse check following manual verification.
- `events.jsonl`: timestamped HTTP/browser events, checks, errors, headers, and
  resource/body/cookie file references.
- When WebKit runs, `resources/*.bin`, `page-*.json`, `cookies*.json`, and
  `page.mhtml` contain available resource bodies, DOM/storage/console snapshots,
  cookies for observed URLs, and the final page archive.

Browser capture writes finish before exit, with a 10-second deadline. The log
records incomplete operations. WebKit exposes fewer wire-level details than
curl: its request records can omit network-process headers and request bodies.
Cross-origin iframe DOM and unfinished streaming bodies are not captured.
Captures contain cookies, tokens, and page data and are excluded from Git.

## Requirements and verification

Requires Rust and curl. GTK 3/WebKitGTK 4.1 development libraries are needed to
build; a graphical desktop is needed only when a challenge opens WebKit.

```sh
# Parsing, cookie/agent persistence, locking, and HTTP-first CLI without a display.
cargo test --bin animepahe-dl --test http_flow

# Desktop: browser cookie import/export, captures, and clean shutdown.
cargo test --test capture -- --ignored

# Desktop: challenge and destination detection fixtures.
cargo test --test completion -- --ignored --test-threads=1

# Live checks invoke the fixed executable, not a separate fetching tool.
cargo test --test live_site fixed_application_handles_another_site -- --ignored --nocapture
cargo test --test live_site manual_challenge_flow_on_another_site -- --ignored --nocapture
```

See `CAPTURE_ANALYSIS.md` for the observed challenge sequence and HTTP reuse check.
