# Harmony (Tauri / Rust)

Same player as the Python version, rebuilt as a small native app with `yt-dlp.exe` bundled.

# Download 

## Build the one-file installer
Requirements (once): Rust (https://rustup.rs, MSVC toolchain) and Visual Studio Build Tools.

    build.bat

The installer appears in `src-tauri\target\release\bundle\nsis\Harmony_1.0.0_x64-setup.exe`.
It installs per-user (no admin prompt), creates Start Menu + Desktop shortcuts and an uninstaller,
and includes yt-dlp and the WebView2 runtime bootstrapper.

## Run while developing
    powershell -File scripts\fetch-ytdlp.ps1
    cargo tauri dev

## Data
`%LOCALAPPDATA%\Harmony` holds settings.json, harmony.log and the self-updating yt-dlp.exe copy.
Playlists and preferences from the old Python version are imported automatically on first launch.

