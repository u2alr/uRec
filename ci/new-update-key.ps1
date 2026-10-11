# One-time setup for signed updates. Run in PowerShell 7 from the repository folder, with the
# GitHub CLI signed in (gh auth login):
#
#   pwsh ci/new-update-key.ps1
#
# It creates an ECDSA P-256 key pair, stores the private key as the repository secret
# UREC_UPDATE_KEY (it never touches the disk or the terminal), and writes the public key to
# crates/app/assets/update-key.pub. Commit that file. From the next release on, uRec only
# installs updates whose checksum list is signed with this key.
#
# Keep in mind: if the secret is lost, a new key means copies built with the old key can't
# auto-update to releases signed with the new one (they'd need one manual install).
$ErrorActionPreference = "Stop"
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { throw "Install the GitHub CLI and run 'gh auth login' first." }
$pubFile = "crates/app/assets/update-key.pub"
if ((Test-Path $pubFile) -and (Get-Content $pubFile -Raw).Trim()) { throw "$pubFile already has a key. Delete it first if you really want to replace the key." }
$ecdsa = [System.Security.Cryptography.ECDsa]::Create([System.Security.Cryptography.ECCurve+NamedCurves]::nistP256)
$secret = [Convert]::ToBase64String($ecdsa.ExportPkcs8PrivateKey())
$secret | gh secret set UREC_UPDATE_KEY
if ($LASTEXITCODE -ne 0) { throw "gh secret set failed" }
$p = $ecdsa.ExportParameters($false)
$hex = (($p.Q.X + $p.Q.Y) | ForEach-Object { $_.ToString("x2") }) -join ""
[IO.File]::WriteAllText((Join-Path $PWD $pubFile), "$hex`n")
"Secret UREC_UPDATE_KEY set. Public key written to $pubFile - commit it:"
"  git add $pubFile; git commit -m 'Signed updates'; git push"
