# animepahe-dl

AnimePahe episode search and downloader. It supports episode selection,
resolution and audio options, playlist-only output, and downloads through
`yt-dlp`. If AnimePahe presents a CAPTCHA, the app opens WebKit for you to solve
it, saves the refreshed session, and retries the request.

Running the program without arguments prints help and makes no network request.

## Build and install

Install Rust and the native build dependencies listed under
[Requirements](#requirements-and-verification), then run from the repository
root:

```sh
cargo build --release
```

To make `animepahe-dl` available as a command on your `PATH`, install it with:

```sh
cargo install --path .
```

Tagged releases are built and published automatically by the GitHub Actions and
Forgejo Actions release workflows. Push a `v*` version tag to trigger a release;
the workflows publish the Linux x86_64 archive and checksums. The `latest` tag
tracks the most recently released version on both forges.

## Usage

After installing, run the executable:

```sh
animepahe-dl --help

# Search by title and download selected episodes.
animepahe-dl -a "Anime title" -e '1,3-5' -r 1080 -o eng

# Select a series by its AnimePahe session ID; list episodes only.
animepahe-dl -s anime-session-id -e '*' -l

# Replace an existing episode file.
animepahe-dl -s anime-session-id -e 1 --force
```

Existing episode files are skipped unless `--force` is used. Use `--capture`
to save diagnostics for a run.

## Configuration

On first run, the app creates `~/.config/animepahe-dl/config.toml` (or the
equivalent under `$XDG_CONFIG_HOME`) from the shipped defaults. The config
supports AnimePahe host, audio language, quality, output directory, filename
template, and concurrent fragments. The filename template supports `{anime}`,
`{episode}`, `{episode:02}`, and `{quality}`. By default, the audio language is
`original` and quality is `best`; command-line options override config values
for one run.

Downloads use the configured filename template. By default, a series folder
contains files named `Anime Name e01 720p.mp4`.

## Session and CAPTCHA handling

The reusable AnimePahe session is stored in
`~/.config/animepahe-dl/session/`. It contains the cookie jar, user agent, and
persistent WebKit profile. Existing project-local `.session/` and `config.json`
settings are migrated on first run. Session files have restricted permissions
and are excluded from Git.

AnimePahe API and page requests use the saved cookies and user agent. When a
challenge is detected, WebKit opens for manual completion; the app saves updated
cookies and retries the original request. Session reuse is not guaranteed if
AnimePahe expires or rejects the saved clearance.

## Captures

Captures are disabled by default. Add `--capture` to save request, browser, and
download diagnostics under
`~/.config/animepahe-dl/captures/<timestamp>-<pid>/` (or under
`$XDG_CONFIG_HOME`). Captures may contain cookies, tokens, and page data and are
excluded from Git.

## Requirements and verification

Requires Rust, curl, yt-dlp, ffmpeg, ffprobe, and Python with yt-dlp's AES
support (`python-pycryptodomex`). GTK 3 and WebKitGTK 4.1 development libraries
are needed to build; a graphical desktop is needed only when a CAPTCHA opens
WebKit. `fzf` is used for interactive selection. Playlist-only mode skips
download-only dependencies.

```sh
cargo test --locked
```
