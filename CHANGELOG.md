# Changelog

## 0.1.2

- **Updates:** uRec checks GitHub once a day for a new version. The installed app downloads it in the background, verifies its SHA-256 against the published checksums and installs it silently when you quit, or the next time it starts. **Restart now** installs it right away. It never installs during a recording or while Instant Replay is on. Find it in **Settings › General › Updates**, where both the check and automatic install can be turned off. Portable copies only show that an update is available.
- Releases now include `SHA256SUMS.txt`.

## 0.1.1

Fixes from first testing:

- **Folders:** Open folder and Show in folder now open the right folder, including paths with spaces. They used to land in another folder. The default save location now follows your real Videos folder, even if Windows or OneDrive moved it.
- **Buttons:** every button now shows hover and pressed states. Save replay looks disabled while Instant Replay is off, with a tip explaining why.
- **Library:** the Refresh spinner no longer spins forever while Instant Replay is on, and Refresh responds immediately.
- **Overlay:** now a small, perfectly round dot (anti-aliased), red while recording and amber while Instant Replay is on.

## 0.1.0 — first release

**Recording**
- Record a whole display, a single window, or part of a display.
- Capture uses Windows Graphics Capture, Desktop Duplication or GDI, picked automatically with fallback, and frames stay on the GPU when the encoder allows it.
- Encoders are tested on your PC at startup, never guessed from the GPU brand: NVIDIA NVENC, AMD AMF, Intel Quick Sync, the Windows hardware encoder, then x264/x265 or the Windows encoder on the CPU.
- H.264, HEVC and AV1 (where supported). Resolution from 720p to 4K or custom, 24–240 FPS or match the display, bitrate, CBR/VBR, encoder preset and keyframe interval.
- Five quality presets with a recommended one for your PC.
- Crash-safe fragmented MP4 by default, or MKV.

**Audio**
- Game and desktop sound (WASAPI loopback, any output device) and microphone, each with its own volume, level meters and a mute hotkey.
- One mixed track or separate tracks.

**Instant Replay**
- Off by default. Keeps 15 seconds to 20 minutes in a fixed-size buffer on disk.
- Saves without re-encoding and never includes a half-written segment.
- Recording while Instant Replay is on reuses the same stream.

**Everything else**
- Global hotkeys with conflict and "already in use" detection.
- Recordings library with thumbnails, search, sort, play, rename, copy path, show in folder, and delete to the Recycle Bin.
- Recording overlay (click-through, hidden from recordings on Windows 10 2004+).
- Tray icon; uRec keeps running when the window is closed.
- Disk-space checks: warns when low and stops safely below 1 GB.
- Per-user installer that needs no admin rights, plus a portable zip. Uninstalling keeps recordings and settings.
- No network access, no account, no telemetry.

**Not in this release**
- Pause and resume.
- Mouse-click visualization.
- HDR capture (records 8-bit SDR).
- Code-signed installer.
