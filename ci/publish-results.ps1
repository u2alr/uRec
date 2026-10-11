# Commits a report file to the ci-results branch so it can be read with git.
# Usage: ci\publish-results.ps1 -Name probe -File report.txt
param([Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][string]$File)
$ErrorActionPreference = "Continue"
$dir = Join-Path $env:RUNNER_TEMP "ci-results"
git config --global user.name "urec-ci"
git config --global user.email "ci@users.noreply.github.com"
$remote = "https://x-access-token:$env:GH_TOKEN@github.com/$env:GITHUB_REPOSITORY.git"
for ($try = 0; $try -lt 4; $try++) {
    if (Test-Path $dir) { Remove-Item $dir -Recurse -Force }
    git clone --quiet --depth 1 --branch ci-results $remote $dir 2>$null
    if ($LASTEXITCODE -ne 0) {
        New-Item -ItemType Directory $dir | Out-Null
        Push-Location $dir; git init --quiet; git checkout --quiet -b ci-results; git remote add origin $remote; Pop-Location
    }
    Copy-Item $File (Join-Path $dir "$Name.txt") -Force
    Push-Location $dir
    git add -A
    git commit --quiet -m "${Name}: run $env:GITHUB_RUN_ID ($env:GITHUB_SHA)" 2>$null
    git push --quiet origin ci-results 2>$null
    $ok = $LASTEXITCODE -eq 0
    Pop-Location
    if ($ok) { "published ${Name}.txt"; exit 0 }
    Start-Sleep -Seconds (3 + $try * 3)
}
Write-Host "could not publish results"
exit 1
