# Security

## Supported versions

Only the latest release gets security fixes.

## Reporting a problem

Please report security issues privately through GitHub: **Security › Report a vulnerability** on https://github.com/u2alr/uRec. Don't open a public issue for security problems. Include the uRec version, Windows version and steps to reproduce.

## What uRec does and doesn't do

- **One network connection, for updates:** once a day (unless turned off) it requests `https://api.github.com/repos/u2alr/uRec/releases/latest` with a `uRec/<version>` user agent. No telemetry, no identifiers, no uploads. It uses Windows' own HTTPS stack (WinHTTP), so certificate checks and proxy settings come from Windows.
- **Update integrity (signatures):** see "Signed updates" below.
- **Update integrity:** only `https://github.com/u2alr/uRec/releases/download/...` URLs are accepted. The installer is downloaded to `%LOCALAPPDATA%\uRec\updates` and its SHA-256 must match the release's `SHA256SUMS.txt`. It is checked again right before it runs, and a mismatch deletes it. Only a strictly newer version (no pre-releases) is ever installed. The installer is not code-signed yet. The checksum protects against corrupted or swapped downloads, but not against someone who controls the GitHub repository itself.
- **Normal user rights:** it runs without administrator rights and installs per-user by default.
- **No game injection:** capture uses Windows APIs (Windows Graphics Capture, Desktop Duplication, GDI). It never injects into games and doesn't interact with anti-cheat software.
- **One external program:** it starts only its bundled `ffmpeg.exe`, from its own folder, with arguments it builds itself. FFmpeg is tied to uRec through a Windows job object, so it can't keep running if uRec exits or crashes.
- **Local data only:** settings in `%APPDATA%\uRec`, and the replay buffer, thumbnails and logs in `%LOCALAPPDATA%\uRec`. Logs never contain screen or audio content.
- **Deleting goes to the Recycle Bin:** the library never deletes recordings permanently.
- **Notification links:** uRec registers a per-user `urec:` link scheme in `HKCU\Software\Classes` (removed on uninstall) for the **Open clip** and **Show in folder** notification buttons. The app accepts only `open` or `reveal` of an existing `.mp4` or `.mkv` file inside the configured recordings folder. Anything else is refused and logged.
- **Temporary files:** an interrupted save can leave `*.urec-tmp.mp4` files and `stage-*` folders behind. Only these names are cleaned up at startup.
- **Dependency audit:** `cargo audit` runs on every release build. Results are in TESTING.md.

## Code signing and SmartScreen

uRec's installer and executables are **not code-signed yet**. Windows SmartScreen may show "Windows protected your PC" for new downloads of unsigned programs with little download history. Choose **More info › Run anyway** only if you downloaded uRec from https://github.com/u2alr/uRec/releases. You can also compare the file with `SHA256SUMS.txt` on the release page, for example with `Get-FileHash uRec-x.y.z-setup.exe` in PowerShell.

Options being considered (none is in place today):

- **A free signing service for open-source projects**, such as SignPath Foundation. Signing happens in CI with a key the project doesn't hold.
- **Microsoft's cloud signing service** (Azure Trusted Signing). This is a paid subscription and requires identity validation.
- **A traditional OV or EV code-signing certificate** from a certificate authority.

A signature makes tampering detectable and shows a verified publisher. **It does not guarantee that SmartScreen warnings go away.** SmartScreen also weighs a file's reputation, which builds up over time with any certificate type.

## Signed updates

Separately from code signing, uRec can verify that an update comes from this project:

- Each release can include `SHA256SUMS.txt.sig`, an ECDSA P-256 signature over `SHA256SUMS.txt`, made in the release workflow with a private key stored as the repository secret `UREC_UPDATE_KEY`.
- The public key lives in `crates/app/assets/update-key.pub` and is built into the app.
- A build that contains a public key installs an update only if the signature verifies, using Windows' CNG cryptography. It also still checks the SHA-256 checksum. A missing or invalid signature means no update.
- If the public key file is present but the secret is missing or doesn't match, the release workflow fails. An unsigned release can never go out to copies that expect signatures.

**Status:** no key has been created yet, so updates are checked by SHA-256 only. To turn signing on, run this once from the repository folder (PowerShell 7, GitHub CLI signed in):

```
pwsh ci/new-update-key.ps1
git add crates/app/assets/update-key.pub; git commit -m "Signed updates"; git push
```

The script stores the private key directly as the secret; it never writes it to disk or shows it. Keep in mind that if the key is ever replaced, copies built with the old key need one manual install of a newer version.
