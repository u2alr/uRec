# Testing

Every push to `main` runs these on a real Windows machine (GitHub Actions `windows-latest`: Windows Server 2025 build 26100, AMD EPYC with 2 vCPUs, **no GPU**, 1024 × 768 screen):

- formatting, clippy (warnings are errors) and unit tests
- real screen-capture integration tests (`ci/integration.ps1`)
- the UI smoke test (`ci/ui-screenshots.ps1`)
- on release, the installer test (`ci/install-test.ps1`) and `cargo audit`

The results below are copied from those runs. Nothing is estimated.

**Status labels:**
- ✅ **Verified (automated):** passed on CI.
- 🟡 **Implemented, not verified:** needs hardware or setups that CI doesn't have. See the manual checklist below.
- ⛔ **Not implemented.**

## Unit tests (automated)

| Crate | Tests | Covers |
|---|---|---|
| `urec-engine` | 21 | Output sizing (never upscales, even sizes, aspect), validation limits, presets per machine, bitrate scaling, storage maths, encoder names and arguments, failure explanations, FFmpeg command lines (GPU/system paths, scaling, QSV mapping, regions, GDI, audio pipes, replay ring), progress parsing, replay CSV parsing and selection, ring sizing, file-name sanitizing and timestamps, unique names, hotkey parsing, formatting and conflicts |
| `urec` (app) | 8 | Settings round-trip, corrupt-file quarantine, partial/out-of-range values, conversions, library scan, `ffmpeg -i` metadata parsing, duration format |
| `urec-win` | 1 | App names from window titles/executables |

## Integration tests on Windows (automated, real capture)

`ci/integration.ps1` drives `urec-cli`, which uses the same engine and Windows layer as the app. Every output file is checked with ffprobe and fully decoded. Latest run: **17 passed, 0 failed.**

| # | Test | Result |
|---|---|---|
| T01 | Encoder probe; every argument set uRec generates is accepted by FFmpeg | ✅ |
| T02 | GDI capture, x264, 720p30 (960 × 720 from a 4:3 screen, aspect kept) | ✅ |
| T03 | GDI capture, Windows Media Foundation encoder (CPU), native 30 FPS, with audio | ✅ |
| T04 | Native 60 FPS | ✅ |
| T05 | Automatic fallback WGC → DDA → GDI when GPU capture isn't available | ✅ |
| T06 | MKV container | ✅ |
| T07 | HEVC (x265), 720p30 | ✅ |
| T08 | Custom 800 × 600 output, letterboxed | ✅ |
| T09 | Oversized H.264 resolution refused with a clear message | ✅ |
| T10 | Instant Replay: 18 s run with a 10 s ring, save last 8 s → 10.3 s clip with audio, ring bounded at 9/9 files, no decode errors | ✅ |
| T11 | 5 start/stop cycles; all files valid; no FFmpeg left running | ✅ |
| T12 | Monitor enumeration (size, primary, DXGI mapping) | ✅ |
| T13 | Game/desktop sound: WASAPI loopback records a playing 1 kHz tone at −15 dB | ✅ |
| T14 | Separate audio tracks: one track per working source | ✅ |
| T15 | Microphone blocked by Windows privacy: recording continues, mic honestly reported as not recording, no fake track | ✅ |
| T16 | Window capture (Notepad only, 754 × 512) | ✅ |
| T17 | **A/V sync**: screen flashes white and a beep plays every 2 s; in the recording audio leads video by 6–55 ms (median 26–44 ms across runs, about one frame at 30 FPS; includes the test's own playback and GDI capture latency) | ✅ |

## App tests (automated)

| Test | Result |
|---|---|
| App starts, renders every page in light and dark theme, stays running | ✅ |
| **Global hotkey end to end:** real Alt+F9 keypress → recording starts (red tally, timer) → Alt+F9 → file saved to `Videos\uRec\Desktop\…mp4` | ✅ |
| Crash diagnosis: a startup crash in the overlay was found with a crash dump + `cdb` on CI and fixed | ✅ |

## Installer tests (automated, every release)

| # | Check | Result |
|---|---|---|
| I01 | Silent per-user install, no admin prompt (6.5 s) | ✅ |
| I02 | Files installed: `uRec.exe`, `ffmpeg.exe`, license and notices | ✅ |
| I03 | Start Menu shortcut | ✅ |
| I04 | Optional desktop shortcut | ✅ |
| I05 | Listed in Apps & features with name and icon | ✅ |
| I06 | Doesn't start with Windows unless chosen | ✅ |
| I07 | Installed app launches and stays running | ✅ |
| I08 | Idle with Instant Replay off: no FFmpeg running | ✅ |
| I09 | Idle CPU near zero | ✅ |
| I10 | Instant Replay from the installed app, buffer bounded | ✅ |
| I11 | Killing uRec also ends its FFmpeg (job object) | ✅ |
| I12 | Silent uninstall succeeds | ✅ |
| I13 | Program files, shortcuts and Apps entry removed | ✅ |
| I14 | Recordings kept | ✅ |
| I15 | Settings kept | ✅ |
| I16 | Temporary data (replay buffer, thumbnails, logs) removed | ✅ |

Security audit: `cargo audit` found **0 vulnerabilities in 333 crates**.

## Performance (measured on the CI machine)

These numbers are from a 2-vCPU VM **without a GPU**. There, the UI is software-rendered (WARP) and encoding runs on the CPU, so they aren't representative of a gaming PC. Real-hardware numbers are still pending (see below).

| Measurement | Result |
|---|---|
| Installer size | 32.1 MB |
| Installed size | 116.5 MB (FFmpeg is ~100 MB of that) |
| Portable zip | 43.5 MB |
| Idle CPU, window open, Instant Replay off (3 × 10 s, after 15 s settle) | 0 % |
| Idle memory | 58 MB working set, 278 MB private (most of it the software renderer on this GPU-less VM) |
| Instant Replay on, 15 s buffer, x264 on CPU | uRec 0 % CPU, FFmpeg 5.4 % CPU / 60 MB; buffer 12 files, 1.4 MB |
| Recording start latency (CLI, GDI) | 0.6–1.4 s |
| Stop/finalize latency | 0.1–0.25 s |
| Instant Replay save (8 s clip) | ~1.8 s (includes waiting for the current 2 s segment to close) |
| Dropped frames, x264 720p30 on 2 vCPUs | 0–3 per 5 s; 60 FPS: up to 4 dropped. The app shows "Falling behind" when this happens |

Performance fixes made because of these measurements:
- **Idle CPU, 2 % → 0 %:** the UI no longer wakes for unchanged disk-space readings.
- **uRec CPU with Instant Replay on, 67 % → 0 %:**
  - The overlay re-enumerated monitors 4×/s.
  - Live stats woke the UI 4×/s.
- **Install size, 217 MB → 117 MB:** `ffprobe.exe` was replaced by `ffmpeg -i`.

## Not verified yet (needs real hardware) 🟡

GitHub's machine has no GPU and no microphone, so these are implemented but haven't been run:

- Hardware encoding: NVIDIA NVENC, AMD AMF (reference PC: RX 6600 XT), Intel Quick Sync, Windows hardware MFT. Their FFmpeg arguments are built and parsed the same way as the tested ones, but no hardware encode has run yet.
- Windows Graphics Capture and Desktop Duplication (the CI machine can't create a Direct3D 11 device; GDI capture is verified).
- The zero-copy GPU frame path.
- Game capture in real games (borderless, windowed and exclusive fullscreen).
- A real microphone, device unplugging and reconnecting, Windows 10 (only build 26100 tested), multiple monitors and high-DPI scaling.
- Overlay hidden from recordings.
- Low disk space behaviour (warning and automatic stop).
- 1080p60 and above on real hardware, high-refresh capture (120–240 FPS), long recordings (1 h+).

## Not implemented ⛔

- Pause and resume.
- Mouse-click visualization.
- HDR capture and encoding (8-bit SDR only).
- A code-signed installer (no certificate).

## Manual checklist for real PCs

Please run these on your PC and report results (Settings › Advanced shows which encoders work; the log is in Settings › Advanced › Open log folder):

1. Install, open uRec. Note which encoder Home says it will use.
2. Record the desktop for 1 minute at the recommended preset with game sound playing. Play the file: is the audio in sync?
3. Record a game in borderless fullscreen with Alt+F9 while the game has focus.
4. Turn Instant Replay on, play for a few minutes, press Alt+F10. Check the clip.
5. Record a microphone with a separate track; check both tracks in an editor.
6. While recording, check Task Manager: CPU and GPU usage of `uRec.exe` and `ffmpeg.exe`, and whether frames are dropped (Home shows the count).
7. Try 1440p/4K, 120+ FPS and HEVC/AV1 if your GPU supports them.
8. Uninstall and check that recordings remain.
