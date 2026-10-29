# Harmony playback fix — 2026-10-29

## What changed

- Stream extraction now asks yt-dlp for its single-video JSON metadata instead of printing only a bare URL. The selected media URL and its associated HTTP headers are both retained.
- Added a loopback-only Rust HTTP relay. WebView2 requests a local URL; the relay sends the upstream headers and forwards byte-range requests/responses so audio playback and seeking can work with media URLs that need specific request headers.
- Retry strategy history is tracked per video ID, so prefetching another track can't overwrite the strategy recorded for the currently failing track.
- The per-video strategy map now uses `OnceLock<Mutex<HashMap<...>>>` for lazy initialization, fixing the Rust compile error caused by initializing `HashMap::new()` directly inside a static.
- Playback promise rejections and HTML media errors now record useful diagnostic details in the WebView console, and the last media error can appear in the existing retry status.

## Validation

- `src/app.js` passed `node --check`.
- Tauri JSON configuration files passed Python JSON parsing.
- A Rust/Windows build was not run in the editing environment because Rust/Cargo are not installed here. The GitHub Actions Windows workflow should be used to compile and package this source before replacing a current installation.

## Important

This is a source ZIP intended for a normal GitHub Actions build, not a precompiled Windows installer. The relay is bound to `127.0.0.1` on an operating-system-selected port and is not exposed on the local network.
