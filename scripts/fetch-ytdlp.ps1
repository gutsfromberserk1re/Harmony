# Downloads yt-dlp, Deno, and the matching bgutil PO Token provider plugin/server.
# The provider is pinned so the plugin and server versions stay compatible.
$ErrorActionPreference = "Stop"
$providerVersion = "2.0.2"
$scriptDir = $PSScriptRoot
$root = Join-Path $scriptDir ".."
$dir = Join-Path $root "src-tauri\binaries"
$resourceRoot = Join-Path $root "src-tauri\resources"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
if (Test-Path $resourceRoot) { Remove-Item -Recurse -Force $resourceRoot }
New-Item -ItemType Directory -Force -Path $resourceRoot | Out-Null
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$out = Join-Path $dir "yt-dlp-x86_64-pc-windows-msvc.exe"
Write-Host "Downloading latest official yt-dlp.exe ..."
Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe" -OutFile $out
$mb = [math]::Round((Get-Item $out).Length / 1MB, 1)
Write-Host "Saved $out ($mb MB)"

# Deno is used both by yt-dlp's YouTube JS challenge solver and the bundled PO Token provider.
$denoOut = Join-Path $dir "deno-x86_64-pc-windows-msvc.exe"
$denoZip = Join-Path $env:TEMP "harmony-deno.zip"
$denoTmp = Join-Path $env:TEMP "harmony-deno-extract"
Write-Host "Downloading latest deno.exe ..."
Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip" -OutFile $denoZip
if (Test-Path $denoTmp) { Remove-Item -Recurse -Force $denoTmp }
Expand-Archive -Path $denoZip -DestinationPath $denoTmp -Force
Move-Item -Force (Join-Path $denoTmp "deno.exe") $denoOut
Remove-Item -Force $denoZip
Remove-Item -Recurse -Force $denoTmp
$mb = [math]::Round((Get-Item $denoOut).Length / 1MB, 1)
Write-Host "Saved $denoOut ($mb MB)"

# The PO Token plugin ZIP is designed to be discovered by yt-dlp when placed under
# <yt-dlp executable folder>\yt-dlp-plugins. Harmony copies it there on first launch.
$pluginDir = Join-Path $resourceRoot "yt-dlp-plugins"
New-Item -ItemType Directory -Force -Path $pluginDir | Out-Null
$pluginZip = Join-Path $pluginDir "bgutil-ytdlp-pot-provider.zip"
Write-Host "Downloading bgutil PO Token plugin $providerVersion ..."
$pluginUrl = "https://github.com/Brainicism/bgutil-ytdlp-pot-provider/releases/download/$providerVersion/bgutil-ytdlp-pot-provider.zip"
Invoke-WebRequest -UseBasicParsing -Uri $pluginUrl -OutFile $pluginZip
if ((Get-Item $pluginZip).Length -lt 1024) { throw "PO Token plugin ZIP looks incomplete: $pluginZip" }

# Bundle the matching provider server. It generates video-bound PO tokens for the mweb client.
$sourceZip = Join-Path $env:TEMP "harmony-bgutil-provider-$providerVersion.zip"
$sourceTmp = Join-Path $env:TEMP "harmony-bgutil-provider-source"
Write-Host "Downloading bgutil PO Token server source $providerVersion ..."
Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/Brainicism/bgutil-ytdlp-pot-provider/archive/refs/tags/$providerVersion.zip" -OutFile $sourceZip
if (Test-Path $sourceTmp) { Remove-Item -Recurse -Force $sourceTmp }
Expand-Archive -Path $sourceZip -DestinationPath $sourceTmp -Force
$sourceRepo = Get-ChildItem $sourceTmp -Directory | Select-Object -First 1
$sourceServer = Join-Path $sourceRepo.FullName "server"
$providerRoot = Join-Path $resourceRoot "bgutil-provider"
$serverDest = Join-Path $providerRoot "server"
if (!(Test-Path (Join-Path $sourceServer "src\main.ts"))) { throw "Could not find bgutil provider server source in $sourceZip" }
New-Item -ItemType Directory -Force -Path $providerRoot | Out-Null
Copy-Item -Recurse -Force $sourceServer $serverDest
$licenseSource = Join-Path $sourceRepo.FullName "LICENSE"
if (Test-Path $licenseSource) { Copy-Item -Force $licenseSource (Join-Path $providerRoot "LICENSE") }

Write-Host "Installing provider server dependencies with the bundled Deno runtime ..."
Push-Location $serverDest
try {
    & $denoOut install --allow-scripts=npm:canvas --frozen
    if ($LASTEXITCODE -ne 0) { throw "Deno could not install bgutil provider dependencies (exit code $LASTEXITCODE)" }
}
finally {
    Pop-Location
}
if (!(Test-Path (Join-Path $serverDest "node_modules"))) { throw "Deno install completed but server\node_modules was not created" }
$serverSize = [math]::Round(((Get-ChildItem $serverDest -Recurse -File | Measure-Object -Property Length -Sum).Sum) / 1MB, 1)
Write-Host "Prepared bgutil provider server ($serverSize MB)"

Remove-Item -Force $sourceZip
Remove-Item -Recurse -Force $sourceTmp
Write-Host "yt-dlp, Deno, plugin, and matching PO Token provider are ready for the Tauri build."
