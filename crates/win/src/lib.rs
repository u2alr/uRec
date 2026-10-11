//! Windows platform layer for uRec.
//!
//! Everything here is Windows-only; on other platforms the functions return empty
//! results so the rest of the workspace (engine, tests) still builds and runs.

pub use urec_engine::pipeline::{CaptureTarget, Rect};

#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod display;
#[cfg(windows)]
mod hotkeys;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod sys;

#[cfg(not(windows))]
mod stub;

#[cfg(windows)]
pub use audio::{audio_inputs, audio_outputs, AudioFeed};
#[cfg(windows)]
pub use display::{foreground_window, monitors, windows};
#[cfg(windows)]
pub use hotkeys::Hotkeys;
#[cfg(windows)]
pub use overlay::Overlay;
#[cfg(windows)]
pub use sys::*;

#[cfg(not(windows))]
pub use stub::*;

/// A connected display.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    /// "Display 1" style name plus resolution, for the UI.
    pub name: String,
    /// GDI device name, e.g. `\\.\DISPLAY1`.
    pub device: String,
    pub hmonitor: u64,
    pub bounds: Rect,
    pub primary: bool,
    pub refresh_hz: Option<u32>,
    /// DXGI adapter / output indices. Desktop Duplication in FFmpeg only sees outputs on adapter 0.
    pub dxgi_adapter: Option<u32>,
    pub dxgi_output: Option<u32>,
}

impl MonitorInfo {
    pub fn target(&self) -> CaptureTarget {
        CaptureTarget::Monitor { index: self.dxgi_output.unwrap_or(0), hmonitor: self.hmonitor, bounds: self.bounds }
    }

    /// Desktop Duplication can capture this monitor.
    pub fn dda_capable(&self) -> bool {
        self.dxgi_adapter == Some(0) && self.dxgi_output.is_some()
    }
}

/// A top-level application window that can be captured.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub hwnd: u64,
    pub title: String,
    /// Executable file name without extension, e.g. "VALORANT-Win64-Shipping".
    pub exe: String,
    pub bounds: Rect,
}

impl WindowInfo {
    pub fn target(&self) -> CaptureTarget {
        CaptureTarget::Window { hwnd: self.hwnd, title: self.title.clone(), bounds: self.bounds }
    }

    /// Short name used for folders and file names ("VALORANT", "Minecraft").
    pub fn app_name(&self) -> String {
        let exe = self.exe.split(['-', '_']).next().unwrap_or("").trim();
        let generic = ["", "javaw", "java", "python", "pythonw", "applicationframehost", "explorer"];
        if !generic.contains(&exe.to_ascii_lowercase().as_str()) && exe.len() >= 3 {
            return exe.to_string();
        }
        self.title.split([' ', '-', '|', '—']).find(|s| !s.is_empty()).unwrap_or("Desktop").to_string()
    }
}

/// An audio endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// How captured audio is laid out in the output file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackLayout {
    /// One track: game/desktop sound and microphone mixed together. Plays everywhere.
    Mixed,
    /// Track 1: game/desktop sound, track 2: microphone. For editing; many players only play track 1.
    Separate,
}

/// Which sound to capture.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioSettings {
    pub system: bool,
    /// Render endpoint to capture (loopback). None = Windows default output.
    pub system_device: Option<String>,
    pub system_volume: f32,
    pub mic: bool,
    /// Capture endpoint. None = Windows default microphone.
    pub mic_device: Option<String>,
    pub mic_volume: f32,
    pub mic_muted: bool,
    pub layout: TrackLayout,
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings {
            system: true,
            system_device: None,
            system_volume: 1.0,
            mic: false,
            mic_device: None,
            mic_volume: 1.0,
            mic_muted: false,
            layout: TrackLayout::Mixed,
        }
    }
}

/// State of one audio source as shown in the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceState {
    Off,
    /// Initialized and receiving data from Windows.
    Active {
        device: String,
    },
    /// The device disappeared; recording continues with silence and retries.
    Lost {
        device: String,
    },
    Failed {
        reason: String,
    },
}

/// What the recording overlay shows.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayState {
    pub visible: bool,
    /// Dot colour (r, g, b): red while recording, amber while Instant Replay buffers.
    pub dot: (u8, u8, u8),
    /// "top_right" | "top_left" | "bottom_right" | "bottom_left"
    pub corner: String,
    /// Monitor rectangle to place it on (x, y, w, h).
    pub monitor: (i32, i32, u32, u32),
}

impl Default for OverlayState {
    fn default() -> Self {
        OverlayState { visible: false, dot: (229, 72, 77), corner: "top_right".into(), monitor: (0, 0, 1920, 1080) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names() {
        let w = |exe: &str, title: &str| WindowInfo {
            hwnd: 1,
            title: title.into(),
            exe: exe.into(),
            bounds: Rect { x: 0, y: 0, width: 1, height: 1 },
        };
        assert_eq!(w("VALORANT-Win64-Shipping", "VALORANT  ").app_name(), "VALORANT");
        assert_eq!(w("javaw", "Minecraft 1.21").app_name(), "Minecraft");
        assert_eq!(w("chrome", "YouTube - Google Chrome").app_name(), "chrome");
    }
}
