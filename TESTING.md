# Testing

Every push to `main` runs these on a real Windows machine (GitHub Actions `windows-latest`: Windows Server 2025 build 26100, AMD EPYC with 2 vCPUs, **no GPU**, 1024 × 768 screen):

- formatting, clippy (warnings are errors) and unit tests
- real screen-capture integration tests (`ci/integration.ps1`)
- the UI smoke test (`ci/ui-screenshots.ps1`), including the first-run setup and its real test recording
- the Instant Replay indicator and notification tests (`ci/indicator-test.ps1`), which film the real overlay
- on release, the installer test (`ci/install-test.ps1`) and `cargo audit`, then the automatic update test (`ci/update-test.ps1`) against the release that was just published

The results below are copied from those runs. Nothing is estimated.

**Status labels:**
- ✅ **Verified (automated):** passed on CI.
- 🟡 **Implemented, not verified:** needs hardware or setups that CI doesn't have. See the manual checklist below.
- ⛔ **Not implemented.**

## Unit tests (automated)

| Crate | Tests | Covers |
|---|---|---|
| `urec-engine` | 33 | Output sizing (never upscales, even sizes, aspect), validation limits, presets per machine, bitrate scaling, storage maths, encoder names and arguments, failure explanations, FFmpeg command lines (GPU/system paths, scaling, QSV mapping, regions, GDI, audio pipes, replay ring), progress parsing, replay CSV parsing and selection, ring sizing, file-name sanitizing and timestamps, unique names, hotkey parsing, formatting and conflicts, SHA-256 (standard test vectors incl. one million 'a'). **Replay saving:** 8 threads placing clips under the same name all get separate files and none replaces another; segments recycled at the front are dropped but a missing middle segment is an error; cleanup removes only uRec's temporary names; temp names are unique. **MP4 check:** complete, truncated, unfinished (no `moov`), garbage, empty and fragmented files. **Decode report** parsing (streams, frames, decoder errors). **Constant quality:** quantizer per level and codec, ceiling-only bitrate, per-encoder arguments, regular keyframes in every mode. Push-to-talk key names (plain keys, Caps Lock, side mouse buttons). |
| `urec` (app) | 15 | Settings round-trip, corrupt-file quarantine, partial/out-of-range values, v1 → v2 migration (existing users skip setup, choices kept, unknown values reset), UTF-8 byte-order mark accepted, conversions, library scan, `ffmpeg -i` metadata parsing, duration format, update version comparison (no downgrades or pre-releases), checksum-file parsing, release JSON parsing (only `github.com/u2alr/uRec` download URLs accepted), library diagnosis of unreadable/damaged/still-writing files, temp clips hidden from the library, **clean-up rule** (only old, unstarred, Instant Replay clips unless "all"; never recent files), **update signature rules** (no key → checksum-only; with a key a signature is required, must verify, malformed input refused, the verified hash is SHA-256 of the checksum file) |
| `urec-win` | 16 | App names, overlay dot round and symmetric, HTTPS-only URLs. **Indicator timeline:** success (180 ms open, 2 s hold, 180 ms close, then idle with nothing running), failure (2.5 s hold), repeated successes extend instead of duplicating, a newer failure replaces a success and a success waiting behind a failure is shown only afterwards, a result during the close regrows without a jump, the "saving" pulse stops on a result or after 90 s, reduced motion has no in-between states. **Indicator drawing:** closed pill is exactly the dot, the dot position is stable, the pill grows away from the screen edge, no accent border (minimal design), check and cross differ in shape (not only colour). **Notification links:** round trip with spaces and accents, malformed links refused, only existing videos inside the recordings folder, XML escaping. |

## Integration tests on Windows (automated, real capture)

`ci/integration.ps1` drives `urec-cli`, which uses the same engine and Windows layer as the app. Every output file is checked with ffprobe and fully decoded. Latest run: **23 passed, 0 failed.**

| # | Test | Result |
|---|---|---|
| T01 | Encoder probe; every argument set uRec generates is accepted by FFmpeg | ✅ |
| T01b | Constant-quality argument sets for every encoder (x264/x265 CRF, NVENC CQ, AMF CQP, QSV ICQ, Media Foundation quality) are accepted by FFmpeg | ✅ |
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
| T21 | **Signed updates:** a throwaway P-256 key signs a checksum file; uRec's Windows CNG check accepts it and refuses a changed file, a changed signature and another key | ✅ |
| T22 | **Constant quality vs VBR**, same mostly-still desktop, 720p30 x264, 8 s: VBR 6 Mbps 2.11 MB, constant quality (level 7) 0.66 MB, **69 % smaller**. SSIM between the two clips: 0.996 (visually near-identical). Fast-moving game footage saves less; that wasn't measured on CI | ✅ |
| T17 | **A/V sync**: screen flashes white and a beep plays every 2 s; in the recording audio leads video by 6–55 ms (median 26–44 ms across runs, about one frame at 30 FPS; includes the test's own playback and GDI capture latency) | ✅ |
| T18 | Show in folder / Open folder open the right Explorer folder (path with spaces) | ✅ |
| T19 | **4 simultaneous Instant Replay saves to the same name** (real FFmpeg): 4 separate complete clips (`… Replay.mp4`, `(2)`, `(3)`, `(4)`), each decodes without errors, no temp files or staging folders left; 1.9 s for all four | ✅ |
| T20 | **150 s Instant Replay run:** FFmpeg memory 168.0 → 168.1 MB, host 2.0 → 2.0 MB, ring never above 19/19 files, a 20 s save at the end decodes cleanly | ✅ |

## App tests (automated)

| Test | Result |
|---|---|
| App starts, renders every page in light and dark theme, stays running | ✅ |
| **Global hotkey end to end:** real Alt+F9 keypress → recording starts (red tally, timer) → Alt+F9 → file saved to `Videos\uRec\Desktop\…mp4` | ✅ |
| Crash diagnosis: a startup crash in the overlay was found with a crash dump + `cdb` on CI and fixed | ✅ |
| **First-run setup:** all four steps render; the real 5-second test recording (started automatically on CI) reported: capture and encoder start ✅ (x264 via GDI, 960 × 720 at 30 FPS), game and desktop sound ✅ (VB-Audio cable), microphone – (off), ran 5 s ✅ (159 frames), file finalized ✅ (468 KB, crash-safe MP4), plays back ✅ (181 frames decoded, no errors), audio tracks ✅ (1 of 1) | ✅ |

## Instant Replay indicator and notifications (automated, real overlay)

`ci/indicator-test.ps1` films the corner of the screen at ~47 frames per second while the real overlay runs. Normally the overlay is excluded from capture; `UREC_OVERLAY_CAPTURABLE=1` turns that off for the test only. Latest run: **13 passed, 0 failed.**

| # | Check | Result |
|---|---|---|
| P01 | Success: the dot opens into "Instant Replay Saved" with a thin green check, holds, closes. Measured (minimal design, 0.3.0): visible 2311 ms (target ~2360), opening 126 ms, closing 95 ms after the message faded | ✅ |
| P02 | Back to the plain dot afterwards, nothing left on screen | ✅ |
| P03 | Failure: thin red cross and "Instant Replay Failed", visible 2768 ms (target ~2860) | ✅ |
| P04 | While a save runs, the dot pulses (51 of 51 frames changed) and no message appears; the pill appears 67 ms after the result | ✅ |
| P05 | Burst (success, failure 0.5 s later, success): the failure replaces the success, is shown for 2.4 s, then the later success appears | ✅ |
| P06 | Reduced motion: message appears and disappears with no in-between frames | ✅ |
| P07 | No CPU at rest: 0 ms CPU in 5 s after the animation; 47 ms in 3 s while pulsing | ✅ |
| P08 | Top-left corner: the pill grows to the right, away from the screen edge | ✅ |
| P09 | A Windows notification is accepted (it appears in the Action Center history) | ✅ |
| P10 | **Real app, real Alt+F10:** clip written, then the success pill | ✅ |
| P11 | **Real app, 4 rapid Alt+F10 presses:** 2 saves queued, 2 presses folded in, every clip complete, nothing overwritten | ✅ |
| P12 | Overlay turned off: nothing is drawn on screen for a save | ✅ |
| P13 | Overlay turned off: a Windows notification is shown instead | ✅ |

The 0.3.0 design is minimal: a slim dark pill, a thin check or cross, no border or glow, and the message fades in place. The filmed frames were reviewed: the pill stays dark until the dot reappears, with no muddy in-between colours.

In the real-app run (P10), the pill was measured as visible for about 1.95 s rather than 2.3 s. That run happens while x264 encodes Instant Replay on the same 2-vCPU machine, and the frames this test can capture under that load don't catch the first and last few frames of the animation. The standalone runs (P01, P03) are the timing reference.

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

## Automatic update tests (automated, every release)

These run against the real GitHub release that the pipeline has just published (first run: v0.1.2). An older version is simulated with `UREC_FAKE_VERSION=0.0.1`.

| # | Check | Result |
|---|---|---|
| U01 | The current build reports "up to date" (one check: 0.02 s CPU, 0.4 s) | ✅ |
| U02 | An older version finds the release, downloads the 32 MB installer and verifies it (0.27 s CPU, 3 s) | ✅ |
| U03 | The downloaded installer matches the published SHA-256 | ✅ |
| U04 | A corrupted download is rejected and deleted, and nothing is queued to install | ✅ |
| U05 | The queued update installs silently when uRec starts | ✅ |
| U06 | uRec starts again by itself after updating (5 s) and its exe was replaced | ✅ |

Not tested on CI 🟡: **Restart now** and install-on-quit from the tray (same installer call as U05, different trigger), all-users installs (these need an admin prompt), and behaviour behind a corporate proxy.

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
| 4 simultaneous saves (T19) | 1.9 s for all four |
| Memory over a 150 s Instant Replay run (T20) | FFmpeg 168.0 → 168.1 MB private; no growth |
| Overlay at rest / while animating (P07) | 0 ms CPU per 5 s / 47 ms CPU per 3 s (~1.6 % of one core during the 2.4 s animation) |
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
- Overlay hidden from recordings. CI only shows the opposite: with the exclusion turned off for the test, the overlay is captured.
- The indicator at high-DPI scaling (125–200 %) and on a secondary monitor.
- **Push-to-talk** with a real microphone. The key watching and muting logic is implemented, but CI has no microphone and no one to hold a key.
- **Constant quality on GPU encoders** (NVENC CQ, AMF CQP, QSV ICQ). FFmpeg accepts the arguments (T01b), but only x264 was measured. Hardware encoders usually save less space than x264 does in constant-quality mode.
- **"Use CPU encoder"** after a hardware encoder fails at start. CI has no hardware encoder that could fail.
- Notification buttons ("Open clip", "Show in folder") clicked by a person. The links and their checks are unit-tested, and the notification was accepted by Windows, but no click was automated.
- Low disk space behaviour (warning and automatic stop).
- 1080p60 and above on real hardware, high-refresh capture (120–240 FPS), long recordings (1 h+).

## Not implemented ⛔

- Pause and resume.
- Mouse-click visualization.
- HDR capture and encoding (8-bit SDR only).
- A code-signed installer (no certificate).
- Hiding the overlay automatically in fullscreen apps. The overlay is meant for games, so it stays; it can be turned off or toggled with a hotkey.
- Update signing is built and tested (T21), but **not switched on**: the project's key hasn't been created yet (one command, see SECURITY.md). Until then updates are checked by SHA-256 only.

## Manual checklist for real PCs

Please run these on your PC and report results (Settings › Advanced shows which encoders work; the log is in Settings › Advanced › Open log folder):

1. Install, open uRec. Note which encoder Home says it will use.
2. Record the desktop for 1 minute at the recommended preset with game sound playing. Play the file: is the audio in sync?
3. Record a game in borderless fullscreen with Alt+F9 while the game has focus.
4. Turn Instant Replay on, play for a few minutes, press Alt+F10. Watch the overlay show "Instant Replay Saved", then check the clip. Press Alt+F10 several times quickly: you should get at most two new clips, none broken.
5. Turn the overlay off (Settings › Appearance), press Alt+F10 and click **Open clip** on the Windows notification.
6. Run the first-run setup again (Settings › Advanced) and do the test recording.
7. Record a microphone with a separate track; check both tracks in an editor.
8. While recording, check Task Manager: CPU and GPU usage of `uRec.exe` and `ffmpeg.exe`, and whether frames are dropped (Home shows the count).
9. Try 1440p/4K, 120+ FPS and HEVC/AV1 if your GPU supports them.
10. Uninstall and check that recordings remain.
