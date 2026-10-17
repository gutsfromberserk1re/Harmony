# Downloads the latest official yt-dlp.exe into src-tauri/binaries so the installer ships it.
$ErrorActionPreference = "Stop"
$dir = Join-Path $PSScriptRoot "..\src-tauri\binaries"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$out = Join-Path $dir "yt-dlp-x86_64-pc-windows-msvc.exe"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Write-Host "Downloading latest yt-dlp.exe ..."
Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe" -OutFile $out
$mb = [math]::Round((Get-Item $out).Length / 1MB, 1)
Write-Host "Saved $out ($mb MB)"
