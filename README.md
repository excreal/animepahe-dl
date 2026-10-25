# HTTP-first page test with manual WebKit fallback

Fetch page details from any HTTP(S) URL using HTTP and a reusable session.
Start WebKit only if the HTTP response contains a CAPTCHA/challenge. The
original Kwik URL remains the default when `--url` is omitted.

```sh
cargo run
```

```sh
cargo run -- --url https://idrama.dramafren.org/
```

The previous command `cargo run -- --show-on-challenge` is still accepted and
now uses this same HTTP-first flow. It no longer starts a hidden browser.

The fixed executable was tested live against `https://idrama.dramafren.org/`:
HTTP 403 challenge → browser verification → loaded `IDRAMA Player - Watch Free
Short Dramas` → automatic close → saved-session HTTP 200. Capture:
`captures/1792938493406-296703/`. These requests were made by the application.

The default flow:

1. Load `.session/cookies.txt` and `.session/user-agent.txt`.
2. Fetch over HTTP using curl. Save response HTML, raw headers, traffic trace,
   and `page-details.json` with the title, file metadata, forms, and asset URLs.
3. On a normal response, exit without initializing GTK or creating WebKit.
4. On a detected challenge, initialize GTK, restore the browser profile/cookies
   and the same user agent, and show the browser for you to solve it manually.
5. Wait for a complete HTTP(S) document with no detected CAPTCHA and a successful
   HTTP response for two consecutive checks, capture it, export updated cookies,
   close the browser, and check whether the session also works over HTTP.

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

## Reusable session

Your successful capture `captures/1792936784425-255142` was imported into
`.session/`. Future runs reuse it automatically. This folder contains:

- `cookies.txt`: shared Netscape cookie jar, including HTTP-only and session cookies.
- `user-agent.txt`: the exact user agent recorded by the browser.
- `session.json`: import/update information.
- `browser-data/`, `browser-cache/`, and `browser-cookies.sqlite`: persistent
  WebKit profile data created when the browser is needed.
- `lock`: prevents simultaneous runs from overwriting this session.

Refreshing cookies for one site preserves saved cookies for other sites and
unvisited paths. Cookies remain scoped to their domains when curl/WebKit send them.

The session directory has permissions `700`, newly written cookie/agent files
have permissions `600`, and `.session/` is excluded from Git. Cookies are
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
cargo run -- --import-session captures/<run> --import-only
```

## Options

```sh
# Never launch a browser; return an error when challenged.
cargo run -- --http-only

# Use a separate cookie/profile directory.
cargo run -- --session-dir /tmp/kwik-test-session

# Fetch a different site using the same generic detection.
cargo run -- --url https://idrama.dramafren.org/
```

## Captures

Each run prints its folder, `captures/<timestamp>-<pid>/`.

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
cargo test --bin webkit-test --test http_flow

# Desktop: browser cookie import/export, captures, and clean shutdown.
cargo test --test capture -- --ignored

# Desktop: challenge and destination detection fixtures.
cargo test --test completion -- --ignored --test-threads=1

# Live checks invoke the fixed executable, not a separate fetching tool.
cargo test --test live_site fixed_application_handles_another_site -- --ignored --nocapture
cargo test --test live_site manual_challenge_flow_on_another_site -- --ignored --nocapture
```

See `CAPTURE_ANALYSIS.md` for the observed challenge sequence and HTTP reuse check.
