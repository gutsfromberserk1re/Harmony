# Harmony authentication and playback patch — 2026-10-29

This source patch targets the Rust/Tauri Harmony application.

## Changes

- Keeps the per-video strategy map on `OnceLock<Mutex<HashMap<...>>>`, avoiding the non-const `HashMap::new()` static initializer error.
- Tries the user's imported `cookies.txt` immediately after the default extractor strategy, before anonymous client fallbacks.
- Avoids noisy browser-cookie attempts for missing browser profiles. If a manual cookie file is present, Harmony uses that file and does not additionally attempt Windows browser-cookie decryption, which can fail with DPAPI/app-bound encryption.
- Makes `cookies_status` recognise a validly located cookie file next to the executable as well as the application-data folder.
- Logs why an invalid startup cookie file is ignored.
- Returns yt-dlp's actual error text when extraction fails, rather than replacing bot-check failures with a JSON parse error.
- Adds default `Accept` and YouTube `Referer` headers to the media relay if yt-dlp did not supply them, while preserving the extracted headers and range/seek behavior.
- Probes each candidate media URL with a short `Range: bytes=0-0` request before giving it to WebView2. When a URL is rejected (for example HTTP 403), Harmony tries another extraction strategy within the existing stream lookup time budget instead of immediately starting a reconnect loop.
- Prevents a slow stream probe from registering a stale URL after the user has already skipped to another track.
- Updates the cookie guide to reduce the chance of exporting a cookie that YouTube has just rotated, and clarifies that cookies are login-session credentials.
- Makes the retry message less misleading: YouTube's current enforcement may involve both cookies and Proof-of-Origin tokens, so a cookie file alone is not guaranteed to solve every bot check or 403.

## Validation

- `src/app.js` passed `node --check`.
- All Tauri JSON files passed JSON parsing.
- Archive integrity is checked before delivery.
- A full Rust/Windows build could not be run in this editing environment because Rust/Cargo are not installed. Run the provided GitHub Actions workflow to verify compilation and produce the Windows installer.

## Important limitation

YouTube controls authentication and media access remotely. This patch improves cookie selection, client fallback, request headers, and early detection of rejected stream URLs; it cannot guarantee access when YouTube blocks the account/IP or requires a Proof-of-Origin token provider that is not installed. If bot checks continue despite a fresh signed-in cookie export, the log should now show the original yt-dlp error rather than hiding it.


## Follow-up: persistent HTTP 403s / PO-token enforcement

- The Windows build now fetches the matching bgutil PO Token provider plugin and server (version 2.0.2) and bundles them with the app. It does not need Python or Docker on the user's machine; the bundled Deno runtime runs the provider server.
- The app installs the ZIP plugin beside its own yt-dlp executable, where yt-dlp's plugin loader discovers it.
- The token server binds only to `127.0.0.1`, starts on demand for YouTube extraction, and shuts down after three minutes without extraction activity or when Harmony exits.
- Added `mweb` fallback, including a combined `mweb` + imported-cookie attempt, and passes the provider's loopback URL to yt-dlp so video-bound GVS PO tokens can be requested.
- Stream extraction runs with verbose diagnostics, but only PO-token-provider status lines are copied into Harmony's log (not full signed URLs). The media URL validator logs a bounded response-body excerpt for HTTP failures to distinguish signature/expiry/token errors where the upstream provides useful text.
- The build workflow prepares resources before `cargo tauri build`, and the provider version is pinned to match its plugin.

Validation notes: JavaScript and JSON syntax checks pass. This environment has no Rust/Cargo or PowerShell, so it cannot locally compile the Tauri app or execute the Windows dependency-preparation script. Run the GitHub Actions build before installing. The provider itself explicitly warns that PO tokens do not guarantee every 403 or bot-check will disappear; account/IP restrictions may still require a fresh signed-in cookie export or may remain inaccessible.

## Corrected 403 recovery package

The source package now includes `src-tauri/src/pot_provider.rs` and the build script that fetches the matching `bgutil-ytdlp-pot-provider` plugin and server (v2.0.2). The earlier cookie-and-relay-only variant did not include this runtime integration. After rebuilding, confirm `harmony.log` records `PO Token provider ready` and a yt-dlp `PO Token Providers` line when `mweb` is used. If the provider is missing or fails to start, `pot-provider.log` records its startup diagnostics. This provider can help with missing GVS PO tokens, but cannot guarantee access if YouTube blocks the account/IP or rejects the attestation.
