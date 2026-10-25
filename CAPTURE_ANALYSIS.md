# Kwik capture analysis

## Generic detection verified on another live site

The fixed application was run against `https://idrama.dramafren.org/` through
its live integration test. The test invoked the compiled Rust executable; no
separate curl or browsing-tool fetch was used for this site.

Capture: `captures/1792938493406-296703/`.

- The application's HTTP request returned 403 with a Cloudflare challenge.
- It initialized WebKit only after that challenge was detected.
- After verification, WebKit loaded `IDRAMA Player - Watch Free Short Dramas`.
- The generic detector reported `Document finished loading with no detected CAPTCHA`.
- The browser captured the page, closed cleanly, and saved the session.
- The application's subsequent HTTP request returned 200 using the saved session.
- Static parsing produced 15 headings, 37 links, and 14 asset URLs without JavaScript execution.

A separate application invocation then reused this session and returned HTTP
200 without starting a browser. Its details are saved in
`captures/1792938554932-298226/page-details.json`.

Completion now requires a complete HTTP(S) document, a successful HTTP response,
and no known CAPTCHA signals for two consecutive observations. It requires no
Kwik hostname, title suffix, `#app`, or download form. Hidden/inactive widgets
and stale empty hidden response fields do not by themselves block completion.
Cookie refresh preserves sessions for other sites; the shared jar retains
cookies for both dramafren.org and kwik.cx after this run.

## Latest run: `1792936784425-255142`

This run exited cleanly at 43.58 seconds, with zero pending capture writes and
no timeout. Its state transitions were:

| Time after start | Observation |
| --- | --- |
| 3.08 s | HTTP 403, `cf-mitigated: challenge`, title `Just a moment...` |
| 33.18 s | `verified_waiting`; manual verification succeeded, destination still pending |
| 36.02 s | Destination response committed |
| 41.49 s | Destination document finished loading |
| 42.39 s | First confirmed `ready` observation |
| 43.32 s | Second confirmed `ready` observation |
| 43.58 s | Captures finished and application exited |

The run saved 35 resource starts, 29 response metadata records, 34 bodies, 29 DOM
snapshots, and 26 cookie snapshots. It again produced `cf_clearance`,
`kwik_session`, and `srv`. The first two have recorded expirations; `srv` is a
session cookie and must be exported explicitly to preserve it between runs.
Both final localStorage and sessionStorage were empty.

The old `--show-on-challenge` implementation still started a hidden WebKit
process before detecting the challenge. It did not satisfy HTTP-first fetching.
The implementation now makes an HTTP request before GTK initialization and
creates WebKit only after detecting a challenge response.

Cookies and the exact recorded user agent from this capture have been imported
into `.session/`. A direct curl request using that saved cookie jar and user
agent returned HTTP 200 with the real filename title and no `cf-mitigated`
challenge header. This confirms HTTP session reuse worked for this request;
future server acceptance still needs checking on every fetch.

The HTTP response exposes the file's name, MIME type, size (42,914,847 bytes),
resolution, media properties, and hashes in static HTML. It has no download
form: JavaScript creates that form in the rendered document. Browser-free
extraction therefore reports static metadata and asset URLs, and records
`javascript_executed: false`; it does not claim to have rendered the page.

The direct live HTTP check succeeded, while executing the whole CLI inside the
tool sandbox failed DNS resolution. CLI integration tests use a local HTTP
server with display variables removed; they verify successful fetches, cookie
reuse, a stable user agent, and no browser launch for normal HTTP responses.

## Earlier run

Source: `captures/1792936259896-235351/` (one successful manual verification).

The application completed and shut down normally in this run: `capture_finished`
recorded zero pending writes and no timeout, followed by `TerminatedByApi` and
`exit`. That termination reason is the app's explicit cleanup, not a crash.

## Observed sequence

Times are seconds after the capture's `start` event.

| Time | Evidence | Interpretation |
| --- | --- | --- |
| ~2.3 | Resource 4: target URL, HTTP 403, `cf-mitigated: challenge`, HTML | Cloudflare interstitial, not the file page |
| 4.8 | `Just a moment...`; visible text `Performing security verification` | Challenge document finished loading; continue waiting |
| Before 21.9 | Snapshots 41–43 show `Verification successful. Waiting for kwik.cx to respond` | Verification succeeded, but the destination is not loaded yet |
| 21.9 | New navigation starts at the same file path with a temporary Cloudflare query parameter | Cloudflare initiates the return to the destination |
| ~24.0 | Resource 44: target URL, HTTP 200, HTML | Destination response received |
| 24.79 | Filename title ending `:: Kwik`; `#app` contains a visible POST form whose action is `/d/<file-id>` | Destination document loaded |
| 26.01 | Two successful completion checks; final capture starts | App begins shutdown |
| 26.11 | Zero pending writes, no capture timeout, explicit termination, exit | Clean completion |

## Signals to use

- Detect the main document's `cf-mitigated: challenge` header, or the challenge title/visible unresolved Turnstile UI, and present the browser for manual interaction.
- Treat visible `Verification successful. Waiting for ...` text as `verified_waiting`, not completion. Use visible text: the success message already exists in the challenge's HTML inside a hidden container before verification.
- Confirm readiness using the target's HTTP 200 response, complete document, filename title ending `:: Kwik`, and visible `#app form[method=POST]` pointing to a same-site `/d/` path. Require two consecutive checks and no navigation in progress.
- Log the state and reason with each completion check for future analysis.

Do not depend on the random challenge element IDs in this capture. Do not use
Cloudflare script presence or generic iframe presence as a challenge signal:
the destination also loads a `/cdn-cgi/challenge-platform/.../scripts/jsd/...`
script and contains hidden iframes. A filled Turnstile field alone also does not
prove that the destination has finished loading.

Final cookies include `cf_clearance`, `kwik_session`, and `srv` (values omitted).
All three are HTTP-only in this capture, so `document.cookie` cannot observe
them. Cookie presence is supporting evidence, not a completion condition;
clearance may predate a navigation or no longer be accepted.

## Traffic coverage and nonfatal errors

The capture contains 35 resource starts, 36 sent-request events, 29 response
metadata records, 34 saved bodies, and 28 DOM snapshots. Body files include
HTML, CSS, JavaScript, fonts, and images; `events.jsonl` maps resource IDs to
URLs, MIME types, headers, and body filenames. Some resources are local/blob
documents rather than network exchanges.

Three resource failures were recorded: two canceled loads and a DNS failure for
a Cloudflare subdomain. A Cloudflare PAT request returned HTTP 401. These did
not prevent this run from reaching the file page; none should alone be treated
as proof that verification failed.

The recorded request headers do not include every wire-level field (for example,
the observed target request entries omit `Cookie`, despite the final cookie
jar). Response headers include `Set-Cookie` on the successful live target
response, but that field was omitted by WebKit in a local HTTP fixture.
These are WebKit-exposed records, not a complete packet capture. POST request
bodies and cross-origin frame DOM are not available through this logger.

## Implemented behavior

`cargo run` fetches the page using HTTP, saved cookies, and the saved user agent.
It saves static page details and exits without a browser on a normal response.
Only a detected challenge opens WebKit for manual verification. After success,
it exports refreshed cookies and checks HTTP reuse once. The old
`--show-on-challenge` flag is accepted as an alias for this default flow.

The capture GUI regression now runs on the process main thread using a custom
test harness. The earlier libtest worker-thread setup could abort after its
assertions passed, while the normal application run above exited cleanly.
