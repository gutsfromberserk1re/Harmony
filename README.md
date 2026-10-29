# Harmony

**A lightweight, native desktop music player.**

Remake of the original python software

<p align="center">
  <a href="https://github.com/gutsfromberserk1re/Harmony/actions/runs/39566500291/artifacts/12330523400">
    <img src="https://img.shields.io/badge/⬇%20Download-Harmony%20for%20Windows-7C83FD?style=for-the-badge&logo=windows&logoColor=white" alt="Download Harmony for Windows">
  </a>
  <a href="https://github.com/gutsfromberserk1re/hamonyr">
    <img src="https://img.shields.io/badge/Source%20Code-GitHub-24292F?style=for-the-badge&logo=github&logoColor=white" alt="View source code on GitHub">
  </a>
</p>

<p align="center">
  <sub>Windows · Tauri · Rust · yt-dlp</sub>
</p>

---
**NOTE**: IF YOUTUBE IS BLOCKING YOUR APP ADD COOKIES FROM THE MENU 
---
## ✨ Highlights

- Discord rich presence
- **EXTREMELY** Lightweight only 5 mb of ram usage (aside from the webview it runs which consumes around 50 soooo 55-60 in total)
- super clean
- Cookies are optional 
---- 
# Images 
<p align="center">
  <img src="https://github.com/user-attachments/assets/15ec827a-e1e9-409d-8da6-0230f48b6a7a" alt="Harmony application screenshot" width="819">
</p>

<p align="center">
  <img src="https://github.com/user-attachments/assets/2824a447-a7ec-4c39-9f04-c17f661538d1" alt="Harmony interface preview" width="439">
</p>

<p align="center">
  <img src="https://github.com/user-attachments/assets/a9e403b9-327d-4387-81ee-5ffe8e273eae" alt="Harmony application screenshot" width="842">
</p>





-----
## 📥 Download & Install

### Recommended: Prebuilt installer

<a href="https://github.com/gutsfromberserk1re/hamonyr/releases/download/1.0.0/Harmony-installer.zip">
  <img src="https://img.shields.io/badge/Download%20Harmony-Get%20the%20Windows%20Installer-7C83FD?style=for-the-badge&logo=windows&logoColor=white" alt="Download the Harmony Windows installer">
</a>

1. Download the installer ZIP using the button above.
2. Extract the ZIP archive.
3. Run the included installer.
4. Launch Harmony from your Start Menu or Desktop shortcut.

The installer is designed to install per-user, without requiring administrator privileges. It includes yt-dlp and a WebView2 runtime bootstrapper.




## 📂 Application Data

Harmony stores its local configuration and application data in:

```text
%LOCALAPPDATA%\Harmony\
```


| File | Purpose |
|---|---|
| `settings.json` | Application settings and preferences |
| `harmony.log` | Application logs |
| `yt-dlp.exe` | Local, self-updating yt-dlp executable |

Your existing playlists and preferences from the Python version are imported automatically the first time you launch Harmony.

## 🧰 Tech Stack

| Technology | Role |
|---|---|
| [Tauri](https://tauri.app/) | Native desktop application framework |
| [Rust](https://www.rust-lang.org/) | Native application backend |
| [yt-dlp](https://github.com/yt-dlp/yt-dlp) | Media extraction |
| WebView2 | Windows UI rendering |

## 📄 License

See the repository for license information.

---

<p align="center">
  <strong>Harmony</strong>
</p>

## Playback reliability notes

Playback uses a small loopback-only HTTP relay between WebView2 and the upstream media URL. The relay preserves yt-dlp's selected-format request headers and forwards HTTP byte-range requests, which helps avoid media URLs that fail when opened directly by the browser and keeps seeking functional. Playback errors are also logged in the WebView console with media error codes, and retry strategy history is tracked per video so prefetching another track cannot overwrite it.
