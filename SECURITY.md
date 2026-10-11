# Security

## Supported versions

Only the latest release gets security fixes.

## Reporting a problem

Please report security issues privately through GitHub: **Security › Report a vulnerability** on https://github.com/u2alr/uRec. Don't open a public issue for security problems. Include the uRec version, Windows version and steps to reproduce.

## What uRec does and doesn't do

- **No network:** it makes no connections, with no telemetry, no update checks and no uploads.
- **Normal user rights:** it runs without administrator rights and installs per-user by default.
- **No game injection:** capture uses Windows APIs (Windows Graphics Capture, Desktop Duplication, GDI). It never injects into games and doesn't interact with anti-cheat software.
- **One external program:** it starts only its bundled `ffmpeg.exe`, from its own folder, with arguments it builds itself. FFmpeg is tied to uRec through a Windows job object, so it can't keep running if uRec exits or crashes.
- **Local data only:** settings in `%APPDATA%\uRec`, and the replay buffer, thumbnails and logs in `%LOCALAPPDATA%\uRec`. Logs never contain screen or audio content.
- **Deleting goes to the Recycle Bin:** the library never deletes recordings permanently.
- **Dependency audit:** `cargo audit` runs on every release build. Results are in TESTING.md.
