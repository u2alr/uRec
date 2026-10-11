# Dependencies

Versions are pinned (`=` in `Cargo.toml`, `Cargo.lock` committed, release builds use `--locked`). FFmpeg is pinned by version, URL and SHA-256.

## Bundled with the app

| Component | Version | License | Source | Purpose | Redistribution |
|---|---|---|---|---|---|
| FFmpeg (`ffmpeg.exe`) | 9.0.2, "essentials" build by gyan.dev | GPL-3.0 | https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-9.0.2-essentials_build.zip (source: https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz) | Screen capture filters (`gfxcapture`, `ddagrab`, `gdigrab`), encoding (NVENC, AMF, Quick Sync, Media Foundation, x264, x265, AAC), muxing, replay joining, thumbnails | Allowed. Shipped as a separate program with its license (`FFMPEG-LICENSE.txt`) and a source link in `THIRD-PARTY-NOTICES.txt`. uRec itself stays MIT. |

`ffmpeg.exe` SHA-256: `3256173F3F8BFFD7DF12227C68ADF68025EDB1832273A9530688A7BB1ED8EDEC`. It's checked by `ci/get-ffmpeg.ps1`, and `UREC_STRICT_FFMPEG=1` turns a mismatch into an error.

Why the GPL build: x264 and x265 (the CPU fallback encoders that make uRec work on PCs without a usable hardware encoder) are GPL. Running FFmpeg as a separate process keeps uRec's own code MIT.

## Rust crates (direct)

| Crate | Version | License | Used in | Purpose |
|---|---|---|---|---|
| `windows` | 0.62.2 | MIT OR Apache-2.0 | win | Win32/WASAPI/DXGI/COM bindings (Microsoft) |
| `eframe` / `egui` | 0.36.2 | MIT OR Apache-2.0 | app | UI. Renderers: wgpu (default) and OpenGL/glow (fallback via `UREC_RENDERER=glow`) |
| `tray-icon` | 0.26.1 | MIT OR Apache-2.0 | app | System tray icon and menu |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | app | Settings serialization |
| `serde_json` | 1.0.152 | MIT OR Apache-2.0 | app | Settings and library cache files |
| `embed-resource` | 3.0.12 | MIT | app (build only) | Icon, version info and manifest in `uRec.exe` |

The engine crate (`urec-engine`) has no dependencies. The full list of transitive crates with their licenses is generated at build time into `THIRD-PARTY-NOTICES.txt`, from `cargo metadata` filtered to the Windows target. All are permissive (MIT, Apache-2.0, BSD, Zlib, Unicode, ISC, or dual-licensed with them).

Newest-version notes:
- `windows-core` 0.100 exists, but `windows` 0.62.2 is the newest release of the `windows` crate itself. It depends on its matching `windows-core` and is the version used.
- egui 0.36 requires Rust 1.95. The engine and CLI build with Rust 1.85 or newer.

## Build and test tools

| Tool | Version | Purpose |
|---|---|---|
| Rust (MSVC) | 1.98.1 stable | Compiler used by CI and releases |
| Inno Setup | 6 (preinstalled on the GitHub `windows-latest` image) | Installer |
| GitHub Actions `windows-latest` | Windows Server 2025, build 26100 | Build, tests, packaging |
| `actions/checkout` | v4 | CI |
| `Swatinem/rust-cache` | v2 | CI build cache |
| `LABSN/sound-ci-helpers` | v1 | Installs a virtual audio device (VB-Audio Virtual Cable) on CI for audio tests |
| `actions/upload-artifact` | v4 | CI artifacts |
| `softprops/action-gh-release` | v2 | Publishing releases |
| `cargo-audit` | latest at build time | RustSec vulnerability audit on every release |

## Windows features used

| Feature | Minimum Windows | Fallback |
|---|---|---|
| Windows Graphics Capture (via FFmpeg `gfxcapture`) | 10 version 1903 (build 18362) | Desktop Duplication, then GDI |
| Desktop Duplication (via FFmpeg `ddagrab`) | 8 | GDI |
| WASAPI loopback + AUTOCONVERTPCM | 10 | Converts the device's own float format |
| `WDA_EXCLUDEFROMCAPTURE` (overlay hidden from recordings) | 10 version 2004 (build 19041) | Overlay may appear in recordings; the UI says so |
| WinHTTP (update checks and downloads) | 7 | Updates unavailable; uRec works normally |
| Installer `MinVersion` | 10 version 1809 (build 17763) | |
