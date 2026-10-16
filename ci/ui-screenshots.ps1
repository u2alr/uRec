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

# Library clicks with the real mouse: the thumbnail, Play, Show in folder and Edit on the first
# row must each reach their action. (In 0.3/0.4 an invisible row area sat on top and took them all.)
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class UrecMouse {
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
    public static void Click(int x, int y) {
        SetCursorPos(x, y); System.Threading.Thread.Sleep(150);
        mouse_event(2, 0, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(60);
        mouse_event(4, 0, 0, 0, UIntPtr.Zero);
    }
}
"@
$hit = Join-Path $env:TEMP "urec-hitmap.txt"
Remove-Item $hit, "$hit.clicks" -ErrorAction SilentlyContinue
$env:UREC_HITMAP = $hit; $env:UREC_PAGE = "library"
$results = @()
foreach ($control in "thumbnail", "play", "reveal", "edit") {
    $proc = Start-Process $Exe -PassThru
    Start-Sleep -Seconds 6
    $pos = if (Test-Path $hit) { Get-Content $hit | Where-Object { $_ -like "$control *" } | Select-Object -First 1 } else { $null }
    if ($pos) {
        $parts = $pos -split " "
        Shot "library-before-$control"
        [UrecMouse]::Click([int]$parts[1], [int]$parts[2])
        Start-Sleep -Seconds 3
        if ($control -eq "edit") { Shot "library-click-edit" }
    }
    $got = (Test-Path "$hit.clicks") -and (@(Get-Content "$hit.clicks") -contains $control)
    $results += "$control=$(if ($got) { 'PASS' } else { 'FAIL' })$(if (-not $pos) { ' (no position)' })"
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    # Close the folder window "Show in folder" opened. (With the test hook on, Play and the
    # thumbnail record the click but don't open a player.)
    try { (New-Object -ComObject Shell.Application).Windows() | ForEach-Object { $_.Quit() } } catch {}
    Start-Sleep -Seconds 1
}
"library clicks: $($results -join ' ')"
Remove-Item env:UREC_HITMAP

# Clip editor on a real recording: open, split, play (with sound), screenshots.
$clip = Get-ChildItem $videos -Recurse -Filter *.mp4 | Sort-Object Length -Descending | Select-Object -First 1
if ($clip) {
    $env:UREC_PAGE = "library"; $env:UREC_EDIT = $clip.FullName
    $proc = Start-Process $Exe -PassThru
    Start-Sleep -Seconds 7
    Shot "editor-open"
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    Start-Sleep -Seconds 1
    $env:UREC_EDIT_PLAY = "1"
    $proc = Start-Process $Exe -PassThru
    Start-Sleep -Seconds 5
    Shot "editor-playing"
    "editor alive while playing=$(-not $proc.HasExited)"
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    Remove-Item env:UREC_EDIT, env:UREC_EDIT_PLAY

    # Preview sound: play a 1 kHz tone clip in the editor and record what the speakers output.
    $tone = Join-Path $env:TEMP "urec-editor-tone.mp4"
    & $env:UREC_FFMPEG -hide_banner -loglevel error -y -f lavfi -i "testsrc2=s=640x360:r=30:d=12" -f lavfi -i "sine=f=1000:d=12" -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest $tone
    $env:UREC_EDIT = $tone; $env:UREC_EDIT_PLAY = "1"; $env:UREC_PAGE = "library"
    $proc = Start-Process $Exe -PassThru
    Start-Sleep -Seconds 3
    $heard = Join-Path $env:TEMP "urec-editor-heard.mp4"
    if ($Cli) { & $Cli record --out $heard --method gdi --encoder software --res 720 --fps 15 --seconds 4 --audio system | Out-Null }
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    Remove-Item env:UREC_EDIT, env:UREC_EDIT_PLAY
    $vd = (& $env:UREC_FFMPEG -hide_banner -i $heard -af volumedetect -vn -f null - 2>&1 | Out-String)
    $mean = if ($vd -match "mean_volume: (-?[0-9.]+) dB") { [double]$Matches[1] } else { -999 }
    "editor preview sound: mean $mean dB on the output device (silence is about -91 dB)"
    Select-String -Path (Join-Path $env:LOCALAPPDATA "uRec\logs\urec.log") -Pattern "editor:" | Select-Object -Last 6 | ForEach-Object { "  log: " + $_.Line }

    Start-Sleep -Seconds 1
}


# Long window title in Settings > Recording (must be shortened, not overlap the label).
$longTitle = "A very long window title that keeps going and going, for testing the window picker in uRec"
$np = Start-Process powershell.exe -ArgumentList "-NoProfile", "-Command", "Add-Type -AssemblyName System.Windows.Forms; `$f = New-Object System.Windows.Forms.Form; `$f.Text = '$longTitle'; `$f.ShowDialog()" -PassThru
$title = $null
foreach ($i in 1..20) { Start-Sleep -Milliseconds 500; $title = (Get-Process -Id $np.Id -ErrorAction SilentlyContinue).MainWindowTitle; if ($title) { break } }
$settingsPath = Join-Path $env:APPDATA "uRec\settings.json"
if ((Test-Path $settingsPath) -and $title) {
    $backup = Get-Content $settingsPath -Raw
    $sj = $backup | ConvertFrom-Json
    $sj.capture.kind = "window"; $sj.capture.window_title = $title; $sj.capture.window_exe = "powershell.exe"
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

# Start-up guard: a start that never drew a window must make the next start use OpenGL.
$dir = Join-Path $env:LOCALAPPDATA "uRec"
$savedRenderer = $env:UREC_RENDERER
try {
    Remove-Item env:UREC_RENDERER -ErrorAction SilentlyContinue
    Remove-Item env:UREC_PAGE -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $dir | Out-Null
    Set-Content (Join-Path $dir "renderer-trial.txt") "wgpu"
    $proc = Start-Process $Exe -PassThru
    Start-Sleep -Seconds 6
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    $lg = Get-Content (Join-Path $dir "logs\urec.log") -Raw
    $switched = $lg -match "using OpenGL from now on"
    $saved = (Test-Path (Join-Path $dir "renderer.txt")) -and ((Get-Content (Join-Path $dir "renderer.txt") -Raw).Trim() -eq "glow")
    "renderer guard: switched=$switched saved=$saved"
} finally {
    Remove-Item (Join-Path $dir "renderer.txt"), (Join-Path $dir "renderer-trial.txt") -ErrorAction SilentlyContinue
    $env:UREC_RENDERER = $savedRenderer
}

$log = Join-Path $env:LOCALAPPDATA "uRec\logs\urec.log"
if (Test-Path $log) { Copy-Item $log (Join-Path $Out "urec.log") }

# Fail the step only for a real UI failure (not the exit code of whatever command ran last).
if ($results -match "FAIL") { exit 1 } else { exit 0 }
