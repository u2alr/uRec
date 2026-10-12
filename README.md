# uRec

A lightweight screen and game recorder for Windows 10 and 11. Press a hotkey to record, or keep Instant Replay running and save the last moments after they happen. Everything stays on your PC: no account, no uploads, no telemetry. It keeps itself up to date.

## Install

1. Download `uRec-<version>-setup.exe` from the [Releases page](https://github.com/u2alr/uRec/releases).
2. Run it. It installs for your user only, so it doesn't need administrator rights.
3. Open uRec from the Start menu.

Windows may show a SmartScreen warning because the installer isn't code-signed yet. Choose **More info › Run anyway** if you trust the download.

There's also a portable zip (`uRec-<version>-portable.zip`). Unzip it anywhere and run `uRec.exe`. Settings are still stored in `%APPDATA%\uRec`.

### Requirements

| | |
|---|---|
| Windows | 10 version 1809 or newer, 64-bit. Windows 11 recommended. |
| Best capture | Windows 10 version 1903+ (Windows Graphics Capture). Older versions use Desktop Duplication or GDI. |
| Overlay hidden from recordings | Windows 10 version 2004+ |
| Graphics | Any. A hardware encoder (NVIDIA, AMD or Intel) is used when one works; otherwise the CPU encodes. |
| Disk | About 120 MB for the app, plus space for your recordings |

## Using it

On first launch, a short setup shows what uRec found on your PC:
- capture methods;
- the video encoders that actually work, tested rather than guessed from the graphics card's brand;
- your sound devices.

It then recommends a quality preset, shows where files go and how much space the Instant Replay buffer uses, and offers a 5-second test recording. The test checks capture, encoding, sound, saving and playback, and shows which parts passed. You can skip setup and run it again later from **Settings › Advanced**.

| Action | Default hotkey |
|---|---|
| Start or stop recording | Alt+F9 |
| Save Instant Replay | Alt+F10 |
| Turn Instant Replay on or off | Alt+Shift+F10 |
| Mute or unmute microphone | Ctrl+Alt+M |
| Show or hide the overlay | not set |

Hotkeys work while a game has focus. Change them in **Settings › Hotkeys**.

- **Recordings** are saved to `Videos\uRec`, in a sub-folder per game, with names like `VALORANT 2026-10-11 15.02.11.mp4`.
- **Instant Replay** is off by default. When it's on, uRec keeps the last 15 seconds to 20 minutes in a fixed-size buffer on disk and saves it when you press the hotkey.
  - Saves are written without re-encoding. A clip counts as saved only once it has been checked as a complete MP4 and flushed to disk.
  - Pressing the hotkey several times quickly never overwrites a clip: one save runs, one waits, and further presses are included in the waiting one.
  - Starting a recording while Instant Replay is on reuses the same stream, so nothing is encoded twice.
- **Library** lists your recordings with thumbnails, length, resolution, frame rate and size.
  - You can search, sort, star favorites, play, rename, copy the path, show in Explorer, or delete.
  - **Select** mode lets you remove many recordings at once. Deleting always moves files to the Recycle Bin.
  - Damaged or unfinished files are marked. Files moved or deleted outside uRec are handled without errors.
- **Editor:** **Edit** on any recording, or **Edit together** for several, opens a simple editor.
  - Cut out as many parts as you like: split at the playhead (S), delete pieces, trim edges, reorder.
  - Preview with sound. Crop to 16:9, 9:16, 1:1 or freely. Mute or change the volume.
  - Export to a new file, either frame-exact or lossless. Originals are never changed.
- **Overlay** is a small dot in a screen corner: red while recording, amber while Instant Replay is on.
  - When an Instant Replay clip is saved, the dot briefly opens into "Instant Replay Saved" with a green check, then closes again. If a save fails, it shows "Instant Replay Failed" with a red cross.
  - The dot dims gently while a save is in progress. Nothing animates while idle.
  - It's click-through, never takes focus, and is hidden from your recordings on Windows 10 2004 and newer.
  - You can set its corner, size and opacity. Animations follow Windows' "Animation effects" setting, or you can set them yourself.
- **Overlay off:** save results appear as Windows notifications instead, with **Open clip** and **Show in folder** buttons. They also always appear inside the app.
- **Tray:** closing the window keeps uRec in the system tray so hotkeys keep working. Quit from the tray icon.

### Settings at a glance

- **Recording:** a whole display, one window, or part of a display. The capture method can be automatic, Windows Graphics Capture, Desktop Duplication or GDI. You can choose whether to record the cursor.
- **Video:**
  - Presets: Low resource usage, Balanced, High quality, Very high quality, Custom, plus one-click recommended settings.
  - Resolution: native, 4K, 1440p, 1080p, 900p, 720p or custom.
  - Frame rate: 24 to 240 FPS, match the display, or custom.
  - Encoding: bitrate, codec (H.264, HEVC, AV1 when your hardware supports it), encoder and encoder preset.
  - Rate control: **Constant quality** (smallest files for how it looks; the bitrate becomes a ceiling), VBR or CBR.
  - **Smaller files** preset: constant quality with HEVC when your PC can encode it.
- **Audio:**
  - Game and desktop sound (any output device) and microphone, each with its own volume.
  - Push to talk: the microphone is silent until you hold a key or a side mouse button, and the key still works in your game.
  - One mixed track or separate tracks, and the audio quality.
- **Storage:**
  - Recordings folder, sub-folders per game, and a low-space warning. Recording stops automatically below 1 GB free.
  - Optional clean-up of old recordings. It's off by default and moves files to the Recycle Bin; favorites are never removed.
- **Advanced:** keyframe interval, MP4 or MKV, a list of the encoders found on your PC, and log and settings folders.

## Troubleshooting

| Problem | What to do |
|---|---|
| The recording is black | Use borderless or windowed fullscreen in the game. Some exclusive-fullscreen games and protected video (DRM streaming apps) can't be captured. Try another capture method in Settings › Recording. |
| "Falling behind: frames dropped" | The PC can't encode fast enough. Choose the Low resource usage preset, a lower resolution or frame rate, or a hardware encoder if one is listed. |
| A hotkey doesn't work | Another app may already use it; Settings › Hotkeys says so. Choose a different combination. |
| The microphone isn't recorded | Windows may block it: Settings › Privacy & security › Microphone › Let desktop apps access your microphone. |
| No game sound | Check Settings › Audio › Output device matches where you hear the game. |
| Anything else | Settings › Advanced › Open log folder. The log lists what uRec did and any errors. It never contains screen or audio content. |

## Privacy

uRec has no account, no telemetry and no uploads. Recordings, settings and logs stay on your PC. Windows notifications are created locally. Their buttons use a per-user `urec:` link that only opens or reveals existing videos in your recordings folder.

Its only network connection is the update check: once a day it asks GitHub (`api.github.com`) for the latest uRec release. The request carries nothing but a `uRec/<version>` user agent. Turn it off in **Settings › General › Check for updates**.

## Updates

When a newer version exists, the installed app downloads it in the background and checks its SHA-256 against the `SHA256SUMS.txt` published with the release. A file that doesn't match is deleted. The verified update installs silently when you quit uRec or the next time it starts. You can also press **Restart now** in Settings › General. It never installs while you're recording or Instant Replay is on, and your settings and recordings are kept.

- Turn off **Install updates automatically** to only be told about new versions.
- The portable zip never updates itself. It only shows that a new version is available.

## Uninstall

Use **Settings › Apps › Installed apps › uRec › Uninstall**. Recordings are never removed. Uninstalling asks whether to also remove your settings, and keeps them by default.

## More

- [BUILDING.md](BUILDING.md): build from source
- [TESTING.md](TESTING.md): test plan and actual results
- [DEPENDENCIES.md](DEPENDENCIES.md): versions, licenses, sources
- [CHANGELOG.md](CHANGELOG.md), [SECURITY.md](SECURITY.md)

uRec is MIT licensed. It runs FFmpeg (GPLv3) as a separate program; see `THIRD-PARTY-NOTICES.txt` in the install folder.
