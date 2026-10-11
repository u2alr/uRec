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
    /// FFmpeg finished but the result isn't a complete MP4.
    Invalid(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Empty => write!(f, "Nothing has been recorded yet."),
            SaveError::Io(e) => write!(f, "Couldn't write the replay: {e}"),
            SaveError::Join(e) => write!(f, "Couldn't join the replay: {e}"),
            SaveError::Invalid(e) => write!(f, "The saved clip was damaged ({e}) and was discarded."),
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
    ///
    /// Safe to call from several threads at once: every call stages its own copies, and the
    /// finished clip never replaces an existing file (a free " (n)" name is used instead).
    /// Success is only returned once the MP4 has been checked and flushed to disk.
    pub fn save(
        &self,
        ffmpeg: &Path,
        seconds: u32,
        out: &Path,
        wait_for_current: bool,
    ) -> Result<SavedClip, SaveError> {
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
        let stage = self.dir.join(format!("stage-{}-{}", std::process::id(), next_id()));
        let _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir_all(&stage)?;
        let result = (|| {
            let files = copy_contiguous(&self.dir, &chosen, &stage)?;
            let list = concat_list(&files);
            std::fs::write(stage.join("list.txt"), &list)?;
            let path = join_segments(ffmpeg, &stage.join("list.txt"), out)?;
            let covered: f64 = chosen.iter().rev().take(files.len()).map(|s| s.end - s.start).sum();
            let info = crate::mp4check::check(&path).map_err(SaveError::Invalid)?;
            Ok(SavedClip { path, seconds: info.duration_secs.unwrap_or(covered), bytes: info.bytes })
        })();
        let _ = std::fs::remove_dir_all(&stage);
        result
    }
}

/// A finished, verified clip.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedClip {
    pub path: PathBuf,
    pub seconds: f64,
    pub bytes: u64,
}

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Copy the chosen segments into `stage`. The oldest ones may already have been recycled by
/// the ring while we were waiting; those are dropped from the front. A segment missing in
/// the middle would leave a hole in the clip, so that is an error instead.
pub fn copy_contiguous(dir: &Path, chosen: &[Segment], stage: &Path) -> Result<Vec<PathBuf>, SaveError> {
    let mut files = Vec::new();
    for s in chosen {
        let dst = stage.join(format!("{:05}.ts", files.len()));
        match std::fs::copy(dir.join(&s.file), &dst) {
            Ok(n) if n > 0 => files.push(dst),
            Ok(_) | Err(_) if files.is_empty() => continue,
            Ok(_) => return Err(SaveError::Join(format!("segment {} was empty", s.file))),
            Err(e) => return Err(SaveError::Join(format!("couldn't read segment {}: {e}", s.file))),
        }
    }
    if files.is_empty() {
        return Err(SaveError::Empty);
    }
    Ok(files)
}

/// An FFmpeg concat list for these files.
pub fn concat_list(files: &[PathBuf]) -> String {
    files
        .iter()
        .map(|p| format!("file '{}'\n", p.to_string_lossy().replace('\\', "/").replace('\'', "'\\''")))
        .collect()
}

/// Temporary name next to the final clip. Never shown in the library and removed at startup.
pub const TEMP_SUFFIX: &str = ".urec-tmp.mp4";

fn temp_path(out: &Path) -> PathBuf {
    let stem = out.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "clip".into());
    out.with_file_name(format!("{stem}.{}-{}{TEMP_SUFFIX}", std::process::id(), next_id()))
}

/// Move a finished file into place without ever replacing an existing file. If `out` is
/// taken, "name (2).ext", "name (3).ext"... are tried. Returns the final path.
pub fn place_no_overwrite(tmp: &Path, out: &Path) -> std::io::Result<PathBuf> {
    let stem = out.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = out.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    for n in 1..1000 {
        let candidate = if n == 1 { out.to_path_buf() } else { out.with_file_name(format!("{stem} ({n}).{ext}")) };
        // A hard link fails atomically if the name exists. Some drives (FAT32/exFAT) don't
        // support links; there, check-then-rename (saves are serialized by the caller).
        match std::fs::hard_link(tmp, &candidate) {
            Ok(()) => {
                let _ = std::fs::remove_file(tmp);
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                if candidate.exists() {
                    continue;
                }
                std::fs::rename(tmp, &candidate)?;
                return Ok(candidate);
            }
        }
    }
    Err(std::io::Error::other("no free file name"))
}

/// Remove leftovers of interrupted saves: staging folders in the buffer folder.
pub fn remove_stale_stages(buffer_dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(buffer_dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("stage-") && e.path().is_dir() {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Remove unfinished clips (`*.urec-tmp.mp4`) from a recordings folder and its direct
/// subfolders. Only uRec's own temporary names are touched; finished recordings never are.
pub fn remove_temp_outputs(dir: &Path) -> usize {
    let mut removed = 0;
    let mut dirs = vec![dir.to_path_buf()];
    if let Ok(rd) = std::fs::read_dir(dir) {
        dirs.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().ends_with(TEMP_SUFFIX) && std::fs::remove_file(e.path()).is_ok() {
                    removed += 1;
                }
            }
        }
    }
    removed
}

/// Concatenate MPEG-TS segments listed in a concat file into a standard MP4 at `out` (or a
/// free " (n)" variant of it). The file only appears under its final name once FFmpeg has
/// finished, the structure checks out and the data has been flushed to disk.
pub fn join_segments(ffmpeg: &Path, list: &Path, out: &Path) -> Result<PathBuf, SaveError> {
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = temp_path(out);
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
    let run = crate::process::run_capture(ffmpeg, &args, Duration::from_secs(300));
    let fail = |e: SaveError| {
        let _ = std::fs::remove_file(&tmp);
        Err(e)
    };
    let (ok, err) = match run {
        Ok(r) => r,
        Err(e) => return fail(SaveError::Io(e)),
    };
    if !ok || !tmp.exists() {
        return fail(SaveError::Join(err.lines().take(3).collect::<Vec<_>>().join(" ")));
    }
    if let Err(e) = crate::mp4check::check(&tmp) {
        return fail(SaveError::Invalid(e));
    }
    if let Err(e) = std::fs::OpenOptions::new().write(true).open(&tmp).and_then(|f| f.sync_all()) {
        return fail(SaveError::Io(e));
    }
    match place_no_overwrite(&tmp, out) {
        Ok(p) => Ok(p),
        Err(e) => fail(SaveError::Io(e)),
    }
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

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("urec-replay-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn concurrent_placement_never_overwrites() {
        let d = tmpdir("place");
        let out = d.join("Desktop 2026-10-12 01-02-03 Replay.mp4");
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let d = d.clone();
                let out = out.clone();
                std::thread::spawn(move || {
                    let tmp = d.join(format!("t{i}{TEMP_SUFFIX}"));
                    std::fs::write(&tmp, format!("clip {i}")).unwrap();
                    place_no_overwrite(&tmp, &out).unwrap()
                })
            })
            .collect();
        let mut paths: Vec<PathBuf> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        paths.sort();
        paths.dedup();
        assert_eq!(paths.len(), 8, "every save gets its own file");
        let mut contents: Vec<String> = paths.iter().map(|p| std::fs::read_to_string(p).unwrap()).collect();
        contents.sort();
        contents.dedup();
        assert_eq!(contents.len(), 8, "no clip replaced another");
        assert!(paths.iter().any(|p| p.ends_with("Desktop 2026-10-12 01-02-03 Replay (8).mp4")));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn copy_drops_recycled_head_but_not_holes() {
        let d = tmpdir("copy");
        let stage = d.join("stage");
        std::fs::create_dir_all(&stage).unwrap();
        let seg = |f: &str, a: f64| Segment { file: f.into(), start: a, end: a + 2.0 };
        std::fs::write(d.join("b.ts"), b"bb").unwrap();
        std::fs::write(d.join("c.ts"), b"cc").unwrap();
        // "a" was already recycled by the ring: dropped from the front.
        let got = copy_contiguous(&d, &[seg("a.ts", 0.0), seg("b.ts", 2.0), seg("c.ts", 4.0)], &stage).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(concat_list(&got).lines().count(), 2);
        // A missing segment in the middle is an error, never a silent gap.
        let err = copy_contiguous(&d, &[seg("b.ts", 2.0), seg("x.ts", 4.0), seg("c.ts", 6.0)], &stage);
        assert!(matches!(err, Err(SaveError::Join(_))));
        assert!(matches!(copy_contiguous(&d, &[seg("x.ts", 0.0)], &stage), Err(SaveError::Empty)));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn cleanup_touches_only_temp_files() {
        let d = tmpdir("cleanup");
        std::fs::create_dir_all(d.join("VALORANT")).unwrap();
        std::fs::write(d.join("keep.mp4"), b"x").unwrap();
        std::fs::write(d.join("keep.part.mp4"), b"x").unwrap();
        std::fs::write(d.join(format!("a.1-2{TEMP_SUFFIX}")), b"x").unwrap();
        std::fs::write(d.join("VALORANT").join(format!("b.1-3{TEMP_SUFFIX}")), b"x").unwrap();
        std::fs::write(d.join("VALORANT").join("clip.mp4"), b"x").unwrap();
        assert_eq!(remove_temp_outputs(&d), 2);
        assert!(d.join("keep.mp4").exists() && d.join("keep.part.mp4").exists());
        assert!(d.join("VALORANT").join("clip.mp4").exists());
        std::fs::create_dir_all(d.join("stage-1-2")).unwrap();
        std::fs::create_dir_all(d.join("logs")).unwrap();
        remove_stale_stages(&d);
        assert!(!d.join("stage-1-2").exists() && d.join("logs").exists());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn temp_names_are_unique() {
        let out = Path::new("C:/x/clip.mp4");
        assert_ne!(temp_path(out), temp_path(out));
        assert!(temp_path(out).to_string_lossy().ends_with(TEMP_SUFFIX));
    }
}
