# Downloads yt-dlp.exe and deno.exe (JavaScript runtime yt-dlp needs) into src-tauri/binaries.
$ErrorActionPreference = "Stop"
$dir = Join-Path $PSScriptRoot "..\src-tauri\binaries"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$out = Join-Path $dir "yt-dlp-x86_64-pc-windows-msvc.exe"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Write-Host "Downloading latest yt-dlp.exe ..."
Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe" -OutFile $out
$mb = [math]::Round((Get-Item $out).Length / 1MB, 1)
Write-Host "Saved $out ($mb MB)"

Write-Host "Downloading latest deno.exe ..."
$zip = Join-Path $env:TEMP "deno.zip"
$tmp = Join-Path $env:TEMP "deno-extract"
Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip" -OutFile $zip
Expand-Archive -Force -Path $zip -DestinationPath $tmp
$denoOut = Join-Path $dir "deno-x86_64-pc-windows-msvc.exe"
Copy-Item (Join-Path $tmp "deno.exe") $denoOut -Force
$mb = [math]::Round((Get-Item $denoOut).Length / 1MB, 1)
Write-Host "Saved $denoOut ($mb MB)"
