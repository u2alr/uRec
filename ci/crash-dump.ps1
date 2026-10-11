# Runs the app once with Windows Error Reporting local dumps enabled and prints the crashing stack.
param([Parameter(Mandatory)][string]$Exe, [string]$Out = "screenshots")
$ErrorActionPreference = "Continue"
New-Item -ItemType Directory -Force $Out | Out-Null
$dumps = Join-Path $env:RUNNER_TEMP "dumps"
New-Item -ItemType Directory -Force $dumps | Out-Null
$key = "HKLM:\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps\uRec.exe"
New-Item -Path $key -Force | Out-Null
New-ItemProperty -Path $key -Name DumpFolder -Value $dumps -PropertyType ExpandString -Force | Out-Null
New-ItemProperty -Path $key -Name DumpType -Value 2 -PropertyType DWord -Force | Out-Null
$env:UREC_PAGE = "home"
$p = Start-Process $Exe -PassThru
Start-Sleep -Seconds 10
"alive=$(-not $p.HasExited) exit=$($p.ExitCode)"
if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
Start-Sleep -Seconds 3
$dump = Get-ChildItem $dumps -Filter *.dmp | Select-Object -First 1
$cdb = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\Debuggers\x64\cdb.exe", "$env:ProgramFiles\Windows Kits\10\Debuggers\x64\cdb.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
"dump=$($dump.FullName) cdb=$($cdb.FullName)"
if ($dump -and $cdb) {
    $sym = (Resolve-Path (Split-Path $Exe)).Path
    & $cdb.FullName -z $dump.FullName -y "$sym;srv*" -c ".ecxr; kc 40; !analyze -v; ~*kc 12; q" 2>&1 | Select-Object -Last 220
}
