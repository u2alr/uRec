# Building uRec

## What you need

| Tool | Version used for releases | Notes |
|---|---|---|
| Windows | 10/11 x64 (CI: Windows Server 2025, `windows-latest`) | |
| Rust | 1.98.1 stable, MSVC toolchain (`x86_64-pc-windows-msvc`) | The app crate needs Rust 1.95 or newer (egui 0.36) |
| Visual Studio Build Tools | 2022, "Desktop development with C++" | Linker and `rc.exe` for the icon/manifest |
| PowerShell | 7 (`pwsh`) | Build scripts |
| Inno Setup | 6 | Only for the installer |
| FFmpeg | 9.0.2 essentials (gyan.dev) | Downloaded and checksum-checked by `ci/get-ffmpeg.ps1` |

## Build and run

```powershell
git clone https://github.com/u2alr/uRec
cd uRec
./ci/get-ffmpeg.ps1                # puts ffmpeg.exe in third_party\ffmpeg
cargo build --release --locked     # target\release\uRec.exe and urec-cli.exe
target\release\uRec.exe
```

During development, uRec finds FFmpeg next to the exe, in `third_party\ffmpeg`, through `UREC_FFMPEG`, or on `PATH`.

## Installer and portable zip

```powershell
./ci/build-installer.ps1           # needs a release build first
```

This produces:

- `dist\uRec-<version>-setup.exe`: per-user Inno Setup installer
- `dist\uRec-<version>-portable.zip`
- `dist\uRec\`: the files that go into both, including `THIRD-PARTY-NOTICES.txt` generated from `cargo metadata`

`ci/install-test.ps1 -Setup dist\uRec-<version>-setup.exe` installs silently, checks shortcuts and registration, measures idle and Instant Replay resource use, uninstalls, and checks what was kept.

## Releases

Push a tag such as `v0.1.0`. The `Release` workflow runs these steps:

1. Builds with `--locked`.
2. Runs `cargo audit`.
3. Builds the installer and zip.
4. Runs the install/uninstall test.
5. Attaches the files to a GitHub release.

The installer is not code-signed; no signing certificate is available.

## Workspace layout

| Crate | Purpose |
|---|---|
| `crates/engine` | Platform-neutral, std-only engine: video settings and presets, encoder selection and arguments, FFmpeg command lines, capture fallback, process supervision, Instant Replay ring and saving, file naming, hotkey parsing. Builds and unit-tests on any OS. |
| `crates/win` | Windows layer: WASAPI loopback/mic capture to named pipes, global hotkeys (`RegisterHotKey`), monitors (GDI + DXGI), windows, recording overlay (GDI), job object, folder picker, Recycle Bin, startup entry. Stubs on other OSes. |
| `crates/app` | The `uRec.exe` app: egui UI, settings, recording controller, library, tray. |
| `crates/cli` | `urec-cli`: drives the engine from a terminal; used by the automated Windows tests. |

## Checks

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --release -- -D warnings
cargo test --workspace --release
./ci/integration.ps1 -Cli target\release\urec-cli.exe   # real screen capture tests
./ci/ui-screenshots.ps1 -Exe target\release\uRec.exe -Cli target\release\urec-cli.exe
```

CI runs all of these on every push to `main` (`.github/workflows/ci.yml` and `ui.yml`).
