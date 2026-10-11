# Reports what this Windows machine can capture and encode. Writes to stdout.
$ErrorActionPreference = "Continue"
$ProgressPreference = "SilentlyContinue"

"=== System"
Get-CimInstance Win32_OperatingSystem | ForEach-Object { "$($_.Caption) $($_.Version) build $($_.BuildNumber)" }
Get-CimInstance Win32_VideoController | ForEach-Object { "GPU: $($_.Name) | $($_.VideoModeDescription)" }
Get-CimInstance Win32_Processor | ForEach-Object { "CPU: $($_.Name) cores=$($_.NumberOfCores)" }
"Session=$([System.Diagnostics.Process]::GetCurrentProcess().SessionId) Interactive=$([Environment]::UserInteractive)"
"Audio endpoints:"; Get-PnpDevice -Class AudioEndpoint -ErrorAction SilentlyContinue | ForEach-Object { "  $($_.Status) $($_.FriendlyName)" }
"rust: $(rustc --version 2>&1) | $(cargo --version 2>&1)"
"targets: $((rustup target list --installed 2>&1) -join ', ')"

"=== FFmpeg"
$ff = & "$PSScriptRoot\get-ffmpeg.ps1" | Select-Object -Last 1
$fp = $ff -replace 'ffmpeg\.exe$', 'ffprobe.exe'
& $ff -hide_banner -version 2>&1 | Select-Object -First 1
"filters:"; & $ff -hide_banner -filters 2>$null | Select-String "gfxcapture|ddagrab|hwdownload|scale_d3d11|hwupload" | ForEach-Object { "  $_" }

"=== Encoders (10-frame test encode)"
foreach ($e in "h264_nvenc","h264_amf","h264_qsv","h264_mf","libx264","libopenh264","hevc_nvenc","hevc_amf","hevc_qsv","av1_nvenc","av1_amf","av1_qsv","libsvtav1") {
    $o = & $ff -hide_banner -loglevel error -f lavfi -i color=black:s=1280x720:r=30 -frames:v 10 -pix_fmt yuv420p -c:v $e -f null - 2>&1
    "{0,-12} exit={1} {2}" -f $e, $LASTEXITCODE, ((($o | Select-Object -First 1) -as [string]) -replace '\s+', ' ')
}

"=== Capture (3 s each)"
$tests = [ordered]@{
    "gfxcapture" = @("-f","lavfi","-i","gfxcapture=monitor_idx=0:max_framerate=30,hwdownload,format=bgra")
    "ddagrab"    = @("-f","lavfi","-i","ddagrab=output_idx=0:framerate=30,hwdownload,format=bgra")
    "gdigrab"    = @("-f","gdigrab","-framerate","30","-i","desktop")
}
foreach ($k in $tests.Keys) {
    $out = Join-Path $env:TEMP "cap_$k.mp4"
    $log = & $ff -hide_banner -loglevel warning -y @($tests[$k]) -t 3 -c:v libx264 -preset ultrafast -pix_fmt yuv420p $out 2>&1
    $code = $LASTEXITCODE
    $info = if (Test-Path $out) { (& $fp -v error -count_frames -select_streams v:0 -show_entries stream=width,height,nb_read_frames -of csv=p=0 $out) -join " " } else { "no file" }
    "{0,-11} exit={1} -> {2}" -f $k, $code, $info
    $log | Select-Object -First 3 | ForEach-Object { "    $_" }
}

"=== WASAPI loopback via dshow list (informational)"
& $ff -hide_banner -list_devices true -f dshow -i dummy 2>&1 | Select-Object -First 8 | ForEach-Object { "  $_" }
