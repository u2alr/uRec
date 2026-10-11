# Downloads a pinned FFmpeg build into third_party\ffmpeg and prints the path.
# Usage: ci\get-ffmpeg.ps1 [-Dest third_party\ffmpeg]
param([string]$Dest = "third_party\ffmpeg")
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

if (Test-Path "$Dest\ffmpeg.exe") { Write-Output (Resolve-Path "$Dest\ffmpeg.exe").Path; return }
New-Item -ItemType Directory -Force -Path $Dest | Out-Null

# BtbN builds of the FFmpeg 8.0 release branch (GPL, static, win64).
$pattern = 'ffmpeg-n8\.0.*-win64-gpl-8\.0\.zip$'
$url = $null
try {
    $rel = Invoke-RestMethod -Headers @{ "User-Agent" = "urec-ci" } `
        "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest"
    $asset = $rel.assets | Where-Object { $_.name -match $pattern } | Select-Object -First 1
    if ($asset) { $url = $asset.browser_download_url; Write-Host "FFmpeg build: $($rel.tag_name) / $($asset.name)" }
} catch { Write-Host "release lookup failed: $_" }
if (-not $url) { $url = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip" }

$zip = Join-Path $env:TEMP "ffmpeg.zip"
$tmp = Join-Path $env:TEMP "ffmpeg-x"
Invoke-WebRequest $url -OutFile $zip
if (Test-Path $tmp) { Remove-Item $tmp -Recurse -Force }
Expand-Archive $zip -DestinationPath $tmp
foreach ($n in "ffmpeg.exe", "ffprobe.exe") {
    $f = Get-ChildItem $tmp -Recurse -Filter $n | Select-Object -First 1
    if ($f) { Copy-Item $f.FullName (Join-Path $Dest $n) -Force }
}
$lic = Get-ChildItem $tmp -Recurse -Filter "LICENSE*" | Select-Object -First 1
if ($lic) { Copy-Item $lic.FullName (Join-Path $Dest "LICENSE.txt") -Force }
$url | Out-File (Join-Path $Dest "SOURCE.txt")
Write-Output (Resolve-Path "$Dest\ffmpeg.exe").Path
