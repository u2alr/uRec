# Security

## Supported versions

Only the latest release gets security fixes.

## Reporting a problem

Please report security issues privately through GitHub: **Security › Report a vulnerability** on https://github.com/u2alr/uRec. Don't open a public issue for security problems. Include the uRec version, Windows version and steps to reproduce.

## What uRec does and doesn't do

- **One network connection, for updates:** once a day (unless turned off) it requests `https://api.github.com/repos/u2alr/uRec/releases/latest` with a `uRec/<version>` user agent. No telemetry, no identifiers, no uploads. It uses Windows' own HTTPS stack (WinHTTP), so certificate checks and proxy settings come from Windows.
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

A signature makes tampering detectable and shows a verified publisher. **It does not guarantee that SmartScreen warnings go away.** SmartScreen also weighs a file's reputation, which builds up over time with any certificate type. Once releases are signed, the updater will also check the signature before it installs anything, in addition to the SHA-256 checksum.
