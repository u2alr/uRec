# Launches the real app, captures every page (light and dark), and drives a recording with the
# real global hotkey (Alt+F9) to check hotkey -> recording -> saved file end to end.
param([Parameter(Mandatory)][string]$Exe, [string]$Cli = "", [string]$Out = "screenshots")
$ErrorActionPreference = "Continue"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
New-Item -ItemType Directory -Force $Out | Out-Null
$env:UREC_FFMPEG = (& "$PSScriptRoot\get-ffmpeg.ps1" | Select-Object -Last 1)
$env:RUST_BACKTRACE = "1"
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$videos = Join-Path $env:USERPROFILE "Videos\uRec"

function Shot($name) {
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
    $bmp.Save((Join-Path $Out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

# A sample recording so the library has something to show.
if ($Cli -and (Test-Path $Cli)) {
    New-Item -ItemType Directory -Force (Join-Path $videos "Desktop") | Out-Null
    & $Cli record --out (Join-Path $videos "Desktop\Desktop 2026-10-11 10.00.00.mp4") --method gdi --encoder software --res 720 --fps 30 --seconds 4 | Out-Null
    "sample recording: $(Test-Path (Join-Path $videos 'Desktop\Desktop 2026-10-11 10.00.00.mp4'))"
}

# Hotkey test: start the app, press Alt+F9 to start, wait, press again to stop.
$env:UREC_PAGE = "home"
$before = @(Get-ChildItem $videos -Recurse -Filter *.mp4 -ErrorAction SilentlyContinue).Count
$proc = Start-Process $Exe -PassThru
Start-Sleep -Seconds 9
Shot "home-ready"
$wsh = New-Object -ComObject WScript.Shell
$wsh.SendKeys("%{F9}")
Start-Sleep -Seconds 6
Shot "home-recording"
$wsh.SendKeys("%{F9}")
Start-Sleep -Seconds 6
Shot "home-saved"
$after = @(Get-ChildItem $videos -Recurse -Filter *.mp4 -ErrorAction SilentlyContinue)
$new = $after | Sort-Object LastWriteTime | Select-Object -Last 1
"hotkey recording: files before=$before after=$($after.Count) newest=$($new.Name) size=$($new.Length)"
if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force } else { "app exited: $($proc.ExitCode)" }
Start-Sleep -Seconds 2

# First-run setup: every step, then the real 5-second test recording started by itself.
foreach ($step in 0, 1, 2) {
    $env:UREC_PAGE = "setup:$step"
    $proc = Start-Process $Exe -PassThru
    Start-Sleep -Seconds 7
    Shot "setup-$step"
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    Start-Sleep -Seconds 1
}
$env:UREC_PAGE = "setup:3"
$env:UREC_SETUP_AUTOTEST = "1"
$proc = Start-Process $Exe -PassThru
Start-Sleep -Seconds 30
Shot "setup-3-test"
if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
Remove-Item env:UREC_SETUP_AUTOTEST
Start-Sleep -Seconds 1
$log = Join-Path $env:LOCALAPPDATA "uRec\logs\urec.log"
Select-String -Path $log -Pattern "setup test:" | Select-Object -Last 10 | ForEach-Object { "setup test log: " + ($_.Line -replace '^\S+ \S+ ', '') }

$pages = "home", "library", "settings:general", "settings:recording", "settings:video", "settings:audio", "settings:instantreplay", "settings:hotkeys", "settings:storage", "settings:appearance", "settings:advanced"
foreach ($theme in "light", "dark") {
    $env:UREC_THEME = $theme
    foreach ($pg in $pages) {
        if ($theme -eq "dark" -and $pg -notin "home", "library", "settings:video", "settings:audio") { continue }
        $env:UREC_PAGE = $pg
        $proc = Start-Process $Exe -PassThru
        Start-Sleep -Seconds 5
        Shot ("{0}-{1}" -f $theme, ($pg -replace ':', '-'))
        "$theme $pg alive=$(-not $proc.HasExited)"
        if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force } else { "  exit code $($proc.ExitCode)" }
        Start-Sleep -Seconds 1
    }
}
Remove-Item env:UREC_THEME

# Long window title in Settings > Recording (must be shortened, not overlap the label).
$longName = Join-Path $env:TEMP "A very long document name that keeps going and going for testing the window picker.txt"
"x" | Out-File $longName
$np = Start-Process notepad.exe -ArgumentList "`"$longName`"" -PassThru
Start-Sleep -Seconds 3
$title = (Get-Process -Id $np.Id).MainWindowTitle
$settingsPath = Join-Path $env:APPDATA "uRec\settings.json"
if ((Test-Path $settingsPath) -and $title) {
    $backup = Get-Content $settingsPath -Raw
    $sj = $backup | ConvertFrom-Json
    $sj.capture.kind = "window"; $sj.capture.window_title = $title; $sj.capture.window_exe = "notepad.exe"
    [IO.File]::WriteAllText($settingsPath, ($sj | ConvertTo-Json -Depth 8))
    foreach ($pg in "settings:recording", "home") {
        $env:UREC_PAGE = $pg
        $proc = Start-Process $Exe -PassThru
        Start-Sleep -Seconds 5
        Shot ("long-title-" + ($pg -replace ':', '-'))
        if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
        Start-Sleep -Seconds 1
    }
    [IO.File]::WriteAllText($settingsPath, $backup)
    "long window title: $($title.Length) chars"
}
Stop-Process -Id $np.Id -Force -ErrorAction SilentlyContinue

$log = Join-Path $env:LOCALAPPDATA "uRec\logs\urec.log"
if (Test-Path $log) { Copy-Item $log (Join-Path $Out "urec.log") }
