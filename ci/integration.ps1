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
$probeQ = Invoke-Cli @("probe", "--check-args", "--rc", "quality")
$qErr = @($probeQ.Out | Where-Object { $_ -match "arg_error=true" })
Add-Result "T01b" "Constant-quality argument sets (x264/x265 CRF, NVENC CQ, AMF CQP, QSV ICQ, MF quality) are accepted by FFmpeg" ($probeQ.Code -eq 0 -and $qErr.Count -eq 0) ("argument errors: " + $qErr.Count + " | " + (($probeQ.Out | Where-Object { $_ -match "ok=true" } | ForEach-Object { ($_ -split "\s+")[0] }) -join ", "))

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

# Rapid repeated saves: 4 saves at the same instant to the same name (hotkey mashing).
$rapidDir = Join-Path $work "rapid"
New-Item -ItemType Directory -Force $rapidDir | Out-Null
$rapid = Join-Path $rapidDir "Desktop Replay.mp4"
$r4 = Invoke-Cli @("replay", "--dir", (Join-Path $work "ring4"), "--out", $rapid, "--source", $src, "--method", "gdi", "--encoder", "software", "--fps", "30", "--replay-seconds", "10", "--run-seconds", "14", "--save-seconds", "6", "--audio-test", "--saves", "4")
$clips = @(Get-ChildItem $rapidDir -Filter *.mp4 | Where-Object { $_.Name -notlike "*.urec-tmp.mp4" })
$temps = @(Get-ChildItem $rapidDir -Filter "*.urec-tmp.mp4").Count
$bad = @($clips | ForEach-Object { $mm = Get-MediaInfo $_.FullName; if (-not $mm -or $mm.DecodeErrors -or $mm.Duration -lt 5.5 -or $mm.AudioTracks -ne 1) { $_.Name } })
Add-Result "T19" "4 simultaneous replay saves: 4 separate complete clips, none overwritten, no leftovers" ($r4.Code -eq 0 -and $clips.Count -eq 4 -and $bad.Count -eq 0 -and $temps -eq 0 -and $r4.Kv['stage_leftovers'] -eq '0') "clips=$($clips.Count) [$(($clips.Name) -join ', ')] bad=[$($bad -join ', ')] temp=$temps stages_left=$($r4.Kv['stage_leftovers']) total=$($r4.Kv['save_ms'])ms"

# Long run: memory of the capture/encoder process and the CLI host while Instant Replay runs 150 s.
$ringL = Join-Path $work "ringL"
$clipL = Join-Path $work "long.mp4"
$pl = Start-Process $Cli -ArgumentList "replay", "--dir", $ringL, "--out", $clipL, "--source", $src, "--method", "gdi", "--encoder", "software", "--fps", "30", "--replay-seconds", "30", "--run-seconds", "150", "--save-seconds", "20", "--audio-test" -PassThru -WindowStyle Hidden
$samples = @()
foreach ($t in 20, 50, 80, 110, 140) {
    while ($pl.StartTime -and ((Get-Date) - $pl.StartTime).TotalSeconds -lt $t) { Start-Sleep -Milliseconds 500 }
    # (not $ff: that's the FFmpeg path the other tests use)
    $ffProc = Get-Process ffmpeg -ErrorAction SilentlyContinue | Sort-Object StartTime | Select-Object -Last 1
    $pl.Refresh()
    $samples += [pscustomobject]@{ T = $t; Ff = [math]::Round($ffProc.PrivateMemorySize64 / 1MB, 1); Cli = [math]::Round($pl.PrivateMemorySize64 / 1MB, 1); Ring = @(Get-ChildItem $ringL -Filter *.ts -ErrorAction SilentlyContinue).Count }
}
$pl.WaitForExit(60000) | Out-Null
$mL = Get-MediaInfo $clipL
$grow = $samples[-1].Ff - $samples[0].Ff
$growCli = $samples[-1].Cli - $samples[0].Cli
Add-Result "T20" "150 s Instant Replay: memory stays flat, ring stays bounded, save still works" ($grow -lt 30 -and $growCli -lt 10 -and ($samples | Measure-Object Ring -Maximum).Maximum -le 19 -and $mL -and -not $mL.DecodeErrors) ("ffmpeg private MB " + (($samples | ForEach-Object { "$($_.T)s=$($_.Ff)" }) -join " ") + " | host " + (($samples | ForEach-Object { $_.Cli }) -join "/") + " | ring max $(($samples | Measure-Object Ring -Maximum).Maximum)/19 | clip $($mL.Duration)s")

# Update signatures: a throwaway P-256 key signs a file; uRec's CNG check must accept it and
# refuse a changed file, a changed signature and another key.
$ec = [System.Security.Cryptography.ECDsa]::Create([System.Security.Cryptography.ECCurve+NamedCurves]::nistP256)
$qp = $ec.ExportParameters($false)
$keyHex = (($qp.Q.X + $qp.Q.Y) | ForEach-Object { $_.ToString("x2") }) -join ""
$sumsF = Join-Path $work "SHA256SUMS.txt"; $sigF = Join-Path $work "SHA256SUMS.txt.sig"
"abc123  uRec-9.9.9-setup.exe`n" | Out-File -NoNewline -Encoding ascii $sumsF
$sigHex = (($ec.SignData([IO.File]::ReadAllBytes($sumsF), [System.Security.Cryptography.HashAlgorithmName]::SHA256)) | ForEach-Object { $_.ToString("x2") }) -join ""
$sigHex | Out-File -NoNewline -Encoding ascii $sigF
$good = Invoke-Cli @("verify-sig", "--key", $keyHex, "--file", $sumsF, "--sig", $sigF)
"tampered  uRec-9.9.9-setup.exe`n" | Out-File -NoNewline -Encoding ascii (Join-Path $work "tampered.txt")
$badFile = Invoke-Cli @("verify-sig", "--key", $keyHex, "--file", (Join-Path $work "tampered.txt"), "--sig", $sigF)
$flipped = ($sigHex.Substring(0, 10) + $(if ($sigHex[10] -eq 'a') { 'b' } else { 'a' }) + $sigHex.Substring(11))
$flipped | Out-File -NoNewline -Encoding ascii (Join-Path $work "flipped.sig")
$badSig = Invoke-Cli @("verify-sig", "--key", $keyHex, "--file", $sumsF, "--sig", (Join-Path $work "flipped.sig"))
$other = [System.Security.Cryptography.ECDsa]::Create([System.Security.Cryptography.ECCurve+NamedCurves]::nistP256).ExportParameters($false)
$otherHex = (($other.Q.X + $other.Q.Y) | ForEach-Object { $_.ToString("x2") }) -join ""
$badKey = Invoke-Cli @("verify-sig", "--key", $otherHex, "--file", $sumsF, "--sig", $sigF)
Add-Result "T21" "Signed updates: valid signature accepted; changed file, changed signature and other key refused" ($good.Kv['verified'] -eq 'true' -and $badFile.Kv['verified'] -eq 'false' -and $badSig.Kv['verified'] -eq 'false' -and $badKey.Kv['verified'] -eq 'false') "good=$($good.Kv['verified']) tampered_file=$($badFile.Kv['verified']) flipped_sig=$($badSig.Kv['verified']) other_key=$($badKey.Kv['verified'])"

# Constant quality vs VBR on the same mostly-still desktop: size and similarity (SSIM between the two).
$vbrF = Join-Path $work "T22-vbr.mp4"; $cqF = Join-Path $work "T22-cq.mp4"
$rv = Invoke-Cli @("record", "--out", $vbrF, "--source", $src, "--method", "gdi", "--encoder", "software", "--res", "720", "--fps", "30", "--seconds", "8", "--bitrate", "6000", "--rc", "vbr")
$rq = Invoke-Cli @("record", "--out", $cqF, "--source", $src, "--method", "gdi", "--encoder", "software", "--res", "720", "--fps", "30", "--seconds", "8", "--bitrate", "6000", "--rc", "quality", "--quality", "7")
$mv = Get-MediaInfo $vbrF; $mq = Get-MediaInfo $cqF
$ssim = ((& $ff -hide_banner -i $cqF -i $vbrF -lavfi "[0:v][1:v]scale2ref[a][b];[a][b]ssim" -f null - 2>&1 | Out-String) -split "`n" | Where-Object { $_ -match "SSIM" } | Select-Object -Last 1)
$ssimAll = if ($ssim -match "All:([0-9.]+)") { [double]$Matches[1] } else { 0 }
$ratio = if ($mv -and $mv.SizeMB -gt 0) { [math]::Round($mq.SizeMB / $mv.SizeMB, 2) } else { 0 }
Add-Result "T22" "Constant quality (level 7) vs VBR 6 Mbps, same desktop: both decode cleanly; size and similarity reported" ($rv.Code -eq 0 -and $rq.Code -eq 0 -and $mv -and $mq -and -not $mv.DecodeErrors -and -not $mq.DecodeErrors) "VBR $($mv.SizeMB) MB, constant quality $($mq.SizeMB) MB (ratio $ratio); SSIM between them $ssimAll"

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

"=== Devices"
$d = & $Cli devices 2>&1 | ForEach-Object { "$_" }
$d | ForEach-Object { "  $_" }
$mon = $d | Where-Object { $_ -like "monitor 0:*" } | Select-Object -First 1
Add-Result "T12" "Monitor enumeration finds the primary screen at its real size" ($mon -match "width: $($screen.Width), height: $($screen.Height)" -and $mon -match "primary=true") "$mon"
$hasOut = @($d | Where-Object { $_ -like "audio_out:*" }).Count
$hasIn = @($d | Where-Object { $_ -like "audio_in:*" }).Count
"  audio outputs=$hasOut inputs=$hasIn"

function Get-MeanVolume([string]$path) {
    $o = & $ff -hide_banner -i $path -map 0:a:0 -af volumedetect -f null - 2>&1 | Out-String
    if ($o -match "mean_volume: (-?[\d.]+) dB") { return [double]$Matches[1] } else { return $null }
}

"=== Audio"
if ($hasOut -gt 0) {
    $tone = Start-Process pwsh -ArgumentList "-NoProfile", "-File", "$PSScriptRoot\play-tone.ps1", "-Seconds", "14" -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 2
    $out = Join-Path $work "T13.mp4"
    $r = Invoke-Cli @("record", "--out", $out, "--source", $src, "--method", "gdi", "--encoder", "software", "--fps", "30", "--seconds", "6", "--audio", "system")
    $m = Get-MediaInfo $out
    $vol = if ($m -and $m.AudioTracks -ge 1) { Get-MeanVolume $out } else { $null }
    $ok = $r.Code -eq 0 -and $m.AudioTracks -eq 1 -and $vol -ne $null -and $vol -gt -45 -and -not $m.DecodeErrors
    Add-Result "T13" "Game/desktop sound (WASAPI loopback) captures a playing 1 kHz tone" $ok "state=$($r.Kv['audio_system_state']) end=$($r.Kv['audio_end_system']) tracks=$($m.AudioTracks) mean=$vol dB dur=$($m.Duration)s"
    $tone | Wait-Process -Timeout 20 -ErrorAction SilentlyContinue

    $out = Join-Path $work "T14.mp4"
    $r = Invoke-Cli @("record", "--out", $out, "--source", $src, "--method", "gdi", "--encoder", "software", "--fps", "30", "--seconds", "4", "--audio", "both", "--layout", "separate")
    $m = Get-MediaInfo $out
    $micActive = "$($r.Kv['audio_mic_state'])" -like "Active*"
    $expect = 1 + [int]$micActive
    Add-Result "T14" "Separate tracks: one per working source (mic present: $micActive)" ($r.Code -eq 0 -and $m.AudioTracks -eq $expect -and -not $m.DecodeErrors) "tracks=$($m.AudioTracks) expected=$expect mic=$($r.Kv['audio_mic_state'])"
} else {
    Add-Result "T13" "Game/desktop sound (WASAPI loopback)" $false "no audio output device on this machine"
}

$out = Join-Path $work "T15.mp4"
$r = Invoke-Cli @("record", "--out", $out, "--source", $src, "--method", "gdi", "--encoder", "software", "--fps", "30", "--seconds", "3", "--audio", "mic")
$m = Get-MediaInfo $out
$micState = "$($r.Kv['audio_mic_state'])"
$consistent = ($micState -like "Active*" -and $m.AudioTracks -eq 1) -or ($micState -like "Failed*" -and $m.AudioTracks -eq 0)
Add-Result "T15" "Microphone: recorded only if it really initialized; recording works either way" ($r.Code -eq 0 -and $consistent -and -not $m.DecodeErrors) "mic=$micState tracks=$($m.AudioTracks)"

"=== Audio/video sync"
if ($hasOut -gt 0) {
    $flash = Start-Process pwsh -ArgumentList "-NoProfile", "-STA", "-File", "$PSScriptRoot\av-flash.ps1", "-Seconds", "16" -PassThru
    Start-Sleep -Seconds 2
    $out = Join-Path $work "T17.mp4"
    $r = Invoke-Cli @("record", "--out", $out, "--source", $src, "--method", "gdi", "--encoder", "software", "--res", "720", "--fps", "30", "--seconds", "11", "--audio", "system")
    $flash | Wait-Process -Timeout 25 -ErrorAction SilentlyContinue
    # Video onsets: frames where average brightness jumps from dark to bright.
    Push-Location $work
    & $ff -hide_banner -v error -i $out -vf "signalstats,metadata=print:key=lavfi.signalstats.YAVG:file=yavg.txt" -f null - 2>&1 | Out-Null
    Pop-Location
    $yfile = Join-Path $work "yavg.txt"
    $vOn = @(); $prevBright = $false; $t = 0.0
    foreach ($l in (Get-Content $yfile -ErrorAction SilentlyContinue)) {
        if ($l -match "pts_time:([\d.]+)") { $t = [double]$Matches[1] }
        if ($l -match "YAVG=([\d.]+)") { $bright = [double]$Matches[1] -gt 100; if ($bright -and -not $prevBright) { $vOn += $t }; $prevBright = $bright }
    }
    # Audio onsets: ends of silence.
    $sd = & $ff -hide_banner -i $out -map 0:a:0 -af "silencedetect=n=-35dB:d=0.3" -f null - 2>&1 | Out-String
    $aOn = @([regex]::Matches($sd, "silence_end: ([\d.]+)") | ForEach-Object { [double]$_.Groups[1].Value })
    "  video flash onsets: $($vOn -join ', ')"
    "  audio beep onsets:  $($aOn -join ', ')"
    $offs = @()
    foreach ($v in $vOn) {
        $near = $aOn | Sort-Object { [math]::Abs($_ - $v) } | Select-Object -First 1
        if ($near -ne $null -and [math]::Abs($near - $v) -lt 0.8) { $offs += [math]::Round(($near - $v) * 1000) }
    }
    $sorted = $offs | Sort-Object
    $median = if ($sorted.Count) { $sorted[[int][math]::Floor($sorted.Count / 2)] } else { $null }
    $ok = $sorted.Count -ge 3 -and [math]::Abs($median) -le 100
    Add-Result "T17" "Audio/video sync: flash + beep every 2 s, audio offset within 100 ms" $ok "pairs=$($sorted.Count) offsets_ms=[$($offs -join ', ')] median=$median ms (positive = audio later)"
} else {
    Add-Result "T17" "Audio/video sync" $false "no audio output device"
}

"=== Window capture"
$np = Start-Process notepad -PassThru
# Wait until Notepad's window really exists (it can take a few seconds on a cold runner).
for ($i = 0; $i -lt 40; $i++) {
    if (@(Get-Process notepad -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 }).Count -gt 0) { break }
    Start-Sleep -Milliseconds 250
}
Start-Sleep -Seconds 1
$out = Join-Path $work "T16.mp4"
$r = Invoke-Cli @("record", "--out", $out, "--window", "notepad", "--method", "gdi", "--encoder", "software", "--fps", "30", "--seconds", "3")
$m = Get-MediaInfo $out
$np | Stop-Process -Force -ErrorAction SilentlyContinue
Add-Result "T16" "Window capture (Notepad) records just that window" ($r.Code -eq 0 -and $m -and $m.Width -lt $screen.Width -and -not $m.DecodeErrors) "$($m.Width)x$($m.Height) via $($r.Kv['method']) $(($r.Out | Select-Object -Last 2) -join ' ')"

"=== Show in folder / Open folder"
function Get-ExplorerFolders {
    $shell = New-Object -ComObject Shell.Application
    @($shell.Windows() | ForEach-Object { try { [Uri]::UnescapeDataString(([Uri]$_.LocationURL).LocalPath) } catch {} })
}
Get-Process explorer -ErrorAction SilentlyContinue | Out-Null
$target = Join-Path $work "Folder With Spaces"
New-Item -ItemType Directory -Force $target | Out-Null
$file = Join-Path $target "My Clip 2026-10-11 10.00.00.mp4"
Copy-Item (Join-Path $work "T02.mp4") $file
& $Cli reveal $file | Out-Null
Start-Sleep -Seconds 4
$open1 = Get-ExplorerFolders
& $Cli reveal (Join-Path $work "ring") | Out-Null
Start-Sleep -Seconds 4
$open2 = Get-ExplorerFolders
$ok1 = $open1 -contains $target
$ok2 = $open2 -contains (Join-Path $work "ring")
Add-Result "T18" "Show in folder (path with spaces) and Open folder open the right Explorer folder" ($ok1 -and $ok2) ("explorer windows: " + (($open2 | Select-Object -Unique) -join " | "))

"=== Summary"
$results | Format-Table -AutoSize -Wrap | Out-String -Width 250
$failed = @($results | Where-Object Result -eq "FAIL").Count
"passed=$($results.Count - $failed) failed=$failed"
if ($failed -gt 0) { exit 1 }
