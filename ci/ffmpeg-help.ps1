# Dumps exact option help for the capture filters and encoders we use.
$ProgressPreference = "SilentlyContinue"
$ff = & "$PSScriptRoot\get-ffmpeg.ps1" | Select-Object -Last 1
"source: $(Get-Content (Join-Path (Split-Path $ff) 'SOURCE.txt'))"
"sha256: $((Get-FileHash $ff -Algorithm SHA256).Hash)"
foreach ($u in "https://www.gyan.dev/ffmpeg/builds/release-version", "https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-9.0.2-essentials_build.zip") {
  try { $r = Invoke-WebRequest $u -Method Head -UseBasicParsing; "HEAD $u -> $($r.StatusCode) len=$($r.Headers['Content-Length'])" } catch { "HEAD $u -> $($_.Exception.Message)" }
}
try { "release-version: $((Invoke-WebRequest https://www.gyan.dev/ffmpeg/builds/release-version -UseBasicParsing).Content)" } catch {}
foreach ($h in "filter=gfxcapture","filter=ddagrab","filter=scale_d3d11","filter=hwmap","encoder=h264_nvenc","encoder=h264_amf","encoder=h264_qsv","encoder=h264_mf","encoder=libx264","encoder=hevc_amf","encoder=av1_amf","demuxer=gdigrab","muxer=segment") {
  "################ $h"
  & $ff -hide_banner -h $h 2>&1
}
"################ encoders list"
& $ff -hide_banner -encoders 2>$null | Select-String "264|265|hevc|av1|aac|opus"
