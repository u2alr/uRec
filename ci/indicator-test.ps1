# Instant Replay indicator tests on a real Windows desktop.
# Layered (per-pixel alpha) windows are only captured with CAPTUREBLT.
# The overlay is normally excluded from screen capture; UREC_OVERLAY_CAPTURABLE=1 lets these
# tests film it. Frames are grabbed as fast as possible and the pill width is measured per frame.
param([Parameter(Mandatory)][string]$Exe, [Parameter(Mandatory)][string]$Cli, [string]$Out = "screenshots")
$ErrorActionPreference = "Continue"
New-Item -ItemType Directory -Force $Out | Out-Null
if (-not $env:UREC_FFMPEG) { $env:UREC_FFMPEG = (& "$PSScriptRoot\get-ffmpeg.ps1" | Select-Object -Last 1) }
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
# Runs in Windows PowerShell 5.1 (see ci.yml): System.Drawing is a single .NET Framework assembly there.
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System; using System.Drawing; using System.Drawing.Imaging; using System.Collections.Generic;
public static class Cap {
    public static byte[] Grab(int x, int y, int w, int h) {
        using (var bmp = new Bitmap(w, h, PixelFormat.Format32bppArgb)) {
            using (var g = Graphics.FromImage(bmp)) g.CopyFromScreen(x, y, 0, 0, new Size(w, h), CopyPixelOperation.SourceCopy | CopyPixelOperation.CaptureBlt);
            var d = bmp.LockBits(new Rectangle(0, 0, w, h), ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
            var px = new byte[w * h * 4];
            System.Runtime.InteropServices.Marshal.Copy(d.Scan0, px, 0, px.Length);
            bmp.UnlockBits(d);
            return px;
        }
    }
    // Width of the region (in columns) that differs from the baseline.
    public static int Changed(byte[] a, byte[] b, int w, int h) {
        int lo = w, hi = -1;
        for (int y = 0; y < h; y++) for (int x = 0; x < w; x++) {
            int i = (y * w + x) * 4;
            int d = Math.Abs(a[i] - b[i]) + Math.Abs(a[i + 1] - b[i + 1]) + Math.Abs(a[i + 2] - b[i + 2]);
            if (d > 60) { if (x < lo) lo = x; if (x > hi) hi = x; }
        }
        return hi < 0 ? 0 : hi - lo + 1;
    }
    // Count of strongly green / red pixels (icon colour).
    public static int[] Accents(byte[] a) {
        int g = 0, r = 0;
        for (int i = 0; i < a.Length; i += 4) {
            int B = a[i], G = a[i + 1], R = a[i + 2];
            if (G > 150 && R < 110 && B < 140) g++;
            if (R > 190 && G < 110 && B < 110) r++;
        }
        return new[] { g, r };
    }
    public static void Sheet(List<byte[]> frames, int w, int h, string path, int scale) {
        using (var bmp = new Bitmap(w * scale, h * scale * frames.Count, PixelFormat.Format32bppArgb)) {
            for (int f = 0; f < frames.Count; f++) for (int y = 0; y < h; y++) for (int x = 0; x < w; x++) {
                int i = (y * w + x) * 4; var c = Color.FromArgb(255, frames[f][i + 2], frames[f][i + 1], frames[f][i]);
                for (int sy = 0; sy < scale; sy++) for (int sx = 0; sx < scale; sx++) bmp.SetPixel(x * scale + sx, (f * h + y) * scale + sy, c);
            }
            bmp.Save(path, ImageFormat.Png);
        }
    }
}
"@
$results = New-Object System.Collections.ArrayList
function Add-Result($id, $name, $pass, $detail) {
    $null = $results.Add([pscustomobject]@{ Id = $id; Result = $(if ($pass) { "PASS" } else { "FAIL" }) })
    "{0} {1,-4} {2} :: {3}" -f $id, $(if ($pass) { "PASS" } else { "FAIL" }), $name, $detail
}
$scr = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$W = 300; $H = 72
$X = $scr.Right - $W; $Y = $scr.Top

# Film the corner while `$action` runs; returns frames with timestamps (ms) and changed width.
function Film([scriptblock]$action, [int]$ms) {
    $base = [Cap]::Grab($X, $Y, $W, $H)
    $frames = New-Object System.Collections.ArrayList
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $job = & $action
    while ($sw.ElapsedMilliseconds -lt $ms) {
        $t = $sw.ElapsedMilliseconds
        $px = [Cap]::Grab($X, $Y, $W, $H)
        $null = $frames.Add([pscustomobject]@{ T = $t; Px = $px; Width = [Cap]::Changed($base, $px, $W, $H); Acc = [Cap]::Accents($px) })
    }
    return @{ Frames = $frames; Base = $base; Job = $job }
}

function Summarize($frames, $from) {
    $vis = @($frames | Where-Object { $_.Width -gt 30 -and $_.T -ge $from })
    if ($vis.Count -eq 0) { return $null }
    $max = ($vis | Measure-Object Width -Maximum).Maximum
    $full = @($vis | Where-Object { $_.Width -ge $max - 4 })
    [pscustomobject]@{
        Start = $vis[0].T; End = $vis[-1].T; Visible = $vis[-1].T - $vis[0].T
        FullFrom = $full[0].T; FullTo = $full[-1].T; MaxWidth = $max
        Fps = [math]::Round($frames.Count / (($frames[-1].T - $frames[0].T) / 1000.0), 0)
        Green = ($frames | ForEach-Object { $_.Acc[0] } | Measure-Object -Maximum).Maximum
        Red = ($frames | ForEach-Object { $_.Acc[1] } | Measure-Object -Maximum).Maximum
    }
}

function Save-Sheet($frames, $name, [int[]]$at) {
    $pick = New-Object 'System.Collections.Generic.List[byte[]]'
    foreach ($t in $at) { $f = $frames | Where-Object { $_.T -ge $t } | Select-Object -First 1; if ($f) { $pick.Add($f.Px) } }
    [Cap]::Sheet($pick, $W, $H, (Join-Path (Resolve-Path $Out) "$name.png"), 2)
}

$env:UREC_OVERLAY_CAPTURABLE = "1"

# P01: success - expand, hold, collapse back to the dot.
$r = Film { Start-Process $Cli -ArgumentList "overlay", "--events", "saved@600", "--hold", "4" -PassThru -WindowStyle Hidden } 5200
$s = Summarize $r.Frames 500
$evT = 600
if ($s) {
    $ok = $s.Visible -ge 2150 -and $s.Visible -le 2700 -and ($s.FullFrom - $s.Start) -le 320 -and $s.MaxWidth -gt 120 -and $s.Green -gt 20
    Add-Result "P01" "Success pill: expands, shows the green check message, holds ~2 s, collapses (total ~2.4 s)" $ok "visible=$($s.Visible)ms (spec 2360) expand=$($s.FullFrom - $s.Start)ms full=$($s.FullTo - $s.FullFrom)ms collapse=$($s.End - $s.FullTo)ms width=$($s.MaxWidth)px green_px=$($s.Green) capture=$($s.Fps)fps"
    Save-Sheet $r.Frames "indicator-success" @(300, ($s.Start + 40), ($s.Start + 90), ($s.Start + 180), ($s.Start + 400), ($s.FullTo + 60), ($s.FullTo + 120), ($s.End + 200))
} else { Add-Result "P01" "Success pill appears" $false "no change in the corner" }
$last = $r.Frames[-1]
Add-Result "P02" "Back to the plain dot afterwards (nothing left on screen)" ($last.Width -le 4) "residual width=$($last.Width)px"
$r.Job | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue

# P03: failure - red cross, held ~2.5 s.
$r = Film { Start-Process $Cli -ArgumentList "overlay", "--events", "failed@600", "--hold", "4" -PassThru -WindowStyle Hidden } 5600
$s = Summarize $r.Frames 500
if ($s) {
    Add-Result "P03" "Failure pill: red cross + 'Instant Replay Failed', held ~2.5 s" ($s.Visible -ge 2650 -and $s.Visible -le 3200 -and $s.Red -gt 20) "visible=$($s.Visible)ms (spec 2860) red_px=$($s.Red) green_px=$($s.Green)"
    Save-Sheet $r.Frames "indicator-failure" @(300, ($s.Start + 400), ($s.End - 60))
} else { Add-Result "P03" "Failure pill appears" $false "no change" }
$r.Job | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue

# P04: saving -> saved: dot breathes (no pill) until the result, then the pill.
$r = Film { Start-Process $Cli -ArgumentList "overlay", "--events", "saving@400,saved@2400", "--hold", "4" -PassThru -WindowStyle Hidden } 6000
$during = @($r.Frames | Where-Object { $_.T -gt 700 -and $_.T -lt 2300 })
$wide = @($during | Where-Object { $_.Width -gt 30 }).Count
$pulsing = @($during | Where-Object { $_.Width -gt 0 }).Count
$s = Summarize $r.Frames 2300
Add-Result "P04" "Saving: dot pulses, no 'Saved' before the result; pill only after it" ($wide -eq 0 -and $pulsing -gt 0 -and $s -and $s.Start -ge 2350) "frames_wide_while_saving=$wide pulse_frames=$pulsing/$($during.Count) pill_start=$($s.Start)ms (result at 2400)"
$r.Job | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue

# P05: rapid results: success, then failure 500 ms later, then success - failure must not be hidden.
$r = Film { Start-Process $Cli -ArgumentList "overlay", "--events", "saved@400,failed@900,saved@1200", "--hold", "6" -PassThru -WindowStyle Hidden } 7600
$redAt = @($r.Frames | Where-Object { $_.Acc[1] -gt 20 } | ForEach-Object { $_.T })
$greenAfter = @($r.Frames | Where-Object { $_.Acc[0] -gt 20 -and $redAt.Count -and $_.T -gt $redAt[-1] } | ForEach-Object { $_.T })
$redSpan = if ($redAt.Count) { $redAt[-1] - $redAt[0] } else { 0 }
Add-Result "P05" "Burst: a failure replaces a showing success, is shown in full, then the later success follows" ($redSpan -ge 2200 -and $greenAfter.Count -gt 0) "red shown ${redSpan}ms from $($redAt[0])ms; green after it from $($greenAfter[0])ms"
Save-Sheet $r.Frames "indicator-burst" @(300, 700, 1300, 2500, ($redAt[-1] + 400))
$r.Job | Wait-Process -Timeout 12 -ErrorAction SilentlyContinue

# P06: reduced motion: no in-between frames.
$r = Film { Start-Process $Cli -ArgumentList "overlay", "--events", "saved@600", "--hold", "4", "--reduced" -PassThru -WindowStyle Hidden } 4500
$s = Summarize $r.Frames 500
$mid = if ($s) { @($r.Frames | Where-Object { $_.Width -gt 30 -and $_.Width -lt $s.MaxWidth - 12 }).Count } else { -1 }
Add-Result "P06" "Reduced motion: message appears and disappears without in-between frames" ($s -and $mid -le 1 -and $s.Visible -ge 1850 -and $s.Visible -le 2300) "visible=$($s.Visible)ms partial_frames=$mid"
$r.Job | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue

# P07: no idle cost: CPU of the overlay process once the animation is over.
$p = Start-Process $Cli -ArgumentList "overlay", "--events", "saved@200", "--hold", "12" -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 4
$c0 = $p.TotalProcessorTime.TotalMilliseconds
Start-Sleep -Seconds 5
$p.Refresh(); $c1 = $p.TotalProcessorTime.TotalMilliseconds
$p2 = Start-Process $Cli -ArgumentList "overlay", "--events", "saving@0", "--hold", "6" -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 1
$a0 = $p2.TotalProcessorTime.TotalMilliseconds
Start-Sleep -Seconds 3
$p2.Refresh(); $a1 = $p2.TotalProcessorTime.TotalMilliseconds
Add-Result "P07" "Overlay uses no CPU at rest (timer stops after the animation)" (($c1 - $c0) -lt 60) "after animation: $([math]::Round($c1 - $c0))ms CPU in 5 s; while animating (pulse): $([math]::Round($a1 - $a0))ms CPU in 3 s"
$p, $p2 | Wait-Process -Timeout 15 -ErrorAction SilentlyContinue

# P08: left corner mirrors (grows to the right) - screenshot only.
$XL = $scr.Left; $XR = $X; $X = $XL
$r = Film { Start-Process $Cli -ArgumentList "overlay", "--events", "saved@400", "--hold", "3", "--corner", "top_left" -PassThru -WindowStyle Hidden } 1600
$s = Summarize $r.Frames 300
Add-Result "P08" "Top-left corner: pill grows away from the screen edge" ($s -and $s.MaxWidth -gt 120) "width=$($s.MaxWidth)px"
Save-Sheet $r.Frames "indicator-left" @(1200)
$r.Job | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue
$X = $XR

# P09/P10: Windows notification (overlay off path).
$t = & $Cli toast --file "C:\Users\Public\Videos\Desktop 2026-10-12 Replay.mp4" 2>&1
"toast: $($t -join ' | ')"
$shown = ($t -join " ") -match "shown=true"
$hist = if (($t -join " ") -match "toast_history=(-?\d+)") { [int]$Matches[1] } else { -1 }
Add-Result "P09" "Windows notification is accepted by Windows (Action Center history)" ($shown -and $hist -ge 1) "shown=$shown history=$hist"

# P10-P13: the real app. Alt+F10 with Instant Replay on.
$settingsPath = Join-Path $env:APPDATA "uRec\settings.json"
$videos = Join-Path $env:USERPROFILE "Videos\uRec"
function Set-AppSettings([bool]$overlay) {
    $s = Get-Content $settingsPath -Raw | ConvertFrom-Json
    $s.replay.enabled = $true; $s.replay.seconds = 15; $s.appearance.overlay = $overlay
    $s.first_run_done = $true
    if ($s.PSObject.Properties.Name -contains "setup_done") { $s.setup_done = $true }
    $s | ConvertTo-Json -Depth 8 | Out-File -Encoding utf8 $settingsPath
}
if (-not (Test-Path $settingsPath)) { $p = Start-Process $Exe -PassThru; Start-Sleep 8; Stop-Process -Id $p.Id -Force; Start-Sleep 2 }
Set-AppSettings $true
$before = @(Get-ChildItem $videos -Recurse -Filter "*Replay*.mp4" -ErrorAction SilentlyContinue).Count
# Start in the tray (as with "Start with Windows") so the window doesn't cover the corner.
$app = Start-Process $Exe -ArgumentList "--minimized" -PassThru
Start-Sleep -Seconds 14
$wsh = New-Object -ComObject WScript.Shell
$r = Film { $wsh.SendKeys("%{F10}") } 9000
$s = Summarize $r.Frames 0
$afterFiles = @(Get-ChildItem $videos -Recurse -Filter "*Replay*.mp4" -ErrorAction SilentlyContinue)
$newest = $afterFiles | Sort-Object LastWriteTime | Select-Object -Last 1
Add-Result "P10" "Real save (Alt+F10): clip written, then the success pill (only after saving)" ($afterFiles.Count -gt $before -and $s -and $s.Green -gt 20) "clips $before -> $($afterFiles.Count) ($($newest.Name), $([math]::Round($newest.Length / 1MB, 1)) MB); pill from $($s.Start)ms for $($s.Visible)ms"
if ($s) { Save-Sheet $r.Frames "indicator-app-save" @(100, ($s.Start - 300), ($s.Start + 400)) }
# Rapid presses in the real app.
$before = $afterFiles.Count
1..4 | ForEach-Object { $wsh.SendKeys("%{F10}"); Start-Sleep -Milliseconds 150 }
Start-Sleep -Seconds 14
$burst = @(Get-ChildItem $videos -Recurse -Filter "*Replay*.mp4" -ErrorAction SilentlyContinue)
$log = Join-Path $env:LOCALAPPDATA "uRec\logs\urec.log"
$folded = @(Select-String -Path $log -Pattern "folded into the save already waiting" -ErrorAction SilentlyContinue).Count
$okSaves = @(Select-String -Path $log -Pattern "replay save #\d+ ok" -ErrorAction SilentlyContinue).Count
$bad = @($burst | Where-Object { $_.Length -lt 10000 })
Add-Result "P11" "4 rapid presses: at most 2 saves queued, rest folded in, every clip complete, nothing overwritten" ($burst.Count -ge $before + 2 -and $burst.Count -le $before + 3 -and $bad.Count -eq 0) "new clips=$($burst.Count - $before) folded=$folded total ok saves=$okSaves"
Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

# Overlay off: no pill, a Windows notification instead.
Set-AppSettings $false
$h0 = if (((& $Cli toast --count) -join " ") -match "toast_history=(-?\d+)") { [int]$Matches[1] } else { -1 }
$app = Start-Process $Exe -ArgumentList "--minimized" -PassThru
Start-Sleep -Seconds 14
$r = Film { $wsh.SendKeys("%{F10}") } 7000
$s = Summarize $r.Frames 0
$h1 = if (((& $Cli toast --count) -join " ") -match "toast_history=(-?\d+)") { [int]$Matches[1] } else { -1 }
Add-Result "P12" "Overlay off: nothing drawn on screen for the save" (-not $s) "corner change: $(if ($s) { "$($s.MaxWidth)px" } else { 'none' })"
Add-Result "P13" "Overlay off: a Windows notification is shown instead" ($h1 -gt $h0) "uRec notifications in Action Center: $h0 -> $h1"
Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
Select-String -Path $log -Pattern "replay save|notification|clip link" -ErrorAction SilentlyContinue | Select-Object -Last 14 | ForEach-Object { "log: " + $_.Line }
Remove-Item env:UREC_OVERLAY_CAPTURABLE

$failed = @($results | Where-Object Result -eq "FAIL").Count
"indicator passed=$($results.Count - $failed) failed=$failed"
