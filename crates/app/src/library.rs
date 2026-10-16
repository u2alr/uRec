//! Recordings library: lists videos in the output folder with duration, resolution,
//! frame rate and size. Thumbnails are made lazily by FFmpeg as raw RGBA (no image
//! decoder needed) and never while a capture is running.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use urec_engine::process::{quiet_command, run_capture};

pub const THUMB_W: u32 = 320;
pub const THUMB_H: u32 = 180;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub name: String,
    /// Sub-folder (game/app) the file is in, if any.
    pub app: String,
    pub size: u64,
    pub modified: SystemTime,
    pub meta: Option<Meta>,
    /// Raw RGBA thumbnail file, THUMB_W × THUMB_H.
    pub thumb: Option<PathBuf>,
    /// Why the file can't be read, if FFmpeg couldn't open it (damaged or unfinished).
    pub problem: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    /// key -> metadata
    entries: HashMap<String, Meta>,
}

enum Job {
    Refresh(PathBuf),
    /// Opt-in clean-up, then a refresh.
    Retention {
        dir: PathBuf,
        days: u32,
        all: bool,
    },
}

pub struct Library {
    pub items: Arc<Mutex<Vec<Item>>>,
    pub scanning: Arc<AtomicBool>,
    tx: Sender<Job>,
    /// Starred recordings (full paths), kept in %APPDATA%\uRec\favorites.json.
    favorites: Arc<Mutex<Vec<PathBuf>>>,
}

fn favorites_path() -> PathBuf {
    crate::settings::Settings::path().with_file_name("favorites.json")
}

fn key(path: &Path, size: u64, modified: SystemTime) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    size.hash(&mut h);
    modified.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).hash(&mut h);
    format!("{:016x}", h.finish())
}

fn cache_dir() -> PathBuf {
    crate::settings::local_dir().join("library")
}

impl Library {
    /// `capturing` (a recording is being written) pauses thumbnails; `repaint` is called on changes.
    pub fn new(ffmpeg: Option<PathBuf>, capturing: Arc<AtomicBool>, repaint: Arc<dyn Fn() + Send + Sync>) -> Library {
        let items = Arc::new(Mutex::new(Vec::new()));
        let scanning = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel::<Job>();
        let (it, sc) = (items.clone(), scanning.clone());
        let favorites: Vec<PathBuf> =
            std::fs::read(favorites_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let favorites = Arc::new(Mutex::new(favorites));
        let favs = favorites.clone();
        std::thread::Builder::new()
            .name("library".into())
            .spawn(move || {
                let cache_path = cache_dir().join("metadata.json");
                let mut cache: Cache =
                    std::fs::read(&cache_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
                let mut pending: Option<Job> = None;
                loop {
                    let job = match pending.take() {
                        Some(j) => j,
                        None => match rx.recv() {
                            Ok(j) => j,
                            Err(_) => break,
                        },
                    };
                    let dir = match job {
                        Job::Refresh(dir) => dir,
                        Job::Retention { dir, days, all } => {
                            let found = scan(&dir);
                            let favs = favs.lock().map(|f| f.clone()).unwrap_or_default();
                            let picked = retention_candidates(
                                &found,
                                days,
                                all,
                                &|p| favs.iter().any(|f| f == p),
                                SystemTime::now(),
                            );
                            let moved = picked.iter().filter(|p| urec_win::recycle(p)).count();
                            if !picked.is_empty() {
                                crate::controller::log(&format!(
                                    "clean-up: {moved} of {} recording(s) older than {days} days moved to the Recycle Bin",
                                    picked.len()
                                ));
                            }
                            dir
                        }
                    };
                    sc.store(true, Ordering::Relaxed);
                    repaint();
                    let mut found = scan(&dir);
                    // Metadata: cheap, from the cache or `ffmpeg -i`.
                    for item in found.iter_mut() {
                        let k = key(&item.path, item.size, item.modified);
                        if let Some(m) = cache.entries.get(&k) {
                            item.meta = Some(m.clone());
                        } else if let Some(ff) = ffmpeg.as_deref() {
                            if let Some(m) = probe_meta(ff, &item.path) {
                                cache.entries.insert(k.clone(), m.clone());
                                item.meta = Some(m);
                            } else {
                                item.problem = Some(diagnose(&item.path, item.modified));
                            }
                        }
                        let t = cache_dir().join(format!("{k}.rgba"));
                        if t.exists() {
                            item.thumb = Some(t);
                        }
                    }
                    *it.lock().unwrap() = found.clone();
                    let _ = std::fs::create_dir_all(cache_dir());
                    let _ = std::fs::write(&cache_path, serde_json::to_vec(&cache).unwrap_or_default());
                    // The list is ready: stop the spinner before the slower thumbnail work.
                    sc.store(false, Ordering::Relaxed);
                    repaint();

                    // Thumbnails, newest first. Paused while a recording is written; a new
                    // refresh request interrupts and restarts the whole pass.
                    let Some(ff) = ffmpeg.as_deref() else { continue };
                    'thumbs: for item in found.iter().filter(|i| i.thumb.is_none() && i.problem.is_none()).take(200) {
                        while capturing.load(Ordering::Relaxed) {
                            if let Ok(j) = rx.recv_timeout(Duration::from_secs(1)) {
                                pending = Some(j);
                                break 'thumbs;
                            }
                        }
                        if let Ok(j) = rx.try_recv() {
                            pending = Some(j);
                            break 'thumbs;
                        }
                        let k = key(&item.path, item.size, item.modified);
                        let t = cache_dir().join(format!("{k}.rgba"));
                        let seek = item.meta.as_ref().map(|m| (m.duration / 3.0).min(2.0)).unwrap_or(1.0);
                        if make_thumb(ff, &item.path, &t, seek) {
                            if let Some(x) = it.lock().unwrap().iter_mut().find(|x| x.path == item.path) {
                                x.thumb = Some(t);
                            }
                            repaint();
                        }
                    }
                }
            })
            .expect("spawn library");
        Library { items, scanning, tx, favorites }
    }

    /// Run the opt-in clean-up rule in the background (then refresh the list).
    pub fn retention(&self, dir: &Path, days: u32, all: bool) {
        let _ = self.tx.send(Job::Retention { dir: dir.to_path_buf(), days, all });
    }

    /// How many recordings the rule would move right now (for the settings preview).
    pub fn retention_preview(&self, days: u32, all: bool) -> usize {
        let items = self.snapshot();
        retention_candidates(&items, days, all, &|p| self.is_favorite(p), SystemTime::now()).len()
    }

    pub fn is_favorite(&self, path: &Path) -> bool {
        self.favorites.lock().map(|f| f.iter().any(|x| x == path)).unwrap_or(false)
    }

    pub fn set_favorite(&self, path: &Path, on: bool) {
        if let Ok(mut f) = self.favorites.lock() {
            f.retain(|x| x != path);
            if on {
                f.push(path.to_path_buf());
            }
            let _ = std::fs::write(favorites_path(), serde_json::to_vec(&*f).unwrap_or_default());
        }
    }

    pub fn refresh(&self, dir: &Path) {
        let _ = self.tx.send(Job::Refresh(dir.to_path_buf()));
    }

    pub fn snapshot(&self) -> Vec<Item> {
        self.items.lock().map(|v| v.clone()).unwrap_or_default()
    }

    /// Rename a recording (same folder, same extension). Never overwrites.
    pub fn rename(&self, item: &Item, new_name: &str) -> Result<PathBuf, String> {
        let ext = item.path.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
        let clean = urec_engine::naming::sanitize(strip_extension(new_name, ext));
        if clean.is_empty() {
            return Err("Enter a name.".into());
        }
        let target = item.path.with_file_name(format!("{clean}.{ext}"));
        if target == item.path {
            return Ok(target);
        }
        // Windows file names ignore case, so "clip.mp4" -> "Clip.mp4" is the same file (and fine),
        // not a name that's already taken.
        let same_file = target.to_string_lossy().to_lowercase() == item.path.to_string_lossy().to_lowercase();
        if target.exists() && !same_file {
            return Err(format!("A file named \"{clean}.{ext}\" already exists."));
        }
        if !item.path.exists() {
            return Err("This recording was moved or deleted outside uRec.".into());
        }
        std::fs::rename(&item.path, &target).map_err(|e| format!("Couldn't rename: {e}"))?;
        if self.is_favorite(&item.path) {
            self.set_favorite(&item.path, false);
            self.set_favorite(&target, true);
        }
        if let Some(x) = self.items.lock().unwrap().iter_mut().find(|x| x.path == item.path) {
            x.path = target.clone();
            x.name = clean;
        }
        Ok(target)
    }

    /// Move a recording to the Recycle Bin.
    pub fn delete(&self, item: &Item) -> Result<(), String> {
        if !item.path.exists() {
            self.items.lock().unwrap().retain(|x| x.path != item.path);
            self.set_favorite(&item.path, false);
            return Err("This recording was already moved or deleted outside uRec.".into());
        }
        if urec_win::recycle(&item.path) {
            self.items.lock().unwrap().retain(|x| x.path != item.path);
            self.set_favorite(&item.path, false);
            Ok(())
        } else {
            Err("Couldn't move the file to the Recycle Bin. It may be open in another app.".into())
        }
    }

    /// Move several recordings to the Recycle Bin. Returns how many moved and the names that
    /// couldn't be (files already gone count as done).
    pub fn delete_many(&self, paths: &[PathBuf]) -> (usize, Vec<String>) {
        let mut ok = 0;
        let mut failed = Vec::new();
        for path in paths {
            if !path.exists() || urec_win::recycle(path) {
                ok += 1;
                self.items.lock().unwrap().retain(|x| &x.path != path);
                self.set_favorite(path, false);
            } else {
                failed.push(path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
            }
        }
        (ok, failed)
    }
}

/// People type the extension too ("clip.mp4"); the file keeps its own, so don't double it.
fn strip_extension<'a>(typed: &'a str, ext: &str) -> &'a str {
    let t = typed.trim();
    let suffix = format!(".{ext}");
    if t.len() > suffix.len() && t.to_ascii_lowercase().ends_with(&suffix.to_ascii_lowercase()) {
        &t[..t.len() - suffix.len()]
    } else {
        t
    }
}

/// Which recordings an opt-in clean-up rule would move to the Recycle Bin: older than `days`,
/// not a favorite, and (unless `all`) only Instant Replay clips. Recent files are never picked.
pub fn retention_candidates(
    items: &[Item],
    days: u32,
    all: bool,
    is_favorite: &dyn Fn(&Path) -> bool,
    now: SystemTime,
) -> Vec<PathBuf> {
    let max_age = Duration::from_secs(days.max(1) as u64 * 86_400);
    items
        .iter()
        .filter(|i| now.duration_since(i.modified).map(|age| age > max_age).unwrap_or(false))
        .filter(|i| all || i.name.ends_with(" Replay") || i.name.contains(" Replay ("))
        .filter(|i| !is_favorite(&i.path))
        .map(|i| i.path.clone())
        .collect()
}

fn is_video(p: &Path) -> bool {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    (name.ends_with(".mp4") || name.ends_with(".mkv"))
        && !name.ends_with(".part.mp4")
        && !name.ends_with(urec_engine::replay::TEMP_SUFFIX)
}

/// Plain-words reason a video can't be opened. Recent files may still be being written.
fn diagnose(path: &Path, modified: SystemTime) -> String {
    if SystemTime::now().duration_since(modified).map(|d| d.as_secs() < 30).unwrap_or(true) {
        return "Still being written".into();
    }
    let is_mp4 = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
    match is_mp4.then(|| urec_engine::mp4check::check(path)) {
        Some(Err(e)) => format!("Damaged: {e}"),
        _ => "Can't be read".into(),
    }
}

/// Videos in `dir` and its direct sub-folders, newest first.
pub fn scan(dir: &Path) -> Vec<Item> {
    let mut out = Vec::new();
    let mut visit = |d: &Path, app: &str| {
        let Ok(rd) = std::fs::read_dir(d) else { return Vec::new() };
        let mut subdirs = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                subdirs.push(p);
            } else if is_video(&p) {
                out.push(Item {
                    name: p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                    app: app.to_string(),
                    size: md.len(),
                    modified: md.modified().unwrap_or(UNIX_EPOCH),
                    meta: None,
                    thumb: None,
                    problem: None,
                    path: p,
                });
            }
        }
        subdirs
    };
    let subs = visit(dir, "");
    for s in subs {
        let app = s.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if app.starts_with('.') {
            continue;
        }
        visit(&s, &app);
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.modified));
    out
}

/// Read duration, size and frame rate from `ffmpeg -i` output (no ffprobe needed).
fn probe_meta(ffmpeg: &Path, file: &Path) -> Option<Meta> {
    let out = quiet_command(ffmpeg).args(["-hide_banner", "-nostdin", "-i"]).arg(file).output().ok()?;
    parse_meta(&String::from_utf8_lossy(&out.stderr))
}

pub fn parse_meta(text: &str) -> Option<Meta> {
    let mut m = Meta::default();
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Duration: ") {
            let d = rest.split(',').next().unwrap_or("");
            let parts: Vec<f64> = d.split(':').filter_map(|x| x.trim().parse().ok()).collect();
            if parts.len() == 3 {
                m.duration = parts[0] * 3600.0 + parts[1] * 60.0 + parts[2];
            }
        } else if l.starts_with("Stream #") && l.contains("Video:") && m.width == 0 {
            for tok in l.split([',', ' ']) {
                if let Some((w, h)) = tok.split_once('x') {
                    if let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.trim_end_matches(']').parse::<u32>()) {
                        if w >= 16 && h >= 16 && m.width == 0 {
                            m.width = w;
                            m.height = h;
                        }
                    }
                }
            }
            let segs: Vec<&str> = l.split(',').map(str::trim).collect();
            for s in segs {
                if let Some(v) = s.strip_suffix(" fps") {
                    m.fps = v.trim().parse().unwrap_or(0.0);
                }
            }
        }
    }
    (m.width > 0).then_some(m)
}

fn make_thumb(ffmpeg: &Path, file: &Path, out: &Path, seek: f64) -> bool {
    let _ = std::fs::create_dir_all(out.parent().unwrap_or(Path::new(".")));
    let vf = format!(
        "scale={THUMB_W}:{THUMB_H}:force_original_aspect_ratio=decrease:flags=bilinear,pad={THUMB_W}:{THUMB_H}:(ow-iw)/2:(oh-ih)/2:color=black"
    );
    let tmp = out.with_extension("tmp");
    let args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-nostdin".into(),
        "-y".into(),
        "-threads".into(),
        "1".into(),
        "-ss".into(),
        format!("{seek:.2}"),
        "-i".into(),
        file.to_string_lossy().into(),
        "-frames:v".into(),
        "1".into(),
        "-vf".into(),
        vf,
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        tmp.to_string_lossy().into(),
    ];
    let ok = run_capture(ffmpeg, &args, Duration::from_secs(20)).map(|(ok, _)| ok).unwrap_or(false);
    let size_ok = tmp.metadata().map(|m| m.len() == (THUMB_W * THUMB_H * 4) as u64).unwrap_or(false);
    if ok && size_ok {
        std::fs::rename(&tmp, out).is_ok()
    } else {
        let _ = std::fs::remove_file(&tmp);
        false
    }
}

pub fn format_duration(secs: f64) -> String {
    let s = secs.round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_nested_and_skips_partials() {
        let d = std::env::temp_dir().join(format!("urec-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("VALORANT")).unwrap();
        std::fs::create_dir_all(d.join(".hidden")).unwrap();
        std::fs::write(d.join("a.mp4"), b"x").unwrap();
        std::fs::write(d.join("VALORANT").join("b.mkv"), b"x").unwrap();
        std::fs::write(d.join("VALORANT").join("c.part.mp4"), b"x").unwrap();
        std::fs::write(d.join("VALORANT").join(format!("e.1-2{}", urec_engine::replay::TEMP_SUFFIX)), b"x").unwrap();
        std::fs::write(d.join(".hidden").join("d.mp4"), b"x").unwrap();
        std::fs::write(d.join("notes.txt"), b"x").unwrap();
        let items = scan(&d);
        let mut names: Vec<_> = items.iter().map(|i| format!("{}/{}", i.app, i.name)).collect();
        names.sort();
        assert_eq!(names, vec!["/a", "VALORANT/b"]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn parses_ffmpeg_info() {
        let text = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'a.mp4':\n  Duration: 00:01:05.20, start: 0.000000, bitrate: 2036 kb/s\n  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 1920x1080 [SAR 1:1 DAR 16:9], 1990 kb/s, 59.94 fps, 60 tbr, 15360 tbn (default)\n  Stream #0:1[0x2](und): Audio: aac (LC), 48000 Hz, stereo, fltp, 160 kb/s";
        let m = parse_meta(text).unwrap();
        assert_eq!((m.width, m.height), (1920, 1080));
        assert!((m.duration - 65.2).abs() < 1e-6);
        assert!((m.fps - 59.94).abs() < 1e-6);
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(65.4), "1:05");
        assert_eq!(format_duration(3725.0), "1:02:05");
    }

    #[test]
    fn diagnoses_unreadable_files() {
        let d = std::env::temp_dir().join(format!("urec-lib-diag-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let f = d.join("broken.mp4");
        std::fs::write(&f, [0u8; 64]).unwrap();
        assert_eq!(diagnose(&f, SystemTime::now()), "Still being written");
        let old = SystemTime::now() - Duration::from_secs(3600);
        assert!(diagnose(&f, old).starts_with("Damaged: "));
        let mkv = d.join("x.mkv");
        std::fs::write(&mkv, b"x").unwrap();
        assert_eq!(diagnose(&mkv, old), "Can't be read");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn retention_picks_only_old_unstarred_clips() {
        let now = SystemTime::now();
        let day = Duration::from_secs(86_400);
        let item = |name: &str, age_days: u64| Item {
            path: PathBuf::from(format!("C:/v/{name}.mp4")),
            name: name.into(),
            app: String::new(),
            size: 1,
            modified: now - day * age_days as u32,
            meta: None,
            thumb: None,
            problem: None,
        };
        let items = vec![
            item("VALORANT 2026-09-01 Replay", 40),
            item("VALORANT 2026-09-01 Replay (2)", 40),
            item("VALORANT 2026-10-10 Replay", 2),
            item("Desktop 2026-09-01", 40),
            item("Starred 2026-09-01 Replay", 40),
        ];
        let fav = |p: &Path| p.to_string_lossy().contains("Starred");
        let picked = retention_candidates(&items, 30, false, &fav, now);
        let names: Vec<_> = picked.iter().map(|p| p.file_stem().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["VALORANT 2026-09-01 Replay", "VALORANT 2026-09-01 Replay (2)"]);
        let all = retention_candidates(&items, 30, true, &fav, now);
        assert_eq!(all.len(), 3, "recordings too, still never favorites or recent files");
        assert!(retention_candidates(&items, 60, true, &fav, now).is_empty());
    }

    #[test]
    fn typed_extension_is_not_doubled() {
        assert_eq!(strip_extension("clip.mp4", "mp4"), "clip");
        assert_eq!(strip_extension("  My Clip.MP4 ", "mp4"), "My Clip");
        assert_eq!(strip_extension("clip.mkv", "mp4"), "clip.mkv");
        assert_eq!(strip_extension(".mp4", "mp4"), ".mp4");
        assert_eq!(strip_extension("Desktop 10.00.00", "mp4"), "Desktop 10.00.00");
        assert_eq!(strip_extension("日本語.mp4", "mp4"), "日本語");
    }
}
