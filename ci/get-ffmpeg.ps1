# Downloads the pinned FFmpeg build into third_party\ffmpeg and prints the path to ffmpeg.exe.
# Pinned: FFmpeg 9.0.2 "essentials" build by gyan.dev (GPLv3, static, win64).
param([string]$Dest = "third_party\ffmpeg")
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$Version = "9.0.2"
$Url = "https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-$Version-essentials_build.zip"
# SHA-256 of ffmpeg.exe inside that archive (recorded from the first verified download).
$ExpectedExeSha = "3256173F3F8BFFD7DF12227C68ADF68025EDB1832273A9530688A7BB1ED8EDEC"

$exe = Join-Path $Dest "ffmpeg.exe"
if (-not (Test-Path $exe)) {
    New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    $zip = Join-Path $env:TEMP "ffmpeg-$Version.zip"
    $tmp = Join-Path $env:TEMP "ffmpeg-$Version-x"
    if (-not (Test-Path $zip)) { Invoke-WebRequest $Url -OutFile $zip }
    if (Test-Path $tmp) { Remove-Item $tmp -Recurse -Force }
    Expand-Archive $zip -DestinationPath $tmp
    foreach ($n in "ffmpeg.exe", "ffprobe.exe") {
        $f = Get-ChildItem $tmp -Recurse -Filter $n | Select-Object -First 1
        Copy-Item $f.FullName (Join-Path $Dest $n) -Force
    }
    $lic = Get-ChildItem $tmp -Recurse -Filter "LICENSE*" | Select-Object -First 1
    if ($lic) { Copy-Item $lic.FullName (Join-Path $Dest "LICENSE.txt") -Force }
    "FFmpeg $Version essentials build from $Url" | Out-File (Join-Path $Dest "SOURCE.txt")
    Remove-Item $tmp -Recurse -Force
}
$sha = (Get-FileHash $exe -Algorithm SHA256).Hash
if ($sha -ne $ExpectedExeSha) {
    Write-Warning "ffmpeg.exe SHA-256 $sha does not match the pinned $ExpectedExeSha"
    if ($env:UREC_STRICT_FFMPEG -eq "1") { throw "FFmpeg checksum mismatch" }
}
Write-Output (Resolve-Path $exe).Path
