//! Video settings: resolution, frame rate, codec, rate control, presets and
//! storage estimates. Pure logic, no I/O.

use std::fmt;

/// Output resolution requested by the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Same as the capture source; no scaling.
    Native,
    /// Fit within this height, keeping the source aspect ratio (2160, 1440, 1080, 900, 720).
    Height(u32),
    /// Exact output size. Aspect ratio is preserved with black bars if it differs.
    Custom { width: u32, height: u32 },
}

impl Resolution {
    pub const STANDARD_HEIGHTS: [u32; 5] = [2160, 1440, 1080, 900, 720];

    pub fn label(&self) -> String {
        match *self {
            Resolution::Native => "Native".into(),
            Resolution::Height(2160) => "3840 × 2160 (4K)".into(),
            Resolution::Height(1440) => "2560 × 1440".into(),
            Resolution::Height(1080) => "1920 × 1080".into(),
            Resolution::Height(900) => "1600 × 900".into(),
            Resolution::Height(720) => "1280 × 720".into(),
            Resolution::Height(h) => format!("{h}p"),
            Resolution::Custom { width, height } => format!("{width} × {height}"),
        }
    }
}

/// Frame rate in frames per second. `Source` means "match the display refresh rate".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameRate {
    Fixed(u32),
    Source,
}

impl FrameRate {
    pub const CHOICES: [u32; 9] = [24, 30, 48, 60, 90, 120, 144, 165, 240];
    pub const MIN: u32 = 1;
    pub const MAX: u32 = 240;

    /// Resolve against the display refresh rate (falls back to 60 if unknown).
    pub fn resolve(&self, source_refresh: Option<u32>) -> u32 {
        match *self {
            FrameRate::Fixed(f) => f.clamp(Self::MIN, Self::MAX),
            FrameRate::Source => source_refresh.unwrap_or(60).clamp(Self::MIN, Self::MAX),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    pub const ALL: [Codec; 3] = [Codec::H264, Codec::Hevc, Codec::Av1];

    pub fn label(&self) -> &'static str {
        match self {
            Codec::H264 => "H.264 (AVC)",
            Codec::Hevc => "H.265 (HEVC)",
            Codec::Av1 => "AV1",
        }
    }

    /// Bitrate multiplier relative to H.264 for similar visual quality.
    pub fn efficiency(&self) -> f64 {
        match self {
            Codec::H264 => 1.0,
            Codec::Hevc => 0.7,
            Codec::Av1 => 0.6,
        }
    }

    /// Largest dimension that common hardware and level limits accept.
    pub fn max_dimension(&self) -> u32 {
        match self {
            Codec::H264 => 4096,
            Codec::Hevc | Codec::Av1 => 8192,
        }
    }
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateControl {
    Cbr,
    Vbr,
}

/// Speed/quality trade-off passed to the encoder's own preset system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncoderEffort {
    Speed,
    Balanced,
    Quality,
}

/// The one-click presets shown in the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    LowEnd,
    Balanced,
    High,
    VeryHigh,
    Custom,
}

impl Preset {
    pub const ALL: [Preset; 5] = [Preset::LowEnd, Preset::Balanced, Preset::High, Preset::VeryHigh, Preset::Custom];

    pub fn label(&self) -> &'static str {
        match self {
            Preset::LowEnd => "Low resource usage",
            Preset::Balanced => "Balanced",
            Preset::High => "High quality",
            Preset::VeryHigh => "Very high quality",
            Preset::Custom => "Custom",
        }
    }
}

/// Everything that determines the encoded video stream.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoSettings {
    pub resolution: Resolution,
    pub frame_rate: FrameRate,
    pub codec: Codec,
    /// Target bitrate in kbit/s.
    pub bitrate_kbps: u32,
    pub rate_control: RateControl,
    pub effort: EncoderEffort,
    /// Seconds between keyframes.
    pub keyframe_seconds: u32,
}

impl Default for VideoSettings {
    fn default() -> Self {
        apply_preset(Preset::Balanced, &Capabilities::unknown())
    }
}

/// What the machine can do, as far as preset selection needs to know.
#[derive(Clone, Debug, PartialEq)]
pub struct Capabilities {
    pub hardware_encoder: bool,
    pub hevc: bool,
    pub av1: bool,
    pub source_width: u32,
    pub source_height: u32,
    pub source_refresh: Option<u32>,
    pub logical_cpus: u32,
}

impl Capabilities {
    pub fn unknown() -> Self {
        Capabilities {
            hardware_encoder: false,
            hevc: false,
            av1: false,
            source_width: 1920,
            source_height: 1080,
            source_refresh: Some(60),
            logical_cpus: 4,
        }
    }
}

/// Settings for a preset, adjusted to the machine. Never exceeds the source resolution.
pub fn apply_preset(preset: Preset, caps: &Capabilities) -> VideoSettings {
    let hw = caps.hardware_encoder;
    let (height, fps, effort) = match preset {
        Preset::LowEnd => (720, 30, EncoderEffort::Speed),
        Preset::Balanced => (1080, if hw { 60 } else { 30 }, EncoderEffort::Balanced),
        Preset::High => (1080, 60, EncoderEffort::Balanced),
        Preset::VeryHigh => (if caps.source_height >= 2160 { 2160 } else { 1440 }, 60, EncoderEffort::Quality),
        Preset::Custom => (1080, 60, EncoderEffort::Balanced),
    };
    // A weak CPU without a hardware encoder can't keep up with 60 fps x264.
    let fps = if !hw && caps.logical_cpus <= 4 { fps.min(30) } else { fps };
    let resolution = if height >= caps.source_height { Resolution::Native } else { Resolution::Height(height) };
    let (w, h) = output_size(resolution, caps.source_width, caps.source_height);
    let codec = Codec::H264;
    let quality_factor = match preset {
        Preset::LowEnd => 0.55,
        Preset::Balanced => 0.8,
        Preset::High | Preset::Custom => 1.0,
        Preset::VeryHigh => 1.5,
    };
    VideoSettings {
        resolution,
        frame_rate: FrameRate::Fixed(fps),
        codec,
        bitrate_kbps: recommended_bitrate_kbps(w, h, fps, codec, quality_factor),
        rate_control: RateControl::Vbr,
        effort,
        keyframe_seconds: 2,
    }
}

/// Bitrate that gives good gameplay quality: 1080p60 H.264 at factor 1.0 is about 20 Mbps.
pub fn recommended_bitrate_kbps(w: u32, h: u32, fps: u32, codec: Codec, quality_factor: f64) -> u32 {
    let pixels_per_sec = w as f64 * h as f64 * fps as f64;
    let reference = 1920.0 * 1080.0 * 60.0;
    let kbps = 20_000.0 * (pixels_per_sec / reference).powf(0.75) * codec.efficiency() * quality_factor;
    // Round to 500 kbps steps, keep within a sane range.
    ((kbps / 500.0).round() * 500.0).clamp(1_000.0, 150_000.0) as u32
}

/// Output frame size for a resolution choice. Always even dimensions; never upscales
/// for `Height` (a 1080 choice on a 900p screen records at 900p).
pub fn output_size(res: Resolution, src_w: u32, src_h: u32) -> (u32, u32) {
    let even = |v: u32| (v.max(2) / 2) * 2;
    match res {
        Resolution::Native => (even(src_w), even(src_h)),
        Resolution::Height(target) => {
            if target >= src_h || src_h == 0 {
                (even(src_w), even(src_h))
            } else {
                let w = (src_w as f64 * target as f64 / src_h as f64).round() as u32;
                (even(w), even(target))
            }
        }
        Resolution::Custom { width, height } => (even(width), even(height)),
    }
}

/// Reasons a configuration can't be recorded as-is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsError {
    TooSmall { width: u32, height: u32 },
    TooLarge { width: u32, height: u32, codec: Codec, max: u32 },
    BadFrameRate(u32),
    BadBitrate(u32),
    BadKeyframeInterval(u32),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::TooSmall { width, height } => {
                write!(f, "{width}×{height} is too small. Use at least 128×128.")
            }
            SettingsError::TooLarge { width, height, codec, max } => write!(
                f,
                "{width}×{height} is larger than {codec} supports ({max} pixels per side). Choose a smaller size or another codec."
            ),
            SettingsError::BadFrameRate(v) => write!(f, "{v} FPS isn't supported. Choose 1–240 FPS."),
            SettingsError::BadBitrate(v) => write!(f, "{v} kbps is outside 500–200000 kbps."),
            SettingsError::BadKeyframeInterval(v) => write!(f, "Keyframe interval {v}s must be 1–10 seconds."),
        }
    }
}

impl std::error::Error for SettingsError {}

/// Validate settings against a source size. Returns the output size on success.
pub fn validate(
    v: &VideoSettings,
    src_w: u32,
    src_h: u32,
    src_refresh: Option<u32>,
) -> Result<(u32, u32, u32), SettingsError> {
    let (w, h) = output_size(v.resolution, src_w, src_h);
    if w < 128 || h < 128 {
        return Err(SettingsError::TooSmall { width: w, height: h });
    }
    let max = v.codec.max_dimension();
    if w > max || h > max {
        return Err(SettingsError::TooLarge { width: w, height: h, codec: v.codec, max });
    }
    let fps = match v.frame_rate {
        FrameRate::Fixed(f) if !(FrameRate::MIN..=FrameRate::MAX).contains(&f) => {
            return Err(SettingsError::BadFrameRate(f))
        }
        other => other.resolve(src_refresh),
    };
    if !(500..=200_000).contains(&v.bitrate_kbps) {
        return Err(SettingsError::BadBitrate(v.bitrate_kbps));
    }
    if !(1..=10).contains(&v.keyframe_seconds) {
        return Err(SettingsError::BadKeyframeInterval(v.keyframe_seconds));
    }
    Ok((w, h, fps))
}

/// Approximate bytes written per second of recording (video + audio + container overhead).
pub fn bytes_per_second(video_kbps: u32, audio_kbps: u32) -> u64 {
    ((video_kbps as u64 + audio_kbps as u64) * 1000 / 8) * 102 / 100
}

/// Human-readable size, e.g. "1.4 GB".
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_size_never_upscales() {
        assert_eq!(output_size(Resolution::Height(1080), 1600, 900), (1600, 900));
        assert_eq!(output_size(Resolution::Height(720), 2560, 1440), (1280, 720));
        assert_eq!(output_size(Resolution::Native, 1366, 768), (1366, 768));
        // odd sizes become even
        assert_eq!(output_size(Resolution::Native, 1365, 767), (1364, 766));
        // ultrawide keeps aspect
        assert_eq!(output_size(Resolution::Height(1080), 3440, 1440), (2580, 1080));
    }

    #[test]
    fn validate_catches_limits() {
        let mut v =
            VideoSettings { resolution: Resolution::Custom { width: 5120, height: 2880 }, ..VideoSettings::default() };
        assert!(matches!(validate(&v, 1920, 1080, None), Err(SettingsError::TooLarge { .. })));
        v.codec = Codec::Hevc;
        assert!(validate(&v, 1920, 1080, None).is_ok());
        v.frame_rate = FrameRate::Fixed(500);
        assert_eq!(validate(&v, 1920, 1080, None), Err(SettingsError::BadFrameRate(500)));
        v.frame_rate = FrameRate::Source;
        assert_eq!(validate(&v, 1920, 1080, Some(144)).unwrap().2, 144);
    }

    #[test]
    fn presets_respect_machine() {
        let mut caps = Capabilities::unknown();
        caps.logical_cpus = 4;
        let low = apply_preset(Preset::LowEnd, &caps);
        assert_eq!(output_size(low.resolution, 1920, 1080), (1280, 720));
        assert_eq!(low.frame_rate, FrameRate::Fixed(30));
        // no hardware encoder on a 4-thread CPU: no 60 fps
        assert_eq!(apply_preset(Preset::High, &caps).frame_rate, FrameRate::Fixed(30));
        caps.hardware_encoder = true;
        assert_eq!(apply_preset(Preset::High, &caps).frame_rate, FrameRate::Fixed(60));
        // 1440 preset on a 1080p screen records native, not upscaled
        assert_eq!(apply_preset(Preset::VeryHigh, &caps).resolution, Resolution::Native);
    }

    #[test]
    fn bitrate_scales() {
        let b1080 = recommended_bitrate_kbps(1920, 1080, 60, Codec::H264, 1.0);
        assert_eq!(b1080, 20_000);
        assert!(recommended_bitrate_kbps(1280, 720, 30, Codec::H264, 1.0) < b1080 / 2);
        assert!(recommended_bitrate_kbps(1920, 1080, 60, Codec::Hevc, 1.0) < b1080);
    }

    #[test]
    fn bytes_formatting() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_500_000), "1.5 MB");
        assert_eq!(format_bytes(250_000_000_000), "250 GB");
        // 20 Mbps video + 160 kbps audio for a minute ~ 154 MB
        let per_min = bytes_per_second(20_000, 160) * 60;
        assert!((150_000_000..160_000_000).contains(&per_min), "{per_min}");
    }
}
