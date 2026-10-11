//! Instant replay: a bounded ring of short MPEG-TS segments on disk.
//!
//! FFmpeg's segment muxer writes `seg_NNNNN.ts` files and a CSV list of *closed* segments
//! with their start/end times. Saving reads only closed segments, so a save during a
//! segment transition can never include a half-written file. The chosen segments are
//! copied first (the ring may overwrite them) and joined without re-encoding.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One closed segment from the CSV list.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub file: String,
    pub start: f64,
    pub end: f64,
}

/// Parse `segments.csv` (`file,start,end` per line). Tolerates a partially written last line.
pub fn parse_list(text: &str) -> Vec<Segment> {
    text.lines()
        .filter_map(|l| {
            let mut parts = l.trim().rsplitn(3, ',');
            let end: f64 = parts.next()?.parse().ok()?;
            let start: f64 = parts.next()?.parse().ok()?;
            let file = parts.next()?.trim_matches('"').to_string();
            (!file.is_empty() && end >= start).then_some(Segment { file, start, end })
        })
        .collect()
}

/// Newest segments covering at least `seconds` (whole segments, oldest first).
pub fn select(segments: &[Segment], seconds: f64) -> Vec<Segment> {
    let mut picked = Vec::new();
    let mut total = 0.0;
    for s in segments.iter().rev() {
        if total >= seconds {
            break;
        }
        total += s.end - s.start;
        picked.push(s.clone());
    }
    picked.reverse();
    picked
}

/// Number of segment files to keep for a replay length.
pub fn ring_size(replay_seconds: u32, segment_seconds: u32) -> u32 {
    replay_seconds.div_ceil(segment_seconds.max(1)) + 4
}

/// Disk space the ring uses at a given total bitrate.
pub fn ring_bytes(replay_seconds: u32, segment_seconds: u32, bytes_per_second: u64) -> u64 {
    ring_size(replay_seconds, segment_seconds) as u64 * segment_seconds as u64 * bytes_per_second
}

pub struct ReplayBuffer {
    pub dir: PathBuf,
    pub segment_seconds: u32,
    pub replay_seconds: u32,
}

#[derive(Debug)]
pub enum SaveError {
    Empty,
    Io(std::io::Error),
    Join(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Empty => write!(f, "Nothing has been recorded yet."),
            SaveError::Io(e) => write!(f, "Couldn't write the replay: {e}"),
            SaveError::Join(e) => write!(f, "Couldn't join the replay: {e}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<std::io::Error> for SaveError {
    fn from(e: std::io::Error) -> Self {
        SaveError::Io(e)
    }
}

impl ReplayBuffer {
    pub fn new(dir: PathBuf, replay_seconds: u32, segment_seconds: u32) -> Self {
        ReplayBuffer { dir, segment_seconds, replay_seconds }
    }

    pub fn wrap(&self) -> u32 {
        ring_size(self.replay_seconds, self.segment_seconds)
    }

    /// Delete old segments before starting a new ring.
    pub fn clear(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        for e in std::fs::read_dir(&self.dir)?.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if (name.starts_with("seg_") && name.ends_with(".ts")) || name == "segments.csv" {
                let _ = std::fs::remove_file(p);
            }
        }
        Ok(())
    }

    pub fn segments(&self) -> Vec<Segment> {
        // The list is rewritten in place; retry if we catch it mid-write.
        for _ in 0..3 {
            if let Ok(t) = std::fs::read_to_string(self.dir.join("segments.csv")) {
                let s = parse_list(&t);
                if !s.is_empty() || t.trim().is_empty() {
                    return s;
                }
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        Vec::new()
    }

    /// Save the last `seconds` to `out` (MP4). Waits up to one segment so the moment the
    /// user pressed the hotkey is included. `ffmpeg` is used to join without re-encoding.
    pub fn save(&self, ffmpeg: &Path, seconds: u32, out: &Path, wait_for_current: bool) -> Result<PathBuf, SaveError> {
        let before = self.segments().last().map(|s| s.end).unwrap_or(-1.0);
        if wait_for_current {
            let deadline = Instant::now() + Duration::from_millis(self.segment_seconds as u64 * 1000 + 1500);
            while Instant::now() < deadline {
                if self.segments().last().map(|s| s.end).unwrap_or(-1.0) > before {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let chosen = select(&self.segments(), seconds as f64);
        if chosen.is_empty() {
            return Err(SaveError::Empty);
        }
        let stage = self.dir.join(format!("stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir_all(&stage)?;
        let mut list = String::new();
        for (i, s) in chosen.iter().enumerate() {
            let src = self.dir.join(&s.file);
            let dst = stage.join(format!("{i:05}.ts"));
            if std::fs::copy(&src, &dst).is_ok() {
                list += &format!("file '{}'\n", dst.to_string_lossy().replace('\\', "/").replace('\'', "'\\''"));
            }
        }
        if list.is_empty() {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(SaveError::Empty);
        }
        std::fs::write(stage.join("list.txt"), &list)?;
        let result = join_segments(ffmpeg, &stage.join("list.txt"), out);
        let _ = std::fs::remove_dir_all(&stage);
        result.map(|_| out.to_path_buf())
    }
}

/// Concatenate MPEG-TS segments listed in a concat file into a standard MP4.
pub fn join_segments(ffmpeg: &Path, list: &Path, out: &Path) -> Result<(), SaveError> {
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = out.with_extension("part.mp4");
    let args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-nostdin".into(),
        "-y".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        list.to_string_lossy().into(),
        "-map".into(),
        "0".into(),
        "-c".into(),
        "copy".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-f".into(),
        "mp4".into(),
        tmp.to_string_lossy().into(),
    ];
    let (ok, err) = crate::process::run_capture(ffmpeg, &args, Duration::from_secs(300)).map_err(SaveError::Io)?;
    if !ok || !tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
        return Err(SaveError::Join(err.lines().take(3).collect::<Vec<_>>().join(" ")));
    }
    // Never overwrite: the caller picked a unique name; rename the finished file into place.
    std::fs::rename(&tmp, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_selects() {
        let csv = "seg_00007.ts,14.000000,16.000000\nseg_00008.ts,16.000000,18.000000\nseg_00009.ts,18.000000,20.0";
        let l = parse_list(csv);
        assert_eq!(l.len(), 3);
        assert_eq!(l[2].end, 20.0);
        let partial = format!("{csv}\nseg_00010.ts,20.0");
        assert_eq!(parse_list(&partial).len(), 3);
        let s = select(&l, 3.0);
        assert_eq!(s.iter().map(|x| x.file.as_str()).collect::<Vec<_>>(), vec!["seg_00008.ts", "seg_00009.ts"]);
        assert_eq!(select(&l, 100.0).len(), 3);
        assert!(select(&[], 10.0).is_empty());
    }

    #[test]
    fn ring_math() {
        assert_eq!(ring_size(120, 2), 64);
        assert_eq!(ring_size(15, 2), 12);
        // 2 minutes at 2.5 MB/s ~ 320 MB of disk
        assert_eq!(ring_bytes(120, 2, 2_500_000), 320_000_000);
    }
}
