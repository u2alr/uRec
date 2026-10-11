//! Encoder backends, their FFmpeg names and arguments, and runtime probing.
//!
//! Availability is never inferred from the GPU brand: every candidate is test-encoded
//! with the exact arguments a recording would use, and only encoders that succeed are offered.

use crate::video::{Codec, EncoderEffort, RateControl, VideoSettings};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Backend {
    /// NVIDIA NVENC.
    Nvenc,
    /// AMD Advanced Media Framework.
    Amf,
    /// Intel Quick Sync Video.
    Qsv,
    /// Windows Media Foundation using the GPU vendor's hardware encoder (MFT).
    MediaFoundationHw,
    /// x264 / x265 on the CPU.
    Software,
    /// Windows Media Foundation software encoder (CPU).
    MediaFoundation,
}

impl Backend {
    /// Preference order when choosing automatically.
    pub const PREFERENCE: [Backend; 6] = [
        Backend::Nvenc,
        Backend::Amf,
        Backend::Qsv,
        Backend::MediaFoundationHw,
        Backend::Software,
        Backend::MediaFoundation,
    ];

    pub fn is_hardware(&self) -> bool {
        matches!(self, Backend::Nvenc | Backend::Amf | Backend::Qsv | Backend::MediaFoundationHw)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Backend::Nvenc => "NVIDIA NVENC",
            Backend::Amf => "AMD AMF",
            Backend::Qsv => "Intel Quick Sync",
            Backend::MediaFoundationHw => "Windows hardware encoder",
            Backend::Software => "CPU (x264/x265)",
            Backend::MediaFoundation => "CPU (Windows encoder)",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Backend::Nvenc => "nvenc",
            Backend::Amf => "amf",
            Backend::Qsv => "qsv",
            Backend::MediaFoundationHw => "mf_hw",
            Backend::Software => "software",
            Backend::MediaFoundation => "mf",
        }
    }

    pub fn from_id(s: &str) -> Option<Backend> {
        Backend::PREFERENCE.into_iter().find(|b| b.id() == s)
    }
}

/// A concrete encoder: backend + codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Encoder {
    pub backend: Backend,
    pub codec: Codec,
}

impl fmt::Display for Encoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.backend.label(), self.codec)
    }
}

impl Encoder {
    pub fn new(backend: Backend, codec: Codec) -> Self {
        Encoder { backend, codec }
    }

    /// FFmpeg encoder name, or None if this backend has no encoder for the codec.
    pub fn ffmpeg_name(&self) -> Option<&'static str> {
        use Backend::*;
        use Codec::*;
        Some(match (self.backend, self.codec) {
            (Nvenc, H264) => "h264_nvenc",
            (Nvenc, Hevc) => "hevc_nvenc",
            (Nvenc, Av1) => "av1_nvenc",
            (Amf, H264) => "h264_amf",
            (Amf, Hevc) => "hevc_amf",
            (Amf, Av1) => "av1_amf",
            (Qsv, H264) => "h264_qsv",
            (Qsv, Hevc) => "hevc_qsv",
            (Qsv, Av1) => "av1_qsv",
            (MediaFoundationHw | MediaFoundation, H264) => "h264_mf",
            (MediaFoundationHw | MediaFoundation, Hevc) => "hevc_mf",
            (MediaFoundationHw, Av1) => "av1_mf",
            (Software, H264) => "libx264",
            (Software, Hevc) => "libx265",
            // Software AV1 (libaom) is far too slow for real-time recording.
            (Software | MediaFoundation, Av1) => return None,
        })
    }

    /// All encoders worth probing, in preference order.
    pub fn candidates(codec: Codec) -> Vec<Encoder> {
        Backend::PREFERENCE.into_iter().map(|b| Encoder::new(b, codec)).filter(|e| e.ffmpeg_name().is_some()).collect()
    }

    /// Can take Direct3D 11 GPU frames directly (zero-copy from screen capture).
    pub fn accepts_d3d11(&self) -> bool {
        matches!(self.backend, Backend::Nvenc | Backend::Amf | Backend::MediaFoundationHw)
    }

    /// Quick Sync needs frames mapped into a QSV surface.
    pub fn needs_qsv_frames(&self) -> bool {
        self.backend == Backend::Qsv
    }

    /// Pixel format for system-memory input.
    pub fn sw_pix_fmt(&self) -> &'static str {
        match self.backend {
            Backend::Software => "yuv420p",
            _ => "nv12",
        }
    }

    /// Encoder arguments (everything after `-c:v`'s position, including `-c:v`).
    pub fn args(&self, v: &VideoSettings, fps: u32) -> Vec<String> {
        let name = self.ffmpeg_name().expect("encoder without ffmpeg name");
        let k = v.bitrate_kbps;
        let cbr = v.rate_control == RateControl::Cbr;
        let cq_mode = v.rate_control == RateControl::Quality;
        // Quantizer for constant quality, adjusted per codec (HEVC/AV1 scales sit higher).
        let cq = crate::video::quality_cq(v.quality)
            + match self.codec {
                Codec::H264 => 0,
                Codec::Hevc => 3,
                Codec::Av1 => 8,
            };
        let cq_s = cq.to_string();
        // In constant-quality mode the bitrate setting is a ceiling, not a target.
        let maxrate = if cbr || cq_mode { k } else { k + k / 2 };
        let gop = (fps * v.keyframe_seconds).max(1);
        let mut a: Vec<String> = vec!["-c:v".into(), name.into()];
        let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));

        match self.backend {
            Backend::Nvenc => {
                let preset = match v.effort {
                    EncoderEffort::Speed => "p2",
                    EncoderEffort::Balanced => "p4",
                    EncoderEffort::Quality => "p6",
                };
                push(&["-preset", preset, "-tune", "hq", "-rc", if cbr { "cbr" } else { "vbr" }]);
                if cq_mode {
                    push(&["-cq", &cq_s]);
                }
                push(&["-forced-idr", "1"]);
            }
            Backend::Amf => {
                let quality = match v.effort {
                    EncoderEffort::Speed => "speed",
                    EncoderEffort::Balanced => "balanced",
                    EncoderEffort::Quality => "quality",
                };
                if cq_mode {
                    // Constant QP: supported by every AMF generation (QVBR isn't).
                    let p = (cq + 2).to_string();
                    push(&["-usage", "transcoding", "-quality", quality, "-rc", "cqp"]);
                    push(&["-qp_i", &cq_s, "-qp_p", &p]);
                    if self.codec == Codec::H264 {
                        push(&["-qp_b", &p]);
                    }
                } else {
                    push(&["-usage", "transcoding", "-quality", quality, "-rc", if cbr { "cbr" } else { "vbr_peak" }]);
                }
                if self.codec == Codec::H264 {
                    push(&["-forced_idr", "1"]);
                }
            }
            Backend::Qsv => {
                let preset = match v.effort {
                    EncoderEffort::Speed => "veryfast",
                    EncoderEffort::Balanced => "medium",
                    EncoderEffort::Quality => "slow",
                };
                push(&["-preset", preset, "-look_ahead", "0", "-forced_idr", "1"]);
                if cq_mode {
                    // Intelligent constant quality (ICQ).
                    push(&["-global_quality", &cq_s]);
                }
            }
            Backend::MediaFoundationHw | Backend::MediaFoundation => {
                if cq_mode {
                    // Media Foundation's quality scale is 0-100, higher is better.
                    let q = (100i32 - (cq as i32 - 10) * 3).clamp(1, 100).to_string();
                    push(&["-rate_control", "quality", "-quality", &q]);
                } else {
                    push(&["-rate_control", if cbr { "cbr" } else { "pc_vbr" }]);
                }
                if self.backend == Backend::MediaFoundationHw {
                    push(&["-hw_encoding", "1"]);
                }
            }
            Backend::Software => {
                let preset = match v.effort {
                    EncoderEffort::Speed => "ultrafast",
                    EncoderEffort::Balanced => "veryfast",
                    EncoderEffort::Quality => "faster",
                };
                push(&["-preset", preset]);
                if cq_mode {
                    push(&["-crf", &cq_s]);
                }
                match self.codec {
                    Codec::H264 if cbr => push(&["-x264-params", "nal-hrd=cbr"]),
                    Codec::Hevc => push(&["-x265-params", "log-level=error"]),
                    _ => {}
                }
            }
        }
        let k_s = format!("{k}k");
        let max_s = format!("{maxrate}k");
        let buf_s = format!("{}k", maxrate * 2);
        let gop_s = gop.to_string();
        let kf = format!("expr:gte(t,n_forced*{})", v.keyframe_seconds);
        match (cq_mode, self.backend) {
            // Constant quality with a ceiling: no average target, only the peak is capped.
            (true, Backend::Nvenc) => push(&["-b:v", "0", "-maxrate", &max_s, "-bufsize", &buf_s]),
            (true, Backend::Software) => push(&["-maxrate", &max_s, "-bufsize", &buf_s]),
            // AMF CQP / QSV ICQ / MF quality ignore bitrates.
            (true, _) => {}
            (false, _) => push(&["-b:v", &k_s, "-maxrate", &max_s, "-bufsize", &buf_s]),
        }
        push(&["-g", &gop_s, "-force_key_frames", &kf]);
        a
    }

    /// Arguments for a short synthetic test encode (no capture involved).
    pub fn probe_args(&self, v: &VideoSettings, width: u32, height: u32, fps: u32) -> Vec<String> {
        let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin", "-f", "lavfi", "-i"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        a.push(format!("color=c=gray:s={width}x{height}:r={fps}"));
        a.extend(["-frames:v", "8", "-pix_fmt", self.sw_pix_fmt()].iter().map(|s| s.to_string()));
        a.extend(self.args(v, fps));
        a.extend(["-f", "null", "-"].iter().map(|s| s.to_string()));
        a
    }
}

/// Why an encoder failed its probe, in words a user can act on.
pub fn explain_probe_failure(enc: &Encoder, stderr: &str) -> String {
    let s = stderr.to_ascii_lowercase();
    let vendor_missing = s.contains("cannot load nvcuda")
        || s.contains("cannot load libcuda")
        || s.contains("amfrt64.dll")
        || s.contains("error creating a mfx session")
        || s.contains("no device available")
        || s.contains("failed to open");
    if vendor_missing {
        return format!("No {} hardware or driver found.", enc.backend.label());
    }
    if s.contains("driver does not support")
        || s.contains("minimum required nvidia driver")
        || s.contains("driver version")
    {
        return "The graphics driver is too old for this encoder. Updating it may enable it.".into();
    }
    if s.contains("no capable devices") || s.contains("not supported") || s.contains("unsupported") {
        return format!("Your GPU can't encode {} with this encoder.", enc.codec);
    }
    if s.contains("out of memory") || s.contains("incompatible client key") || s.contains("openencodesessionex failed")
    {
        return "The encoder is busy or out of sessions (another app may be using it).".into();
    }
    if s.contains("could not find any mft") {
        return format!(
            "Windows has no {} encoder for {} on this PC.",
            if enc.backend.is_hardware() { "hardware" } else { "built-in" },
            enc.codec
        );
    }
    if s.contains("unknown encoder") {
        return "This encoder isn't included in the bundled FFmpeg.".into();
    }
    let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("Unknown error").trim();
    format!("Failed to start: {first}")
}

/// True when the failure is caused by our arguments rather than missing hardware.
/// Used by CI to make sure every argument set we generate is accepted by the bundled FFmpeg.
pub fn is_argument_error(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("unrecognized option")
        || s.contains("option not found")
        || s.contains("error setting option")
        || s.contains("not an encoding option")
        || s.contains("invalid argument") && s.contains("option")
        || s.contains("unable to parse option value")
        || s.contains("undefined constant or missing")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::VideoSettings;

    #[test]
    fn names_and_candidates() {
        assert_eq!(Encoder::new(Backend::Amf, Codec::H264).ffmpeg_name(), Some("h264_amf"));
        assert_eq!(Encoder::new(Backend::Software, Codec::Av1).ffmpeg_name(), None);
        let av1: Vec<_> = Encoder::candidates(Codec::Av1).iter().map(|e| e.backend).collect();
        assert_eq!(av1, vec![Backend::Nvenc, Backend::Amf, Backend::Qsv, Backend::MediaFoundationHw]);
        assert_eq!(Encoder::candidates(Codec::H264)[0].backend, Backend::Nvenc);
        assert_eq!(Backend::from_id("amf"), Some(Backend::Amf));
    }

    #[test]
    fn args_have_rate_and_keyframes() {
        let v = VideoSettings { bitrate_kbps: 10_000, keyframe_seconds: 2, ..VideoSettings::default() };
        let a = Encoder::new(Backend::Software, Codec::H264).args(&v, 60);
        let j = a.join(" ");
        assert!(j.starts_with("-c:v libx264"));
        assert!(j.contains("-b:v 10000k"));
        assert!(j.contains("-g 120"));
        assert!(j.contains("n_forced*2"));
        let n = Encoder::new(Backend::Nvenc, Codec::H264).args(&v, 60).join(" ");
        assert!(n.contains("-forced-idr 1"));
    }

    #[test]
    fn constant_quality_args() {
        let v = VideoSettings {
            bitrate_kbps: 20_000,
            rate_control: RateControl::Quality,
            quality: 7,
            ..VideoSettings::default()
        };
        let j = |b, c| Encoder::new(b, c).args(&v, 60).join(" ");
        // x264: CRF with only a ceiling, no average target.
        let x = j(Backend::Software, Codec::H264);
        assert!(x.contains("-crf 23") && x.contains("-maxrate 20000k") && !x.contains("-b:v 20000k"), "{x}");
        // x265 sits 3 higher for the same picture.
        assert!(j(Backend::Software, Codec::Hevc).contains("-crf 26"));
        let n = j(Backend::Nvenc, Codec::H264);
        assert!(n.contains("-rc vbr") && n.contains("-cq 23") && n.contains("-b:v 0"), "{n}");
        let a = j(Backend::Amf, Codec::Hevc);
        assert!(a.contains("-rc cqp -qp_i 26 -qp_p 28") && !a.contains("-qp_b") && !a.contains("-b:v"), "{a}");
        assert!(j(Backend::Amf, Codec::H264).contains("-qp_b 25"));
        let q = j(Backend::Qsv, Codec::H264);
        assert!(q.contains("-global_quality 23") && !q.contains("-b:v"), "{q}");
        let m = j(Backend::MediaFoundation, Codec::H264);
        assert!(m.contains("-rate_control quality -quality 61"), "{m}");
        // Keyframes stay regular in every mode (Instant Replay segments depend on it).
        for b in Backend::PREFERENCE {
            assert!(j(b, Codec::H264).contains("-g 120"));
        }
        assert_eq!(crate::video::quality_cq(1), 32);
        assert_eq!(crate::video::quality_cq(10), 18);
        assert!(crate::video::quality_cq(0) == 32 && crate::video::quality_cq(99) == 18);
    }

    #[test]
    fn failure_messages() {
        let e = Encoder::new(Backend::Nvenc, Codec::H264);
        assert!(explain_probe_failure(&e, "[h264_nvenc @ 0x1] Cannot load nvcuda.dll").contains("No NVIDIA"));
        assert!(is_argument_error("[h264_amf] Unable to parse option value \"x\""));
        assert!(!is_argument_error("DLL amfrt64.dll failed to open"));
    }
}
