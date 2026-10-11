//! Non-Windows stand-ins so the workspace builds and the engine tests run anywhere.

use crate::{AudioDevice, AudioSettings, MonitorInfo, SourceState, WindowInfo};
use std::path::Path;
use urec_engine::pipeline::AudioInput;

pub fn monitors() -> Vec<MonitorInfo> {
    Vec::new()
}
pub fn windows() -> Vec<WindowInfo> {
    Vec::new()
}
pub fn foreground_window() -> Option<WindowInfo> {
    None
}
pub fn audio_outputs() -> Vec<AudioDevice> {
    Vec::new()
}
pub fn audio_inputs() -> Vec<AudioDevice> {
    Vec::new()
}
pub fn disk_space(_path: &Path) -> Option<(u64, u64)> {
    None
}
pub fn windows_build() -> Option<u32> {
    None
}
pub fn supports_wgc() -> bool {
    false
}
pub fn supports_exclude_from_capture() -> bool {
    false
}
pub fn set_dpi_aware() {}
pub fn exclude_from_capture(_hwnd: u64) -> bool {
    false
}
pub fn bind_child_to_app(_raw: *mut std::ffi::c_void) -> bool {
    false
}
pub fn startup_enabled(_app: &str) -> bool {
    false
}
pub fn set_startup(_app: &str, _command: Option<&str>) -> bool {
    false
}
pub fn reveal_in_explorer(_path: &Path) {}

pub struct AudioFeed;

impl AudioFeed {
    pub fn start(_s: &AudioSettings) -> (AudioFeed, Vec<AudioInput>) {
        (AudioFeed, Vec::new())
    }
    pub fn states(&self) -> (SourceState, SourceState) {
        (SourceState::Off, SourceState::Off)
    }
    pub fn levels(&self) -> (f32, f32) {
        (0.0, 0.0)
    }
    pub fn set_volumes(&self, _system: f32, _mic: f32) {}
    pub fn set_mic_muted(&self, _muted: bool) {}
    pub fn mic_muted(&self) -> bool {
        false
    }
}

pub struct Hotkeys;

impl Hotkeys {
    pub fn new(_on_press: impl Fn(usize) + Send + Sync + 'static) -> Self {
        Hotkeys
    }
    pub fn set(&mut self, _keys: &[Option<urec_engine::hotkey::Hotkey>]) -> Vec<usize> {
        Vec::new()
    }
    pub fn stop(&mut self) {}
}

pub fn local_time() -> (i32, u32, u32, u32, u32, u32) {
    let secs =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let s = urec_engine::naming::Stamp::from_unix(secs, 0);
    (s.year, s.month, s.day, s.hour, s.minute, s.second)
}
pub fn play_ding() {}
pub fn recycle(path: &Path) -> bool {
    std::fs::remove_file(path).is_ok()
}
pub fn show_window_titled(_title: &str) -> bool {
    false
}
pub fn hide_window_titled(_title: &str) -> bool {
    false
}
pub fn find_window(_title: &str) -> Option<u64> {
    None
}
pub fn claim_single_instance(_name: &str) -> bool {
    true
}
pub fn pick_folder(_title: &str, _start: Option<&Path>) -> Option<std::path::PathBuf> {
    None
}

#[derive(Default)]
pub struct Overlay;

impl Overlay {
    pub fn new() -> Self {
        Overlay
    }
    pub fn disabled() -> Self {
        Overlay
    }
    pub fn set(&self, _st: crate::OverlayState) {}
    pub fn notify(&self, _ev: crate::OverlayEvent, _reduced_motion: bool) -> bool {
        false
    }
}
pub fn open_file(path: &Path) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}
pub fn videos_dir() -> Option<std::path::PathBuf> {
    None
}

pub fn https_get(_url: &str, _user_agent: &str, _headers: &[&str], _max_bytes: usize) -> Result<Vec<u8>, String> {
    Err("HTTPS is only implemented on Windows".into())
}
pub fn https_download(
    _url: &str,
    _user_agent: &str,
    _dest: &Path,
    _progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<u64, String> {
    Err("HTTPS is only implemented on Windows".into())
}
pub fn open_url(_url: &str) {}
pub fn system_reduced_motion() -> bool {
    false
}
pub const AUMID: &str = "u2alr.uRec";
pub fn register_notifications(_exe: &Path) -> bool {
    false
}
pub fn show_toast(_xml: &str) -> Result<(), String> {
    Err("notifications are only implemented on Windows".into())
}
pub fn toast_history_count() -> Option<u32> {
    None
}
pub fn work_area() -> Option<(f32, f32, f32, f32)> {
    None
}
