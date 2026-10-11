# Assembles dist\uRec (portable folder + zip) and builds the Inno Setup installer.
# Usage: ci\build-installer.ps1 [-Version 0.1.0]
param([string]$Version = "")
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot)
if (-not $Version) { $Version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value }
$ff = & "$PSScriptRoot\get-ffmpeg.ps1" | Select-Object -Last 1
$ffDir = Split-Path $ff

$dist = "dist\uRec"
if (Test-Path dist) { Remove-Item dist -Recurse -Force }
New-Item -ItemType Directory $dist | Out-Null
Copy-Item target\release\uRec.exe $dist
Copy-Item (Join-Path $ffDir "ffmpeg.exe"), (Join-Path $ffDir "ffprobe.exe") $dist
Copy-Item LICENSE $dist\LICENSE.txt
Copy-Item (Join-Path $ffDir "LICENSE.txt") $dist\FFMPEG-LICENSE.txt

# Third-party notices: Rust crates (from cargo metadata) and FFmpeg.
$meta = cargo metadata --format-version 1 --filter-platform x86_64-pc-windows-msvc | ConvertFrom-Json
$resolved = @{}
foreach ($n in $meta.resolve.nodes) { $resolved[$n.id] = $true }
$crates = $meta.packages | Where-Object { $resolved.ContainsKey($_.id) -and $_.source } | Sort-Object name, version
$notice = @("uRec $Version third-party components", "",
    "FFmpeg 9.0.2 (ffmpeg.exe, ffprobe.exe): GPL version 3. Build by gyan.dev (essentials).",
    "  Source code: https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz",
    "  Build details: https://www.gyan.dev/ffmpeg/builds/",
    "  The FFmpeg programs are separate executables run by uRec; their license text is in FFMPEG-LICENSE.txt.",
    "", "Rust crates compiled into uRec.exe:")
foreach ($c in $crates) { $notice += ("  {0} {1}  ({2})" -f $c.name, $c.version, $c.license) }
$notice | Out-File -Encoding utf8 $dist\THIRD-PARTY-NOTICES.txt
"$($crates.Count) crates listed"

@"
uRec $Version

Lightweight screen and game recorder for Windows 10/11.
Default hotkeys: Alt+F9 start/stop recording, Alt+F10 save Instant Replay,
Alt+Shift+F10 turn Instant Replay on/off, Ctrl+Alt+M mute microphone.
Recordings are saved to Videos\uRec. Settings are in %APPDATA%\uRec.
Nothing is uploaded and no account is needed.
https://github.com/u2alr/uRec
"@ | Out-File -Encoding utf8 $dist\README.txt

Compress-Archive -Path "$dist\*" -DestinationPath "dist\uRec-$Version-portable.zip"

$iscc = Get-ChildItem "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $iscc) {
    choco install innosetup -y --no-progress | Out-Null
    $iscc = Get-ChildItem "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
}
if (-not $iscc) { throw "Inno Setup not found" }
& $iscc.FullName /Q "/DAppVersion=$Version" "/DSourceDir=..\$dist" installer\uRec.iss
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
Get-ChildItem dist -File | ForEach-Object { "{0}  {1:N1} MB  sha256 {2}" -f $_.Name, ($_.Length / 1MB), (Get-FileHash $_.FullName).Hash }
