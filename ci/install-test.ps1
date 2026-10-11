# Installs the built setup silently, checks shortcuts and registration, measures idle and
# Instant Replay resource use of the installed app, then uninstalls and checks what remains.
param([Parameter(Mandatory)][string]$Setup)
$ErrorActionPreference = "Continue"
$results = New-Object System.Collections.ArrayList
function Check($id, $name, $ok, $detail) {
    $null = $results.Add([pscustomobject]@{ Id = $id; Check = $name; Result = $(if ($ok) { "PASS" } else { "FAIL" }); Detail = $detail })
    "{0} {1} {2} :: {3}" -f $id, $(if ($ok) { "PASS" } else { "FAIL" }), $name, $detail
}
function Measure-Proc([string]$name, [int]$seconds) {
    $p = Get-Process $name -ErrorAction SilentlyContinue
    if (-not $p) { return $null }
    $t0 = ($p | Measure-Object TotalProcessorTime -Sum).Sum
    $t0 = ($p | ForEach-Object { $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
    Start-Sleep -Seconds $seconds
    $p = Get-Process $name -ErrorAction SilentlyContinue
    $t1 = ($p | ForEach-Object { $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
    $cores = [Environment]::ProcessorCount
    [pscustomobject]@{
        CpuPercent = [math]::Round(($t1 - $t0) / ($seconds * 1000) / $cores * 100, 2)
        WorkingSetMB = [math]::Round((($p | Measure-Object WorkingSet64 -Sum).Sum) / 1MB, 1)
        PrivateMB = [math]::Round((($p | Measure-Object PrivateMemorySize64 -Sum).Sum) / 1MB, 1)
    }
}

$setupItem = Get-Item $Setup
"installer: $($setupItem.Name) $([math]::Round($setupItem.Length / 1MB, 1)) MB"
$app = Join-Path $env:LOCALAPPDATA "Programs\uRec"
$startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\uRec.lnk"
$desktop = Join-Path ([Environment]::GetFolderPath("Desktop")) "uRec.lnk"
$uninstKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{5B7E2C64-9A1D-4F3B-B8E1-2D6C0A9F4E17}_is1"
$settings = Join-Path $env:APPDATA "uRec\settings.json"
$videos = Join-Path $env:USERPROFILE "Videos\uRec"

$sw = [Diagnostics.Stopwatch]::StartNew()
$p = Start-Process $Setup -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/CURRENTUSER", "/NORESTART", "/TASKS=desktopicon" -PassThru -Wait
Check "I01" "Silent per-user install succeeds without admin prompt" ($p.ExitCode -eq 0) "exit=$($p.ExitCode) in $([math]::Round($sw.Elapsed.TotalSeconds,1)) s"
Check "I02" "Program files installed (uRec.exe, ffmpeg.exe, ffprobe.exe, notices)" ((Test-Path "$app\uRec.exe") -and (Test-Path "$app\ffmpeg.exe") -and (Test-Path "$app\ffprobe.exe") -and (Test-Path "$app\THIRD-PARTY-NOTICES.txt")) $app
$footprint = [math]::Round(((Get-ChildItem $app -Recurse | Measure-Object Length -Sum).Sum) / 1MB, 1)
"installed footprint: $footprint MB"
Check "I03" "Start Menu shortcut created" (Test-Path $startMenu) $startMenu
Check "I04" "Optional desktop shortcut created when chosen" (Test-Path $desktop) $desktop
$reg = Get-ItemProperty $uninstKey -ErrorAction SilentlyContinue
Check "I05" "Registered in Apps & features with name and icon" ($reg -and $reg.DisplayName -eq "uRec" -and $reg.DisplayIcon) "name=$($reg.DisplayName) icon=$($reg.DisplayIcon)"
$run = Get-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name uRec -ErrorAction SilentlyContinue
Check "I06" "Does not start with Windows unless chosen" (-not $run) "run entry: $($run.uRec)"

# Launch the installed app and measure it at idle.
$sw.Restart()
$proc = Start-Process "$app\uRec.exe" -PassThru
Start-Sleep -Seconds 15
$alive = -not $proc.HasExited
Check "I07" "Installed app launches and stays running" $alive "pid=$($proc.Id)"
$ffIdle = @(Get-Process ffmpeg -ErrorAction SilentlyContinue).Count
Check "I08" "Idle with Instant Replay off: no capture or encoder process" ($ffIdle -eq 0) "ffmpeg processes=$ffIdle"
$idle = Measure-Proc "uRec" 30
"idle (30 s window, after 15 s settle): cpu=$($idle.CpuPercent)% workingset=$($idle.WorkingSetMB) MB private=$($idle.PrivateMB) MB"
Check "I09" "Idle CPU close to zero (< 1% of the machine)" ($idle -and $idle.CpuPercent -lt 1.0) "cpu=$($idle.CpuPercent)%"
Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

# Instant Replay on: measure app + ffmpeg, check the buffer stays bounded.
$s = Get-Content $settings -Raw | ConvertFrom-Json
$s.replay.enabled = $true
$s.replay.seconds = 15
$s | ConvertTo-Json -Depth 8 | Out-File -Encoding utf8 $settings
$proc = Start-Process "$app\uRec.exe" -PassThru
Start-Sleep -Seconds 25
$appM = Measure-Proc "uRec" 20
$ffM = Measure-Proc "ffmpeg" 1
$buf = Join-Path $env:LOCALAPPDATA "uRec\replay"
$segs = @(Get-ChildItem $buf -Filter *.ts -ErrorAction SilentlyContinue)
$bufMB = [math]::Round((($segs | Measure-Object Length -Sum).Sum) / 1MB, 1)
"replay on (15 s, CPU x264 on this VM): app cpu=$($appM.CpuPercent)% ws=$($appM.WorkingSetMB) MB; ffmpeg ws=$($ffM.WorkingSetMB) MB; buffer=$($segs.Count) files $bufMB MB"
Check "I10" "Instant Replay runs from the installed app with a bounded buffer" ($segs.Count -gt 0 -and $segs.Count -le 12) "segments=$($segs.Count) (limit 12 for 15 s) size=$bufMB MB"
Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
Check "I11" "Killing the app also ends its FFmpeg (job object)" (@(Get-Process ffmpeg -ErrorAction SilentlyContinue).Count -eq 0) "ffmpeg left=$(@(Get-Process ffmpeg -ErrorAction SilentlyContinue).Count)"

# A recording that must survive uninstall.
New-Item -ItemType Directory -Force $videos | Out-Null
"keep me" | Out-File (Join-Path $videos "keep-test.mp4")

$un = Get-ChildItem $app -Filter "unins*.exe" | Select-Object -First 1
$p = Start-Process $un.FullName -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -PassThru -Wait
Start-Sleep -Seconds 3
Check "I12" "Silent uninstall succeeds" ($p.ExitCode -eq 0) "exit=$($p.ExitCode)"
Check "I13" "Program files, shortcuts and Apps entry removed" (-not (Test-Path "$app\uRec.exe") -and -not (Test-Path $startMenu) -and -not (Test-Path $desktop) -and -not (Get-ItemProperty $uninstKey -ErrorAction SilentlyContinue)) "exe=$(Test-Path "$app\uRec.exe") start=$(Test-Path $startMenu) desk=$(Test-Path $desktop)"
Check "I14" "Recordings are kept" (Test-Path (Join-Path $videos "keep-test.mp4")) $videos
Check "I15" "Settings are kept (preserved by default)" (Test-Path $settings) $settings
Check "I16" "Temporary data (replay buffer, thumbnails, logs) removed" (-not (Test-Path (Join-Path $env:LOCALAPPDATA "uRec"))) ""

"=== Summary"
$results | Format-Table -AutoSize -Wrap | Out-String -Width 220
$failed = @($results | Where-Object Result -eq "FAIL").Count
"install passed=$($results.Count - $failed) failed=$failed installer_mb=$([math]::Round($setupItem.Length / 1MB, 1)) installed_mb=$footprint"
