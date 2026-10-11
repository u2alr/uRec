# Real-capture integration tests on Windows. Records the actual screen with urec-cli,
# then checks every output file with ffprobe and a full decode.
# Usage: ci\integration.ps1 -Cli target\release\urec-cli.exe
param([Parameter(Mandatory)][string]$Cli)
$ErrorActionPreference = "Continue"
$ProgressPreference = "SilentlyContinue"

$ff = & "$PSScriptRoot\get-ffmpeg.ps1" | Select-Object -Last 1
$fp = $ff -replace 'ffmpeg\.exe$', 'ffprobe.exe'
$env:UREC_FFMPEG = $ff
$work = Join-Path $env:RUNNER_TEMP "urec-it"
if (Test-Path $work) { Remove-Item $work -Recurse -Force }
New-Item -ItemType Directory $work | Out-Null

Add-Type -AssemblyName System.Windows.Forms
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$src = "$($screen.Width)x$($screen.Height)"
"Screen: $src"

$results = New-Object System.Collections.ArrayList

function Invoke-Cli([string[]]$a) {
    $out = & $Cli @a 2>&1 | ForEach-Object { "$_" }
    $kv = @{}
    foreach ($l in $out) { if ($l -match '^([a-z_]+)=(.*)$') { $kv[$Matches[1]] = $Matches[2] } }
    return @{ Code = $LASTEXITCODE; Out = $out; Kv = $kv }
}

function Get-MediaInfo([string]$path) {
    if (-not (Test-Path $path)) { return $null }
    $j = & $fp -v error -show_entries "stream=codec_type,codec_name,width,height,r_frame_rate:format=duration,size" -of json $path | ConvertFrom-Json
    $v = $j.streams | Where-Object codec_type -eq "video" | Select-Object -First 1
    $a = @($j.streams | Where-Object codec_type -eq "audio")
    $decodeErr = (& $ff -hide_banner -v error -i $path -f null - 2>&1 | Out-String).Trim()
    $num, $den = "$($v.r_frame_rate)".Split('/')
    return [pscustomobject]@{
        Codec = $v.codec_name; Width = [int]$v.width; Height = [int]$v.height
        Fps = [math]::Round([double]$num / [double]$den, 2)
        Duration = [math]::Round([double]$j.format.duration, 2)
        SizeMB = [math]::Round([double]$j.format.size / 1MB, 2)
        AudioTracks = $a.Count; DecodeErrors = $decodeErr
    }
}

function Add-Result($id, $name, $pass, $detail) {
    $null = $results.Add([pscustomobject]@{ Id = $id; Test = $name; Result = $(if ($pass) { "PASS" } else { "FAIL" }); Detail = $detail })
    "{0} {1,-4} {2} :: {3}" -f $id, $(if ($pass) { "PASS" } else { "FAIL" }), $name, $detail
}

function Test-Recording($id, $name, [string[]]$cliArgs, $expW, $expH, $expFps, $minDur, $expAudio, $expCodec = "h264") {
    $out = Join-Path $work "$id.$(if ($cliArgs -contains 'mkv') { 'mkv' } else { 'mp4' })"
    $r = Invoke-Cli (@("record", "--out", $out, "--source", $src) + $cliArgs)
    $m = Get-MediaInfo $out
    if ($r.Code -ne 0 -or -not $m) {
        Add-Result $id $name $false ("exit=$($r.Code) " + (($r.Out | Select-Object -Last 4) -join " | "))
        return $r
    }
    $checks = @()
    if ($m.Width -ne $expW -or $m.Height -ne $expH) { $checks += "size $($m.Width)x$($m.Height) != ${expW}x${expH}" }
    if ([math]::Abs($m.Fps - $expFps) -gt 0.5) { $checks += "fps $($m.Fps) != $expFps" }
    if ($m.Duration -lt $minDur) { $checks += "duration $($m.Duration) < $minDur" }
    if ($m.AudioTracks -ne $expAudio) { $checks += "audio tracks $($m.AudioTracks) != $expAudio" }
    if ($m.Codec -ne $expCodec) { $checks += "codec $($m.Codec) != $expCodec" }
    if ($m.DecodeErrors) { $checks += "decode errors: $($m.DecodeErrors.Substring(0, [math]::Min(150, $m.DecodeErrors.Length)))" }
    $detail = "$($r.Kv['encoder']) via $($r.Kv['method'])/$($r.Kv['frame_path']) -> $($m.Width)x$($m.Height)@$($m.Fps) $($m.Duration)s $($m.SizeMB)MB audio=$($m.AudioTracks) | $($r.Kv['frames']) startup=$($r.Kv['startup_ms'])ms stop=$($r.Kv['stop_ms'])ms"
    if ($checks) { $detail = ($checks -join "; ") + " || " + $detail }
    Add-Result $id $name ($checks.Count -eq 0) $detail
    return $r
}

# 720p from the 4:3 runner screen keeps aspect: width = round(w * 720 / h)
$w720 = [int]([math]::Round($screen.Width * 720 / $screen.Height / 2) * 2)

"=== Probe"
$p = Invoke-Cli @("probe", "--codecs", "h264,hevc,av1", "--check-args")
$p.Out | ForEach-Object { "  $_" }
Add-Result "T01" "Encoder probe: every generated argument set is accepted by FFmpeg" ($p.Code -eq 0) ("best h264: " + $p.Kv['best_h264'])

"=== Recordings"
Test-Recording "T02" "GDI capture, x264, 720p30" @("--method", "gdi", "--encoder", "software", "--res", "720", "--fps", "30", "--seconds", "5") $w720 720 30 4.5 0 | Out-Null
Test-Recording "T03" "GDI capture, Media Foundation (CPU), native 30fps, audio" @("--method", "gdi", "--encoder", "mf", "--res", "native", "--fps", "30", "--seconds", "5", "--audio-test") $screen.Width $screen.Height 30 4.5 1 | Out-Null
Test-Recording "T04" "GDI capture, x264, native 60fps" @("--method", "gdi", "--encoder", "software", "--fps", "60", "--seconds", "5") $screen.Width $screen.Height 60 4.5 0 | Out-Null
$auto = Test-Recording "T05" "Automatic capture fallback (WGC -> DDA -> GDI)" @("--method", "auto", "--encoder", "software", "--fps", "30", "--seconds", "4") $screen.Width $screen.Height 30 3.5 0
"  fallbacks: " + (($auto.Out | Where-Object { $_ -like "fallback_from=*" }) -join " | ")
Test-Recording "T06" "MKV container" @("--method", "gdi", "--encoder", "software", "--container", "mkv", "--fps", "30", "--seconds", "4") $screen.Width $screen.Height 30 3.5 0 | Out-Null
Test-Recording "T07" "HEVC via x265, 720p30" @("--method", "gdi", "--encoder", "software", "--codec", "hevc", "--res", "720", "--fps", "30", "--seconds", "4") $w720 720 30 3.5 0 "hevc" | Out-Null
Test-Recording "T08" "Custom 800x600 output (letterboxed)" @("--method", "gdi", "--encoder", "software", "--res", "800x600", "--fps", "24", "--seconds", "4") 800 600 24 3.5 0 | Out-Null

"=== Invalid settings are rejected"
$bad = Invoke-Cli @("record", "--out", (Join-Path $work "bad.mp4"), "--source", $src, "--method", "gdi", "--encoder", "software", "--res", "5000x5000", "--seconds", "1")
Add-Result "T09" "Oversized H.264 resolution is refused with a clear message" ($bad.Code -eq 2 -and ($bad.Out -join " ") -match "larger than") (($bad.Out | Select-Object -First 1) -join "")

"=== Instant replay"
$clip = Join-Path $work "replay.mp4"
$rr = Invoke-Cli @("replay", "--dir", (Join-Path $work "ring"), "--out", $clip, "--source", $src, "--method", "gdi", "--encoder", "software", "--fps", "30", "--replay-seconds", "10", "--run-seconds", "18", "--save-seconds", "8", "--audio-test")
$m = Get-MediaInfo $clip
$ok = $rr.Code -eq 0 -and $m -and $m.Duration -ge 7.5 -and $m.Duration -le 12.5 -and -not $m.DecodeErrors -and $m.AudioTracks -eq 1
$ringFiles = (Get-ChildItem (Join-Path $work "ring") -Filter *.ts -ErrorAction SilentlyContinue).Count
Add-Result "T10" "Instant replay saves the last 8 s from a 10 s ring (18 s run, ring stays bounded)" ($ok -and $ringFiles -le [int]$rr.Kv['wrap']) "clip=$($m.Duration)s audio=$($m.AudioTracks) save=$($rr.Kv['save_ms'])ms ring_files=$ringFiles/$($rr.Kv['wrap']) $(if ($m.DecodeErrors) { 'DECODE ERRORS' })"

"=== Repeated start/stop"
$cycleOk = $true; $cycleDetail = @()
for ($i = 1; $i -le 5; $i++) {
    $o = Join-Path $work "cycle$i.mp4"
    $c = Invoke-Cli @("record", "--out", $o, "--source", $src, "--method", "gdi", "--encoder", "software", "--res", "720", "--fps", "30", "--seconds", "2")
    $cm = Get-MediaInfo $o
    if ($c.Code -ne 0 -or -not $cm -or $cm.DecodeErrors -or $cm.Duration -lt 1.5) { $cycleOk = $false }
    $cycleDetail += "$($cm.Duration)s"
}
Start-Sleep -Seconds 1
$left = @(Get-Process ffmpeg -ErrorAction SilentlyContinue).Count
Add-Result "T11" "5 start/stop cycles, no leftover ffmpeg processes" ($cycleOk -and $left -eq 0) ("durations " + ($cycleDetail -join ", ") + " leftover=$left")

"=== Summary"
$results | Format-Table -AutoSize -Wrap | Out-String -Width 250
$failed = @($results | Where-Object Result -eq "FAIL").Count
"passed=$($results.Count - $failed) failed=$failed"
if ($failed -gt 0) { exit 1 }
