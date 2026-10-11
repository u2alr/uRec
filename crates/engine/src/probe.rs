//! Runtime capability detection: which encoders actually work on this PC.

use crate::encoder::{explain_probe_failure, is_argument_error, Encoder};
use crate::process::run_capture;
use crate::video::{Codec, VideoSettings};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct ProbeResult {
    pub encoder: Encoder,
    pub ok: bool,
    /// User-facing reason when not ok.
    pub reason: String,
    /// The failure came from our arguments (a bug), not from missing hardware.
    pub argument_error: bool,
    pub millis: u128,
    pub raw_error: String,
}

/// Test-encode a few frames with every candidate encoder for the given codecs.
/// Encoders missing from this FFmpeg build are reported without running them.
pub fn probe_encoders(ffmpeg: &Path, codecs: &[Codec], v: &VideoSettings) -> Vec<ProbeResult> {
    let built_in = available_encoders(ffmpeg);
    let mut out = Vec::new();
    for &codec in codecs {
        let mut settings = v.clone();
        settings.codec = codec;
        for enc in Encoder::candidates(codec) {
            let name = enc.ffmpeg_name().unwrap_or_default();
            if !built_in.is_empty() && !built_in.iter().any(|n| n == name) {
                out.push(ProbeResult {
                    encoder: enc,
                    ok: false,
                    reason: "This encoder isn't included in the bundled FFmpeg.".into(),
                    argument_error: false,
                    millis: 0,
                    raw_error: String::new(),
                });
                continue;
            }
            out.push(probe_one(ffmpeg, enc, &settings, 640, 360, 30));
        }
    }
    out
}

/// Names of the video encoders compiled into this FFmpeg.
pub fn available_encoders(ffmpeg: &Path) -> Vec<String> {
    let Ok(out) = crate::process::quiet_command(ffmpeg).args(["-hide_banner", "-encoders"]).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let flags = parts.next()?;
            let name = parts.next()?;
            (flags.len() == 6 && flags.starts_with('V')).then(|| name.to_string())
        })
        .collect()
}

pub fn probe_one(ffmpeg: &Path, enc: Encoder, v: &VideoSettings, w: u32, h: u32, fps: u32) -> ProbeResult {
    let start = Instant::now();
    let args = enc.probe_args(v, w, h, fps);
    let (ok, err) = match run_capture(ffmpeg, &args, Duration::from_secs(20)) {
        Ok(r) => r,
        Err(e) => (false, e.to_string()),
    };
    ProbeResult {
        encoder: enc,
        ok,
        reason: if ok { String::new() } else { explain_probe_failure(&enc, &err) },
        argument_error: !ok && is_argument_error(&err),
        millis: start.elapsed().as_millis(),
        raw_error: err.trim().to_string(),
    }
}

/// First working encoder for a codec, in preference order (hardware first).
pub fn best(results: &[ProbeResult], codec: Codec) -> Option<Encoder> {
    results.iter().find(|r| r.ok && r.encoder.codec == codec).map(|r| r.encoder)
}

/// `ffmpeg -version` first line.
pub fn ffmpeg_version(ffmpeg: &Path) -> Option<String> {
    let out = crate::process::quiet_command(ffmpeg).arg("-version").output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next().map(|s| s.trim().to_string())
}

/// Number of logical CPUs (for preset recommendations).
pub fn logical_cpus() -> u32 {
    std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4)
}
