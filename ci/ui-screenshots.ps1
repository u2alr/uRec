# Launches the real app on each page and captures the screen, so the UI can be reviewed.
param([Parameter(Mandatory)][string]$Exe, [string]$Out = "screenshots")
$ErrorActionPreference = "Continue"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
New-Item -ItemType Directory -Force $Out | Out-Null
$env:UREC_FFMPEG = (& "$PSScriptRoot\get-ffmpeg.ps1" | Select-Object -Last 1)
$pages = "home", "library", "settings:general", "settings:recording", "settings:video", "settings:audio", "settings:instantreplay", "settings:hotkeys", "settings:storage", "settings:appearance", "settings:advanced"
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
foreach ($pg in $pages) {
    $env:UREC_PAGE = $pg
    $proc = Start-Process $Exe -PassThru
    Start-Sleep -Seconds $(if ($pg -eq "home") { 9 } else { 5 })
    $alive = -not $proc.HasExited
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
    $file = Join-Path $Out ("{0}.png" -f ($pg -replace ':', '-'))
    $bmp.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    "$pg alive=$alive -> $file"
    if ($alive) { Stop-Process -Id $proc.Id -Force } else { "  exit code $($proc.ExitCode)" }
    Start-Sleep -Seconds 1
}
$log = Join-Path $env:LOCALAPPDATA "uRec\logs\urec.log"
if (Test-Path $log) { Copy-Item $log (Join-Path $Out "urec.log") }
