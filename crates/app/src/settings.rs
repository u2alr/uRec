//! Persistent, versioned settings stored as JSON in %APPDATA%\uRec\settings.json.
//!
//! Loading never fails: unknown fields are ignored, missing ones get defaults, values are
//! clamped into valid ranges, and a corrupt file is moved aside (settings.corrupt-<time>.json)
//! before starting from defaults.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use urec_engine::encoder::Backend;
use urec_engine::pipeline::Container;
use urec_engine::video::{Codec, EncoderEffort, FrameRate, RateControl, Resolution, VideoSettings};

pub const SETTINGS_VERSION: u32 = 2;
pub const APP_NAME: &str = "uRec";

pub fn data_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(APP_NAME)
}

pub fn local_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(APP_NAME)
}

pub fn default_output_dir() -> PathBuf {
    urec_win::videos_dir()
        .unwrap_or_else(|| {
            std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Videos")
        })
        .join(APP_NAME)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub first_run_done: bool,
    /// The first-run setup was finished or skipped.
    pub setup_done: bool,
    pub general: General,
    pub capture: Capture,
    pub video: Video,
    pub audio: Audio,
    pub replay: Replay,
    pub hotkeys: Hotkeys,
    pub storage: Storage,
    pub appearance: Appearance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub minimize_to_tray: bool,
    pub notifications: bool,
    pub sound_on_save: bool,
    /// Game-friendly mode: fewer UI updates, GPU-only frame path, warnings when falling behind.
    pub performance_mode: bool,
    /// Lower Instant Replay quality automatically if encoding can't keep up (always announced).
    pub auto_adjust: bool,
    /// Ask GitHub once a day whether a newer uRec exists.
    pub check_updates: bool,
    /// Download verified updates in the background and install them when uRec quits or restarts.
    pub auto_update: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capture {
    /// "monitor" | "window" | "region"
    pub kind: String,
    /// GDI device name of the monitor (\\.\DISPLAY1); empty = primary.
    pub monitor: String,
    pub window_title: String,
    pub window_exe: String,
    /// Region relative to the monitor: x, y, width, height.
    pub region: [i32; 4],
    /// "auto" | "wgc" | "dda" | "gdi"
    pub method: String,
    pub cursor: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    /// "low" | "balanced" | "high" | "very_high" | "custom"
    pub preset: String,
    /// 0 = native, otherwise target height (2160/1440/1080/900/720); custom uses custom_w/h.
    pub height: u32,
    pub custom: bool,
    pub custom_w: u32,
    pub custom_h: u32,
    /// 0 = match display refresh rate.
    pub fps: u32,
    /// "h264" | "hevc" | "av1"
    pub codec: String,
    pub bitrate_kbps: u32,
    /// "vbr" | "cbr"
    pub rate_control: String,
    /// "speed" | "balanced" | "quality"
    pub effort: String,
    pub keyframe_seconds: u32,
    /// "auto" or a backend id ("amf", "nvenc", "qsv", "mf_hw", "software", "mf").
    pub encoder: String,
    /// "mp4" | "mkv"
    pub container: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    pub system: bool,
    pub system_device: String,
    pub system_volume: f32,
    pub mic: bool,
    pub mic_device: String,
    pub mic_volume: f32,
    /// "mixed" | "separate"
    pub layout: String,
    pub bitrate_kbps: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Replay {
    pub enabled: bool,
    pub seconds: u32,
    /// Where the rolling buffer lives. Empty = %LOCALAPPDATA%\uRec\replay.
    pub buffer_dir: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub record: String,
    pub save_replay: String,
    pub toggle_replay: String,
    pub mute_mic: String,
    pub overlay: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Storage {
    pub output_dir: String,
    /// Put recordings in a sub-folder named after the game/app.
    pub folder_per_app: bool,
    /// Warn below this much free space (GB). Recording stops automatically at 1 GB.
    pub low_space_gb: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// "system" | "dark" | "light"
    pub theme: String,
    pub overlay: bool,
    /// "top_right" | "top_left" | "bottom_right" | "bottom_left"
    pub overlay_corner: String,
    pub ui_scale: f32,
    /// "system" (follow Windows animation effects) | "reduced" | "full"
    pub motion: String,
    /// "small" | "medium" | "large"
    pub overlay_size: String,
    /// Dot opacity in percent (30-100).
    pub overlay_opacity: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: SETTINGS_VERSION,
            first_run_done: false,
            setup_done: false,
            general: General::default(),
            capture: Capture::default(),
            video: Video::default(),
            audio: Audio::default(),
            replay: Replay::default(),
            hotkeys: Hotkeys::default(),
            storage: Storage::default(),
            appearance: Appearance::default(),
        }
    }
}

impl Default for General {
    fn default() -> Self {
        General {
            minimize_to_tray: true,
            notifications: true,
            sound_on_save: true,
            performance_mode: false,
            auto_adjust: false,
            check_updates: true,
            auto_update: true,
        }
    }
}

impl Default for Capture {
    fn default() -> Self {
        Capture {
            kind: "monitor".into(),
            monitor: String::new(),
            window_title: String::new(),
            window_exe: String::new(),
            region: [0, 0, 1280, 720],
            method: "auto".into(),
            cursor: true,
        }
    }
}

impl Default for Video {
    fn default() -> Self {
        Video {
            preset: "balanced".into(),
            height: 1080,
            custom: false,
            custom_w: 1920,
            custom_h: 1080,
            fps: 60,
            codec: "h264".into(),
            bitrate_kbps: 16_000,
            rate_control: "vbr".into(),
            effort: "balanced".into(),
            keyframe_seconds: 2,
            encoder: "auto".into(),
            container: "mp4".into(),
        }
    }
}

impl Default for Audio {
    fn default() -> Self {
        Audio {
            system: true,
            system_device: String::new(),
            system_volume: 1.0,
            mic: false,
            mic_device: String::new(),
            mic_volume: 1.0,
            layout: "mixed".into(),
            bitrate_kbps: 160,
        }
    }
}

impl Default for Replay {
    fn default() -> Self {
        Replay { enabled: false, seconds: 60, buffer_dir: String::new() }
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        Hotkeys {
            record: "Alt+F9".into(),
            save_replay: "Alt+F10".into(),
            toggle_replay: "Alt+Shift+F10".into(),
            mute_mic: "Ctrl+Alt+M".into(),
            overlay: String::new(),
        }
    }
}

impl Default for Storage {
    fn default() -> Self {
        Storage { output_dir: default_output_dir().to_string_lossy().into(), folder_per_app: true, low_space_gb: 5 }
    }
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            theme: "system".into(),
            overlay: true,
            overlay_corner: "top_right".into(),
            ui_scale: 1.0,
            motion: "system".into(),
            overlay_size: "medium".into(),
            overlay_opacity: 100,
        }
    }
}

pub const REPLAY_CHOICES: [u32; 6] = [15, 30, 60, 120, 180, 300];
pub const REPLAY_MAX: u32 = 1200;

impl Settings {
    pub fn path() -> PathBuf {
        data_dir().join("settings.json")
    }

    /// Load from disk; never fails.
    pub fn load() -> (Settings, Option<String>) {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> (Settings, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return (Settings::default(), None),
        };
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => {
                let migrated = migrate(value);
                match serde_json::from_value::<Settings>(migrated) {
                    Ok(mut s) => {
                        let fixed = s.sanitize();
                        (
                            s,
                            (!fixed.is_empty()).then(|| {
                                format!("Some settings were out of range and were reset: {}", fixed.join(", "))
                            }),
                        )
                    }
                    Err(e) => (Settings::default(), Some(quarantine(path, &e.to_string()))),
                }
            }
            Err(e) => (Settings::default(), Some(quarantine(path, &e.to_string()))),
        }
    }

    /// Atomically write to disk (temp file + rename).
    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }

    /// Clamp values into supported ranges. Returns the names of fields that were fixed.
    pub fn sanitize(&mut self) -> Vec<&'static str> {
        let mut fixed = Vec::new();
        let d = Settings::default();
        let mut check = |ok: bool, name: &'static str, reset: &mut dyn FnMut()| {
            if !ok {
                reset();
                fixed.push(name);
            }
        };
        let v = &mut self.video;
        check(v.fps == 0 || (1..=240).contains(&v.fps), "frame rate", &mut || v.fps = 60);
        check([0, 2160, 1440, 1080, 900, 720].contains(&v.height), "resolution", &mut || v.height = 1080);
        check((128..=7680).contains(&v.custom_w) && (128..=4320).contains(&v.custom_h), "custom size", &mut || {
            v.custom_w = 1920;
            v.custom_h = 1080;
        });
        check((500..=200_000).contains(&v.bitrate_kbps), "bitrate", &mut || v.bitrate_kbps = 16_000);
        check((1..=10).contains(&v.keyframe_seconds), "keyframe interval", &mut || v.keyframe_seconds = 2);
        check(["h264", "hevc", "av1"].contains(&v.codec.as_str()), "codec", &mut || v.codec = "h264".into());
        check(["vbr", "cbr"].contains(&v.rate_control.as_str()), "rate control", &mut || v.rate_control = "vbr".into());
        check(["speed", "balanced", "quality"].contains(&v.effort.as_str()), "encoder preset", &mut || {
            v.effort = "balanced".into()
        });
        check(v.encoder == "auto" || Backend::from_id(&v.encoder).is_some(), "encoder", &mut || {
            v.encoder = "auto".into()
        });
        check(["mp4", "mkv"].contains(&v.container.as_str()), "container", &mut || v.container = "mp4".into());
        let a = &mut self.audio;
        check((0.0..=2.0).contains(&a.system_volume), "system volume", &mut || a.system_volume = 1.0);
        check((0.0..=2.0).contains(&a.mic_volume), "mic volume", &mut || a.mic_volume = 1.0);
        check([96, 128, 160, 192, 256, 320].contains(&a.bitrate_kbps), "audio bitrate", &mut || a.bitrate_kbps = 160);
        check(["mixed", "separate"].contains(&a.layout.as_str()), "audio tracks", &mut || a.layout = "mixed".into());
        let r = &mut self.replay;
        check((5..=REPLAY_MAX).contains(&r.seconds), "replay length", &mut || r.seconds = 60);
        let ap = &mut self.appearance;
        check(["system", "reduced", "full"].contains(&ap.motion.as_str()), "animations", &mut || {
            ap.motion = "system".into()
        });
        check(["small", "medium", "large"].contains(&ap.overlay_size.as_str()), "overlay size", &mut || {
            ap.overlay_size = "medium".into()
        });
        check((30..=100).contains(&ap.overlay_opacity), "overlay opacity", &mut || ap.overlay_opacity = 100);
        let c = &mut self.capture;
        check(["monitor", "window", "region"].contains(&c.kind.as_str()), "capture source", &mut || {
            c.kind = "monitor".into()
        });
        check(["auto", "wgc", "dda", "gdi"].contains(&c.method.as_str()), "capture method", &mut || {
            c.method = "auto".into()
        });
        let s = &mut self.storage;
        check(!s.output_dir.trim().is_empty(), "output folder", &mut || s.output_dir = d.storage.output_dir.clone());
        check(s.low_space_gb <= 1000, "low space warning", &mut || s.low_space_gb = 5);
        let ap = &mut self.appearance;
        check(["system", "dark", "light"].contains(&ap.theme.as_str()), "theme", &mut || ap.theme = "system".into());
        check((0.75..=2.0).contains(&ap.ui_scale), "UI scale", &mut || ap.ui_scale = 1.0);
        fixed
    }

    // ------------------------------------------------------------- engine conversions

    pub fn codec(&self) -> Codec {
        match self.video.codec.as_str() {
            "hevc" => Codec::Hevc,
            "av1" => Codec::Av1,
            _ => Codec::H264,
        }
    }

    pub fn resolution(&self) -> Resolution {
        if self.video.custom {
            Resolution::Custom { width: self.video.custom_w, height: self.video.custom_h }
        } else if self.video.height == 0 {
            Resolution::Native
        } else {
            Resolution::Height(self.video.height)
        }
    }

    pub fn video_settings(&self) -> VideoSettings {
        VideoSettings {
            resolution: self.resolution(),
            frame_rate: if self.video.fps == 0 { FrameRate::Source } else { FrameRate::Fixed(self.video.fps) },
            codec: self.codec(),
            bitrate_kbps: self.video.bitrate_kbps,
            rate_control: if self.video.rate_control == "cbr" { RateControl::Cbr } else { RateControl::Vbr },
            effort: match self.video.effort.as_str() {
                "speed" => EncoderEffort::Speed,
                "quality" => EncoderEffort::Quality,
                _ => EncoderEffort::Balanced,
            },
            keyframe_seconds: self.video.keyframe_seconds,
        }
    }

    /// Store engine video settings back (used when applying a preset).
    pub fn set_video(&mut self, v: &VideoSettings) {
        match v.resolution {
            Resolution::Native => {
                self.video.custom = false;
                self.video.height = 0;
            }
            Resolution::Height(h) => {
                self.video.custom = false;
                self.video.height = h;
            }
            Resolution::Custom { width, height } => {
                self.video.custom = true;
                self.video.custom_w = width;
                self.video.custom_h = height;
            }
        }
        self.video.fps = match v.frame_rate {
            FrameRate::Fixed(f) => f,
            FrameRate::Source => 0,
        };
        self.video.codec = match v.codec {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
            Codec::Av1 => "av1",
        }
        .into();
        self.video.bitrate_kbps = v.bitrate_kbps;
        self.video.rate_control = if v.rate_control == RateControl::Cbr { "cbr" } else { "vbr" }.into();
        self.video.effort = match v.effort {
            EncoderEffort::Speed => "speed",
            EncoderEffort::Balanced => "balanced",
            EncoderEffort::Quality => "quality",
        }
        .into();
        self.video.keyframe_seconds = v.keyframe_seconds;
    }

    pub fn container(&self) -> Container {
        if self.video.container == "mkv" {
            Container::Mkv
        } else {
            Container::Mp4
        }
    }

    pub fn output_dir(&self) -> PathBuf {
        PathBuf::from(&self.storage.output_dir)
    }

    pub fn replay_dir(&self) -> PathBuf {
        if self.replay.buffer_dir.trim().is_empty() {
            local_dir().join("replay")
        } else {
            PathBuf::from(&self.replay.buffer_dir)
        }
    }

    pub fn audio_settings(&self) -> urec_win::AudioSettings {
        let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
        urec_win::AudioSettings {
            system: self.audio.system,
            system_device: opt(&self.audio.system_device),
            system_volume: self.audio.system_volume,
            mic: self.audio.mic,
            mic_device: opt(&self.audio.mic_device),
            mic_volume: self.audio.mic_volume,
            mic_muted: false,
            layout: if self.audio.layout == "separate" {
                urec_win::TrackLayout::Separate
            } else {
                urec_win::TrackLayout::Mixed
            },
        }
    }
}

/// Upgrade older settings files. Version 0 = files written before versioning existed.
fn migrate(mut v: serde_json::Value) -> serde_json::Value {
    let version = v.get("version").and_then(|x| x.as_u64()).unwrap_or(0);
    if let Some(obj) = v.as_object_mut() {
        if version < 2 {
            // v2 added the setup wizard. People who already used uRec don't need it.
            let used = obj.get("first_run_done").and_then(|x| x.as_bool()).unwrap_or(false);
            obj.entry("setup_done").or_insert(used.into());
        }
        obj.insert("version".into(), SETTINGS_VERSION.into());
    }
    v
}

fn quarantine(path: &Path, why: &str) -> String {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let backup = path.with_file_name(format!("settings.corrupt-{stamp}.json"));
    let _ = std::fs::rename(path, &backup);
    format!(
        "Your settings file couldn't be read ({why}). It was saved as {} and defaults were restored.",
        backup.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("urec-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("settings.json")
    }

    #[test]
    fn roundtrip_and_defaults() {
        let p = tmp("rt");
        let mut s = Settings::default();
        s.video.fps = 144;
        s.replay.enabled = true;
        s.save_to(&p).unwrap();
        let (l, msg) = Settings::load_from(&p);
        assert_eq!(l, s);
        assert!(msg.is_none());
        // Instant Replay is off by default
        assert!(!Settings::default().replay.enabled);
    }

    #[test]
    fn corrupt_file_is_quarantined() {
        let p = tmp("bad");
        std::fs::write(&p, "{ not json").unwrap();
        let (s, msg) = Settings::load_from(&p);
        assert_eq!(s, Settings::default());
        assert!(msg.unwrap().contains("couldn't be read"));
        assert!(!p.exists());
    }

    #[test]
    fn partial_and_out_of_range_values() {
        let p = tmp("partial");
        std::fs::write(&p, r#"{"video":{"fps":999,"codec":"vp9"},"unknown_field":1}"#).unwrap();
        let (s, msg) = Settings::load_from(&p);
        assert_eq!(s.video.fps, 60);
        assert_eq!(s.video.codec, "h264");
        assert_eq!(s.audio, Audio::default());
        assert!(msg.unwrap().contains("frame rate"));
        assert_eq!(s.version, SETTINGS_VERSION);
    }

    #[test]
    fn video_conversion_roundtrip() {
        let mut s = Settings::default();
        let mut v = s.video_settings();
        v.resolution = Resolution::Custom { width: 1600, height: 900 };
        v.frame_rate = FrameRate::Source;
        v.codec = Codec::Av1;
        s.set_video(&v);
        assert_eq!(s.video_settings(), v);
    }

    #[test]
    fn migrates_v1_to_v2_without_nagging_existing_users() {
        let p = tmp("v1");
        std::fs::write(&p, r#"{"version":1,"first_run_done":true,"appearance":{"overlay":false}}"#).unwrap();
        let (s, _) = Settings::load_from(&p);
        assert!(s.setup_done, "existing users skip the new setup");
        assert!(!s.appearance.overlay, "their choices are kept");
        assert_eq!(s.appearance.motion, "system", "new fields get defaults");
        assert_eq!(s.version, SETTINGS_VERSION);
        let p2 = tmp("v1new");
        std::fs::write(&p2, r#"{"version":1,"first_run_done":false}"#).unwrap();
        assert!(!Settings::load_from(&p2).0.setup_done);
        let p3 = tmp("v2");
        std::fs::write(
            &p3,
            r#"{"version":2,"first_run_done":true,"setup_done":false,"appearance":{"motion":"wobbly"}}"#,
        )
        .unwrap();
        let (s3, _) = Settings::load_from(&p3);
        assert!(!s3.setup_done, "v2 files are taken as they are");
        assert_eq!(s3.appearance.motion, "system", "unknown values are reset");
        assert!(!Settings::default().setup_done);
    }
}
