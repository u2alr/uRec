# Changelog

## 0.4.5

- **Instant Replay no longer turns into a slideshow in some games.** On some PCs Windows Graphics Capture delivers about one new picture per second while a fullscreen game runs, so clips looked like 1 fps. uRec now notices that, switches to Desktop Duplication on its own, restarts Instant Replay, and remembers it for next time. (Only with capture method set to Automatic; the Instant Replay buffer starts over once when this happens.)

## 0.4.4

Starting on older and weaker PCs.

- **uRec now opens on PCs where the graphics driver can't draw the window.** If the default renderer fails, uRec switches to OpenGL by itself, and remembers it if the previous start never drew a window. If nothing works it says so and points to the log instead of closing silently.
- **Faster start-up.** The encoder tests that run when uRec starts now run three at a time instead of one by one, so Instant Replay starts sooner on slow CPUs. Each test also gives up sooner.
- **Launching uRec twice** now waits for the first one to finish starting, and tells you if it is stuck, instead of doing nothing.

## 0.4.3

Bug fixes from a code audit.

- **Crash at launch fixed.** uRec could close right after starting when the PC had been on for less than an hour, which is exactly when "Start with Windows" launches it. Two start-up timers subtracted from the system clock in a way Windows doesn't allow that early.
- **Editor: dragging a piece to the right landed one place too far.** It now drops where the marker shows, including at the very end.
- **Editor: undo and redo** no longer leave the playhead past the end of a shorter timeline (the clock could read "0:07 / 0:05") or highlight the wrong piece. Deleting, undoing or redoing while the preview plays now carries on from the playhead with the new cuts, instead of playing the old ones.
- **Editor:** keys (Space, S, Delete, Ctrl+Z) no longer act in the middle of a mouse drag or behind the Export window.
- **Library:** renaming now works for case-only changes ("clip" to "Clip"), and typing the extension ("clip.mp4") no longer makes "clip.mp4.mp4".
- **Names** like "con.txt" or "Lpt1.backup", which Windows reserves, are now fixed up like "con".

## 0.4.2

**New**
- **Copy a video** (Library › More › Copy, or Copy in Select mode for several): puts the video file on the clipboard, so you can paste it straight into Discord, a chat, an email or a folder.
- Select mode: Ctrl+A selects all, Delete asks to delete the selection, Esc leaves.

**Fixed and polished**
- Resolution choices show the size they really record at on your display (e.g. "720p · 960 × 720" on a 4:3 screen) and no longer offer sizes above your display, which uRec never upscales to.
- Settings: labels use the room short controls leave (no more two-line "Keep running in the tray when closed"), and every label is centred with its control.
- Sliders (volume, quality, overlay opacity, editor volume) are thin and match the rest of the app.
- Editor: the timeline stays fitted after cuts, with no empty strip while playing; ruler labels read "0.5 s" like the rest.
- Home: the "Mute mic" button only appears when a microphone is being recorded.

## 0.4.1

- **Fixed:** in the Library, Play, Edit, Show in folder and clicking a thumbnail did nothing (since 0.3.0). The row's click area for Select mode sat on top of its buttons; it's now only active while selecting. CI now clicks these with the real mouse on every build.

## 0.4.0

**Clip editor**
- **Edit** any recording from the Library, or select several clips and choose **Edit together** to join them into one video.
- **Cut anything out:** split at the playhead (S), delete pieces (Delete), drag a piece's edges to trim, and drag pieces to reorder. Undo and redo cover every step.
- **Timeline:** thumbnails and a waveform, so loud moments are easy to find. Ctrl+wheel zooms and the wheel scrolls.
- **Preview** plays in the app with sound. Space plays and pauses, the arrow keys step one frame (Shift: one second), and clicking the timeline jumps there.
- **Crop:** free, 16:9, 9:16 for Shorts/TikTok/Reels, 1:1 or 4:5, with a draggable box.
- **Audio:** mute, or change the volume.
- **Export** always makes a new file next to the original, which is never changed.
  - **Exact:** frame-accurate cuts, re-encoded with your graphics card when possible, in H.264 or HEVC.
  - **Lossless and fast:** no quality loss, done in moments. Each piece may start up to 2 s early, because a copy has to start on a keyframe.
  - Progress bar and Cancel. A finished export is checked for damage and never replaces an existing file.

## 0.3.0

**Smaller files**
- New **Constant quality** rate control. The encoder spends data where the picture needs it, so menus, desktops and quiet moments take far less space. Your bitrate setting becomes a ceiling.
- Works with every encoder: NVIDIA, AMD, Intel, Windows and the CPU.
- New **Smaller files** preset: 1080p, constant quality, and HEVC where this PC can encode it.

**Library**
- **Select** mode: pick several recordings, or all of them, and move them to the Recycle Bin in one go.
- Rows size to their content, so the action links can no longer spill out of their row at larger text sizes.
- Actions are now **Play**, **Show in folder** and **More** (Rename, Copy path, Delete).
- Optional clean-up (Settings › Storage): moves recordings older than N days to the Recycle Bin. It's off unless you turn it on, covers Instant Replay clips only by default, and never touches favorites.

**Microphone**
- **Push to talk** (Settings › Audio). The microphone is silent until you hold the key (Caps Lock by default) or a side mouse button.
- uRec only watches the key, so it still works in your game.

**Overlay**
- A cleaner, more minimal save message: a slim dark pill, a thin check or cross, plain text, no border or glow.
- The message fades in place, and the closing animation no longer shows muddy colours.

**Reliability**
- If the graphics card's encoder fails to start, uRec offers **Use CPU encoder**, with a warning about the extra CPU load. It never switches on its own.
- A failed Instant Replay save plays the Windows error sound (when save sounds are on).
- Long window titles and device names no longer overlap other settings.
- Plain-letter global hotkeys are refused, because they would stop that letter working in every other app.
- **Copy diagnostics** (Settings › Advanced) gives a plain summary of this PC and your settings for bug reports. It contains no file names.

**Updates**
- Updates can now be signed: the release's checksum list carries an ECDSA signature, checked with Windows' own cryptography.
- Signing turns on once the project creates its key (see SECURITY.md). Until then, updates are checked by SHA-256 only, as before.

## 0.2.0

**Instant Replay saves are reliable under pressure**
- Saves run one at a time. One more can wait, and further presses are included in the waiting save, so mashing the hotkey can't start competing jobs.
- Fixed: two saves at nearly the same moment could corrupt each other, because both used the same temporary folder.
- Fixed: two saves in the same second could overwrite each other. A clip never replaces an existing file now; the second one gets a " (2)" name.
- A clip is reported as saved only after its MP4 structure has been checked and the file is flushed to disk. Damaged results are discarded and reported.
- A segment missing in the middle of the buffer is now an error, never a silent gap in the clip.
- Quitting or changing settings waits for saves in progress. Leftovers from interrupted saves are cleaned up at the next start, and only uRec's own temporary files are ever removed.

**Animated save indicator**
- The overlay dot opens into "Instant Replay Saved" (green check) or "Instant Replay Failed" (red cross), then closes again: about 2.4 s for a save, 2.9 s for a failure.
- The dot dims gently while a save runs. Nothing animates at rest.
- When results arrive close together, a newer failure replaces a success, a success never hides a failure, and repeated successes extend the message instead of stacking.
- Reduced motion follows Windows' "Animation effects" or a new setting in Appearance.
- With the overlay off, save results appear as Windows notifications with **Open clip** and **Show in folder**.
- Overlay size and opacity settings.

**First-run setup**
- Shows what was found on this PC: capture methods, the encoders that actually work, and sound devices.
- Recommends a preset and shows the approximate size per minute for each one.
- Shows where files go and how much space the Instant Replay buffer uses.
- Offers a real 5-second test recording that reports each stage: capture, encoding, sound, file finalization, and a full decode.
- Can be skipped, and run again from Settings › Advanced. People upgrading from 0.1.x don't see it.

**Library**
- Favorites, with a "Favorites only" filter.
- Damaged or unfinished files are marked.
- Play, Show in folder, Rename and Delete cope with files that were moved or deleted outside uRec.

**Other**
- The "Low-end PC" preset is now called "Low resource usage".
- Settings file version 2, migrated automatically.

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
