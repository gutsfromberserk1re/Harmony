# Downloads the latest official yt-dlp.exe and deno.exe into src-tauri/binaries so the installer ships it.
$ErrorActionPreference = "Stop"
$dir = Join-Path $PSScriptRoot "..\src-tauri\binaries"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$out = Join-Path $dir "yt-dlp-x86_64-pc-windows-msvc.exe"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Write-Host "Downloading latest yt-dlp.exe ..."
Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe" -OutFile $out
$mb = [math]::Round((Get-Item $out).Length / 1MB, 1)
Write-Host "Saved $out ($mb MB)"

# Deno: recent yt-dlp versions need a JavaScript runtime for YouTube. Tauri requires the
# sidecar to be named with the target triple.
$denoOut = Join-Path $dir "deno-x86_64-pc-windows-msvc.exe"
$zip = Join-Path $env:TEMP "deno.zip"
$tmp = Join-Path $env:TEMP "deno-extract"
Write-Host "Downloading latest deno.exe ..."
Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip" -OutFile $zip
if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
Expand-Archive -Path $zip -DestinationPath $tmp -Force
Move-Item -Force (Join-Path $tmp "deno.exe") $denoOut
Remove-Item -Force $zip
Remove-Item -Recurse -Force $tmp
$mb = [math]::Round((Get-Item $denoOut).Length / 1MB, 1)
Write-Host "Saved $denoOut ($mb MB)"
