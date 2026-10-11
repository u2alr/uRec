# Signs dist\SHA256SUMS.txt with the project's update key (ECDSA P-256, SHA-256), writing
# dist\SHA256SUMS.txt.sig as hex r||s. Run by the release workflow.
# - No public key in the repo yet: nothing to do (updates are checksum-only).
# - Public key present: the private key secret must be present and match it, or the release fails
#   (an unsigned release would be refused by every copy of uRec built with the key).
$ErrorActionPreference = "Stop"
$pub = (Get-Content crates\app\assets\update-key.pub -Raw -ErrorAction SilentlyContinue)
if (-not $pub -or -not $pub.Trim()) { "update key: none yet (checksum-only updates)"; exit 0 }
if (-not $env:UREC_UPDATE_KEY) { throw "crates\app\assets\update-key.pub exists but the UREC_UPDATE_KEY secret is missing" }
$ecdsa = [System.Security.Cryptography.ECDsa]::Create()
$read = 0
$ecdsa.ImportPkcs8PrivateKey([Convert]::FromBase64String($env:UREC_UPDATE_KEY), [ref]$read)
$p = $ecdsa.ExportParameters($false)
$mine = (($p.Q.X + $p.Q.Y) | ForEach-Object { $_.ToString("x2") }) -join ""
if ($mine -ne $pub.Trim().ToLower()) { throw "UREC_UPDATE_KEY doesn't match crates\app\assets\update-key.pub" }
$data = [IO.File]::ReadAllBytes("$PWD\dist\SHA256SUMS.txt")
$sig = $ecdsa.SignData($data, [System.Security.Cryptography.HashAlgorithmName]::SHA256)
[IO.File]::WriteAllText("$PWD\dist\SHA256SUMS.txt.sig", ((($sig | ForEach-Object { $_.ToString("x2") }) -join "") + "`n"))
"update key: SHA256SUMS.txt signed"
