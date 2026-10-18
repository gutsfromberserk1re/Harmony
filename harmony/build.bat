@echo off
setlocal
cd /d "%~dp0"

echo [1/3] Fetching yt-dlp.exe
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\fetch-ytdlp.ps1 || goto :fail

echo [2/3] Checking Tauri CLI
cargo tauri --version >nul 2>&1 || cargo install tauri-cli --version "^2" --locked || goto :fail

echo [3/3] Building installer
cargo tauri build || goto :fail

echo.
echo Done. Your installer is in: src-tauri\target\release\bundle\nsis\
explorer src-tauri\target\release\bundle\nsis
goto :eof

:fail
echo.
echo Build failed - see the messages above.
exit /b 1
