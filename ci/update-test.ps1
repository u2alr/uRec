# End-to-end test of automatic updates against the real GitHub release that was just published.
# Installs the build, pretends to be an older version (UREC_FAKE_VERSION) and checks that uRec
# finds the release, downloads and verifies the installer, rejects a corrupted download, and
# installs the update silently at startup and comes back.
param([Parameter(Mandatory)][string]$Setup)
$ErrorActionPreference = "Continue"
$results = New-Object System.Collections.ArrayList
function Check($id, $name, $ok, $detail) {
    $null = $results.Add([pscustomobject]@{ Id = $id; Result = $(if ($ok) { "PASS" } else { "FAIL" }) })
    "{0} {1} {2} :: {3}" -f $id, $(if ($ok) { "PASS" } else { "FAIL" }), $name, $detail
}
$app = Join-Path $env:LOCALAPPDATA "Programs\uRec"
$updates = Join-Path $env:LOCALAPPDATA "uRec\updates"
$log = Join-Path $env:LOCALAPPDATA "uRec\logs\urec.log"

$p = Start-Process $Setup -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/CURRENTUSER", "/NORESTART" -PassThru -Wait
if ($p.ExitCode -ne 0) { throw "install failed: $($p.ExitCode)" }
$installed = (Get-Item "$app\uRec.exe").LastWriteTimeUtc

function Run-Check([hashtable]$envs) {
    foreach ($k in $envs.Keys) { Set-Item "env:$k" $envs[$k] }
    $r = Start-Process "$app\uRec.exe" -ArgumentList "--update-check" -PassThru -Wait
    foreach ($k in $envs.Keys) { Remove-Item "env:$k" -ErrorAction SilentlyContinue }
    $r.ExitCode
}

# U01: the real version is up to date.
Remove-Item $updates -Recurse -Force -ErrorAction SilentlyContinue
$code = Run-Check @{}
$last = (Get-Content $log -Tail 1)
Check "U01" "Current build reports up to date" ($code -eq 0 -and $last -match "UpToDate") $last

# U02: an older version finds, downloads and verifies the published installer.
$code = Run-Check @{ UREC_FAKE_VERSION = "0.0.1" }
$last = (Get-Content $log -Tail 1)
$pending = Get-Content "$updates\pending.txt" -ErrorAction SilentlyContinue
Check "U02" "Older version downloads and verifies the update" ($code -eq 0 -and $last -match "Ready" -and $pending.Count -ge 3) "$last | pending=$($pending -join ' ')"
if ($pending) {
    $file = Join-Path $updates $pending[1]
    $hash = (Get-FileHash $file -Algorithm SHA256).Hash
    Check "U03" "Downloaded installer matches the published SHA-256" ($hash -eq $pending[2].ToUpper()) "$($pending[1]) $hash"
} else { Check "U03" "Downloaded installer matches the published SHA-256" $false "no pending update" }

# U04: a corrupted download is rejected and deleted.
Remove-Item $updates -Recurse -Force -ErrorAction SilentlyContinue
$code = Run-Check @{ UREC_FAKE_VERSION = "0.0.1"; UREC_UPDATE_TEST_CORRUPT = "1" }
$last = (Get-Content $log -Tail 1)
$left = @(Get-ChildItem $updates -Filter "*.exe*" -ErrorAction SilentlyContinue).Count
Check "U04" "Corrupted download is rejected and deleted" ($code -ne 0 -and $last -match "checksum" -and $left -eq 0 -and -not (Test-Path "$updates\pending.txt")) "$last | files left=$left"

# U05/U06: pending update installs silently at startup and uRec comes back.
$null = Run-Check @{ UREC_FAKE_VERSION = "0.0.1" }
$env:UREC_FAKE_VERSION = "0.0.1"
$sw = [Diagnostics.Stopwatch]::StartNew()
$first = Start-Process "$app\uRec.exe" -PassThru
$env:UREC_FAKE_VERSION = $null
$first.WaitForExit(20000) | Out-Null
$back = $null
while ($sw.Elapsed.TotalSeconds -lt 120 -and -not $back) {
    Start-Sleep -Seconds 2
    $back = Get-Process uRec -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne $first.Id } | Select-Object -First 1
}
$installLog = Get-Content "$updates\install.log" -ErrorAction SilentlyContinue
$ok = $installLog -match "Installation process succeeded|Need to restart Windows\? No"
Check "U05" "Pending update installs silently on startup" ($first.HasExited -and $ok) "first exited=$($first.HasExited) installer log lines=$($installLog.Count)"
Check "U06" "uRec relaunches after updating" ([bool]$back) "pid=$($back.Id) after $([math]::Round($sw.Elapsed.TotalSeconds,1)) s; exe rewritten=$((Get-Item "$app\uRec.exe").LastWriteTimeUtc -ge $installed)"
Get-Process uRec -ErrorAction SilentlyContinue | Stop-Process -Force
"--- uRec log (update lines) ---"
Select-String -Path $log -Pattern "update:" | Select-Object -Last 15 | ForEach-Object { $_.Line }
if ($installLog) { "--- installer log (tail) ---"; $installLog | Select-Object -Last 8 }

Start-Process "$app\unins000.exe" -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait
$pass = @($results | Where-Object Result -eq PASS).Count
$fail = @($results | Where-Object Result -eq FAIL).Count
"update passed=$pass failed=$fail"
