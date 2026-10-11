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
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    /// key -> metadata
    entries: HashMap<String, Meta>,
}

enum Job {
    Refresh(PathBuf),
}

pub struct Library {
    pub items: Arc<Mutex<Vec<Item>>>,
    pub scanning: Arc<AtomicBool>,
    tx: Sender<Job>,
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
    /// `capturing` pauses thumbnail generation; `repaint` is called when items change.
    pub fn new(ffmpeg: Option<PathBuf>, capturing: Arc<AtomicBool>, repaint: Arc<dyn Fn() + Send + Sync>) -> Library {
        let items = Arc::new(Mutex::new(Vec::new()));
        let scanning = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel::<Job>();
        let (it, sc) = (items.clone(), scanning.clone());
        std::thread::Builder::new()
            .name("library".into())
            .spawn(move || {
                let cache_path = cache_dir().join("metadata.json");
                let mut cache: Cache =
                    std::fs::read(&cache_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
                let ffprobe =
                    ffmpeg.as_ref().map(|f| f.with_file_name(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" }));
                while let Ok(Job::Refresh(dir)) = rx.recv() {
                    sc.store(true, Ordering::Relaxed);
                    let mut found = scan(&dir);
                    // Metadata: cheap, from cache or ffprobe.
                    for item in found.iter_mut() {
                        let k = key(&item.path, item.size, item.modified);
                        if let Some(m) = cache.entries.get(&k) {
                            item.meta = Some(m.clone());
                        } else if let Some(fp) = ffprobe.as_deref().filter(|p| p.exists()) {
                            if let Some(m) = probe_meta(fp, &item.path) {
                                cache.entries.insert(k.clone(), m.clone());
                                item.meta = Some(m);
                            }
                        }
                        let t = cache_dir().join(format!("{k}.rgba"));
                        if t.exists() {
                            item.thumb = Some(t);
                        }
                    }
                    *it.lock().unwrap() = found.clone();
                    repaint();
                    let _ = std::fs::create_dir_all(cache_dir());
                    let _ = std::fs::write(&cache_path, serde_json::to_vec(&cache).unwrap_or_default());
                    // Thumbnails: only while idle, newest first, stop if a refresh arrives.
                    if let Some(ff) = ffmpeg.as_deref() {
                        for item in found.iter().filter(|i| i.thumb.is_none()).take(200) {
                            while capturing.load(Ordering::Relaxed) {
                                std::thread::sleep(Duration::from_secs(1));
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
                    sc.store(false, Ordering::Relaxed);
                }
            })
            .expect("spawn library");
        Library { items, scanning, tx }
    }

    pub fn refresh(&self, dir: &Path) {
        let _ = self.tx.send(Job::Refresh(dir.to_path_buf()));
    }

    pub fn snapshot(&self) -> Vec<Item> {
        self.items.lock().map(|v| v.clone()).unwrap_or_default()
    }

    /// Rename a recording (same folder, same extension). Never overwrites.
    pub fn rename(&self, item: &Item, new_name: &str) -> Result<PathBuf, String> {
        let clean = urec_engine::naming::sanitize(new_name);
        if clean.is_empty() {
            return Err("Enter a name.".into());
        }
        let ext = item.path.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
        let target = item.path.with_file_name(format!("{clean}.{ext}"));
        if target == item.path {
            return Ok(target);
        }
        if target.exists() {
            return Err(format!("A file named \"{clean}.{ext}\" already exists."));
        }
        std::fs::rename(&item.path, &target).map_err(|e| format!("Couldn't rename: {e}"))?;
        if let Some(x) = self.items.lock().unwrap().iter_mut().find(|x| x.path == item.path) {
            x.path = target.clone();
            x.name = clean;
        }
        Ok(target)
    }

    /// Move a recording to the Recycle Bin.
    pub fn delete(&self, item: &Item) -> Result<(), String> {
        if urec_win::recycle(&item.path) {
            self.items.lock().unwrap().retain(|x| x.path != item.path);
            Ok(())
        } else {
            Err("Couldn't move the file to the Recycle Bin. It may be open in another app.".into())
        }
    }
}

fn is_video(p: &Path) -> bool {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    (name.ends_with(".mp4") || name.ends_with(".mkv")) && !name.ends_with(".part.mp4")
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

fn probe_meta(ffprobe: &Path, file: &Path) -> Option<Meta> {
    let out = quiet_command(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,avg_frame_rate:format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(file)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut m = Meta::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "width" => m.width = v.parse().unwrap_or(0),
            "height" => m.height = v.parse().unwrap_or(0),
            "duration" => m.duration = v.parse().unwrap_or(0.0),
            "avg_frame_rate" => {
                if let Some((n, d)) = v.split_once('/') {
                    let (n, d): (f64, f64) = (n.parse().unwrap_or(0.0), d.parse().unwrap_or(1.0));
                    if d > 0.0 {
                        m.fps = (n / d * 100.0).round() / 100.0;
                    }
                }
            }
            _ => {}
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
        std::fs::write(d.join(".hidden").join("d.mp4"), b"x").unwrap();
        std::fs::write(d.join("notes.txt"), b"x").unwrap();
        let items = scan(&d);
        let mut names: Vec<_> = items.iter().map(|i| format!("{}/{}", i.app, i.name)).collect();
        names.sort();
        assert_eq!(names, vec!["/a", "VALORANT/b"]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(65.4), "1:05");
        assert_eq!(format_duration(3725.0), "1:02:05");
    }
}
