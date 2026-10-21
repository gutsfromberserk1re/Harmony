# Harmony

**A lightweight, native desktop music player.**

Harmony brings the Python version into a compact native application built with **Tauri and Rust**, combining a lightweight desktop experience with `yt-dlp` for media extraction.

<p align="center">
  <a href="https://github.com/gutsfromberserk1re/hamonyr/releases/download/1.0.0/Harmony-installer.zip">
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

## ✨ Highlights

- **Native desktop experience** — built with Tauri and Rust.
- **Lightweight by design** — a compact alternative to a heavier Python desktop build.
- **Media extraction** — powered by the bundled `yt-dlp.exe`.
- **Automatic updates** — maintains an up-to-date local copy of yt-dlp.
- **Migration support** — automatically imports playlists and preferences from the previous Python version on first launch.
- **Easy installation** — per-user setup with shortcuts and an uninstaller.
---- 
# Images 
<p align="center">
  <img src="https://github.com/user-attachments/assets/15ec827a-e1e9-409d-8da6-0230f48b6a7a" alt="Harmony application screenshot" width="819">
</p>

<p align="center">
  <img src="https://github.com/user-attachments/assets/2824a447-a7ec-4c39-9f04-c17f661538d1" alt="Harmony interface preview" width="439">
</p>

<p align="center">
  <img src="https://github.com/user-attachments/assets/7165f7b5-fb35-4a5f-a9b6-e77f70f34b60" alt="Harmony application screenshot" width="842">
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
