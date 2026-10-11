//! Check a finished recording the way a player would: the MP4 structure, then a full decode
//! with FFmpeg (every frame, every audio stream), reporting exactly what was found. Used by the
//! first-run test recording, so its results are real, never assumed.

use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecodeReport {
    pub video_codec: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub audio_streams: usize,
    /// Video frames decoded (from FFmpeg's final progress line).
    pub frames: u64,
    pub duration: f64,
    /// Decoder errors, if any (first few lines).
    pub errors: Vec<String>,
}

/// Parse the stderr of `ffmpeg -i FILE -map 0 -f null -`.
pub fn parse_decode(stderr: &str) -> DecodeReport {
    let mut r = DecodeReport::default();
    let mut in_input = false;
    // Progress lines are separated by \r.
    for line in stderr.split(['\n', '\r']) {
        let l = line.trim();
        if l.starts_with("Input #") {
            in_input = true;
        } else if l.starts_with("Output #") || l.starts_with("Stream mapping") {
            in_input = false;
        }
        if in_input {
            if let Some(rest) = l.strip_prefix("Duration: ") {
                let d = rest.split(',').next().unwrap_or("");
                let p: Vec<f64> = d.split(':').filter_map(|x| x.trim().parse().ok()).collect();
                if p.len() == 3 {
                    r.duration = p[0] * 3600.0 + p[1] * 60.0 + p[2];
                }
            }
            if l.starts_with("Stream #") && l.contains("Video:") && r.video_codec.is_none() {
                let after = l.split("Video:").nth(1).unwrap_or("").trim();
                r.video_codec = after.split([' ', ',']).next().map(str::to_string);
                for tok in after.split([',', ' ']) {
                    if let Some((w, h)) = tok.split_once('x') {
                        if let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>()) {
                            if w >= 16 && h >= 16 && r.width == 0 {
                                r.width = w;
                                r.height = h;
                            }
                        }
                    }
                }
                for s in after.split(',').map(str::trim) {
                    if let Some(v) = s.strip_suffix(" fps") {
                        r.fps = v.trim().parse().unwrap_or(0.0);
                    }
                }
            }
            if l.starts_with("Stream #") && l.contains("Audio:") {
                r.audio_streams += 1;
            }
        }
        if let Some(rest) = l.strip_prefix("frame=") {
            if let Some(n) = rest.split_whitespace().next().and_then(|n| n.parse().ok()) {
                r.frames = n;
            }
        }
        let lower = l.to_ascii_lowercase();
        let decoder_error = l.starts_with('[')
            && (lower.contains("error")
                || lower.contains("corrupt")
                || lower.contains("invalid")
                || lower.contains("missing"));
        if decoder_error && r.errors.len() < 5 {
            r.errors.push(l.to_string());
        }
    }
    r
}

/// Decode the whole file. `Err` only if FFmpeg couldn't run or couldn't open the file.
pub fn decode_file(ffmpeg: &Path, file: &Path) -> Result<DecodeReport, String> {
    let args: Vec<String> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-stats".into(),
        "-i".into(),
        file.to_string_lossy().into(),
        "-map".into(),
        "0".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ];
    let (ok, err) = crate::process::run_capture(ffmpeg, &args, Duration::from_secs(120)).map_err(|e| e.to_string())?;
    let report = parse_decode(&err);
    if !ok && report.video_codec.is_none() {
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        return Err(format!("FFmpeg couldn't read the file: {last}"));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'C:\\\\t\\\\test.mp4':\n  Metadata:\n    major_brand     : isom\n  Duration: 00:00:05.03, start: 0.000000, bitrate: 2210 kb/s\n  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), 1280x720 [SAR 1:1 DAR 16:9], 2045 kb/s, 30 fps, 30 tbr, 15360 tbn (default)\n  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, stereo, fltp, 160 kb/s (default)\nStream mapping:\n  Stream #0:0 -> #0:0 (h264 (native) -> wrapped_avframe (native))\nOutput #0, null, to 'pipe:':\n  Stream #0:0: Video: wrapped_avframe, yuv420p, 1280x720, q=2-31, 200 kb/s, 30 fps\nframe=   60 fps=0.0 q=-0.0 size=N/A time=00:00:02.00\rframe=  151 fps=0.0 q=-0.0 Lsize=N/A time=00:00:05.03 bitrate=N/A speed=40x\n";

    #[test]
    fn parses_streams_and_frames() {
        let r = parse_decode(SAMPLE);
        assert_eq!(r.video_codec.as_deref(), Some("h264"));
        assert_eq!((r.width, r.height), (1280, 720));
        assert_eq!(r.fps, 30.0);
        assert_eq!(r.audio_streams, 1, "output streams aren't counted");
        assert_eq!(r.frames, 151);
        assert!((r.duration - 5.03).abs() < 0.001);
        assert!(r.errors.is_empty());
    }

    #[test]
    fn collects_decoder_errors() {
        let t = format!("{SAMPLE}[h264 @ 000001] error while decoding MB 10 20, bytestream -5\n[h264 @ 000001] concealing 120 DC errors\n");
        let r = parse_decode(&t);
        assert_eq!(r.errors.len(), 2);
    }

    #[test]
    fn no_video() {
        let r = parse_decode("Input #0, wav, from 'x.wav':\n  Stream #0:0: Audio: pcm_s16le, 48000 Hz, stereo\n");
        assert!(r.video_codec.is_none());
        assert_eq!(r.audio_streams, 1);
    }
}
