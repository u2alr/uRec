//! The clip editor's background work: opening clips, the preview player (picture follows the
//! sound clock), still frames while scrubbing, timeline thumbnails and waveforms, and export.
//! Everything heavy runs on worker threads with the bundled FFmpeg; the UI only reads results.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use urec_engine::edit::{self, ExportJob, ExportMode, Project, Source};
use urec_engine::process::quiet_command;

pub type Repaint = Arc<dyn Fn() + Send + Sync>;

/// A decoded picture, RGBA.
#[derive(Clone)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub rgba: Arc<Vec<u8>>,
    /// Changes whenever a new picture is published (to refresh the texture).
    pub generation: u64,
}

static GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_gen() -> u64 {
    GENERATION.fetch_add(1, Ordering::Relaxed)
}

fn spawn_piped(ffmpeg: &Path, args: &[String]) -> Option<Child> {
    let child = quiet_command(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        urec_win::bind_child_to_app(child.as_raw_handle());
    }
    Some(child)
}

/// Preview size: fit the output picture into `max_w`×`max_h`, even numbers.
pub fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let k = (max_w as f32 / w.max(1) as f32).min(max_h as f32 / h.max(1) as f32).min(1.0);
    let even = |v: f32| ((v as u32) / 2 * 2).max(2);
    (even(w as f32 * k), even(h as f32 * k))
}

// ------------------------------------------------------------------ opening

/// Read the clips' details in the background.
type OpenResult = Arc<Mutex<Option<Result<Vec<Source>, String>>>>;

pub struct Opening {
    pub result: OpenResult,
}

impl Opening {
    pub fn start(ffmpeg: PathBuf, paths: Vec<PathBuf>, repaint: Repaint) -> Opening {
        let result = Arc::new(Mutex::new(None));
        let r2 = result.clone();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut err = None;
            for p in &paths {
                match edit::probe_source(&ffmpeg, p) {
                    Ok(s) => out.push(s),
                    Err(e) => {
                        err = Some(format!(
                            "\"{}\" can't be edited: {e}.",
                            p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                        ));
                        break;
                    }
                }
            }
            *r2.lock().unwrap() = Some(match err {
                Some(e) => Err(e),
                None => Ok(out),
            });
            repaint();
        });
        Opening { result }
    }
}

// ------------------------------------------------------------------ player

pub struct Player {
    pub frame: Arc<Mutex<Option<Frame>>>,
    pub ended: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    children: Arc<Mutex<Vec<Child>>>,
    threads: Vec<JoinHandle<()>>,
    start_from: f64,
    clock: Arc<Clock>,
}

/// The preview clock: sound actually played, or the wall clock when there's no sound.
struct Clock {
    audio: Mutex<Option<Arc<urec_win::AudioOut>>>,
    started: Instant,
    /// Set when sound didn't start in time; then the wall clock rules.
    wall: AtomicBool,
}

impl Clock {
    fn now(&self) -> f64 {
        if !self.wall.load(Ordering::Relaxed) {
            if let Some(a) = self.audio.lock().unwrap().as_ref() {
                let p = a.position();
                if p > 0.0 || self.started.elapsed() < Duration::from_millis(1500) {
                    return p;
                }
                // No sound after 1.5 s: don't let the picture wait forever.
                self.wall.store(true, Ordering::Relaxed);
            }
        }
        self.started.elapsed().as_secs_f64()
    }
}

impl Player {
    /// Play the timeline from `from` at `w`×`h`.
    pub fn start(ffmpeg: &Path, project: &Project, from: f64, w: u32, h: u32, repaint: Repaint) -> Player {
        let fps = project.fps().min(30.0);
        let stop = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        let frame = Arc::new(Mutex::new(None));
        let children = Arc::new(Mutex::new(Vec::new()));
        let mut threads = Vec::new();
        let audio = if project.state.mute { None } else { urec_win::AudioOut::open().ok().map(Arc::new) };
        let clock = Arc::new(Clock {
            audio: Mutex::new(audio.clone()),
            started: Instant::now(),
            wall: AtomicBool::new(audio.is_none()),
        });
        // Sound.
        if let Some(out) = audio.clone() {
            let args = edit::preview_audio_args(project, from, urec_win::OUT_RATE, urec_win::OUT_CHANNELS);
            if let Some(mut child) = spawn_piped(ffmpeg, &args) {
                let mut stdout = child.stdout.take().expect("piped");
                children.lock().unwrap().push(child);
                let stop2 = stop.clone();
                threads.push(std::thread::spawn(move || {
                    let mut buf = vec![0u8; 32 * 1024];
                    let mut carry: Vec<u8> = Vec::new();
                    let mut samples = Vec::with_capacity(8192);
                    while !stop2.load(Ordering::Relaxed) {
                        let n = match stdout.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        carry.extend_from_slice(&buf[..n]);
                        let whole = carry.len() / 4 * 4;
                        samples.clear();
                        samples.extend(carry[..whole].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));
                        carry.drain(..whole);
                        if !out.push(&samples) {
                            break;
                        }
                    }
                    out.finish();
                }));
            } else {
                clock.wall.store(true, Ordering::Relaxed);
            }
        }
        // Picture.
        let args = edit::preview_video_args(project, from, w, h, fps);
        if let Some(mut child) = spawn_piped(ffmpeg, &args) {
            let mut stdout = child.stdout.take().expect("piped");
            children.lock().unwrap().push(child);
            let (stop2, ended2, frame2, clock2) = (stop.clone(), ended.clone(), frame.clone(), clock.clone());
            let size = (w * h * 4) as usize;
            threads.push(std::thread::spawn(move || {
                let mut k: u64 = 0;
                loop {
                    let mut px = vec![0u8; size];
                    if stdout.read_exact(&mut px).is_err() || stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    let t = k as f64 / fps;
                    k += 1;
                    // Late by more than two frames: skip this one to catch up.
                    if clock2.now() > t + 2.0 / fps {
                        continue;
                    }
                    while clock2.now() < t {
                        if stop2.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(3));
                    }
                    *frame2.lock().unwrap() =
                        Some(Frame { w: w as usize, h: h as usize, rgba: Arc::new(px), generation: next_gen() });
                    repaint();
                }
                ended2.store(true, Ordering::Relaxed);
                repaint();
            }));
        } else {
            ended.store(true, Ordering::Relaxed);
        }
        Player { frame, ended, stop, children, threads, start_from: from, clock }
    }

    /// Timeline time being played now.
    pub fn position(&self) -> f64 {
        self.start_from + self.clock.now()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for c in self.children.lock().unwrap().iter_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        // Dropping the sound output stops it; the reader threads end on the closed pipes.
        self.clock.audio.lock().unwrap().take();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

// ------------------------------------------------------------------ still frames

struct StillRequest {
    path: PathBuf,
    src_t: f64,
    t: f64,
    w: u32,
    h: u32,
    fast: bool,
}

/// Frames for scrubbing and stepping. Only the newest request is worked on.
pub struct Stills {
    pub frame: Arc<Mutex<Option<Frame>>>,
    pending: Arc<(Mutex<Option<StillRequest>>, Condvar)>,
    stop: Arc<AtomicBool>,
}

impl Stills {
    pub fn start(ffmpeg: PathBuf, repaint: Repaint) -> Stills {
        let frame = Arc::new(Mutex::new(None));
        let pending: Arc<(Mutex<Option<StillRequest>>, Condvar)> = Arc::new((Mutex::new(None), Condvar::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (f2, p2, s2) = (frame.clone(), pending.clone(), stop.clone());
        std::thread::spawn(move || loop {
            let req = {
                let (lock, cv) = &*p2;
                let mut g = lock.lock().unwrap();
                while g.is_none() && !s2.load(Ordering::Relaxed) {
                    g = cv.wait_timeout(g, Duration::from_millis(500)).unwrap().0;
                }
                if s2.load(Ordering::Relaxed) {
                    return;
                }
                g.take().unwrap()
            };
            let args = edit::frame_args(&req.path, req.src_t, req.w, req.h, req.fast);
            let size = (req.w * req.h * 4) as usize;
            if let Some(px) = edit::read_stdout(&ffmpeg, &args, Some(size)) {
                *f2.lock().unwrap() =
                    Some(Frame { w: req.w as usize, h: req.h as usize, rgba: Arc::new(px), generation: next_gen() });
                repaint();
            }
        });
        Stills { frame, pending, stop }
    }

    /// Show the timeline at `t` (fast = keyframe-accurate, for dragging).
    pub fn request(&self, project: &Project, t: f64, w: u32, h: u32, fast: bool) {
        let Some((i, src_t)) = project.locate(t) else { return };
        let src = &project.sources[project.pieces()[i].source];
        let (lock, cv) = &*self.pending;
        *lock.lock().unwrap() = Some(StillRequest { path: src.path.clone(), src_t, t, w, h, fast });
        cv.notify_one();
    }
}

impl Drop for Stills {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.pending.1.notify_all();
    }
}

// ------------------------------------------------------------------ thumbnails + waveforms

pub const THUMB_W: u32 = 96;
pub const THUMB_H: u32 = 54;
/// Waveform resolution: peaks per second.
pub const PEAKS_PER_SEC: usize = 50;

#[derive(Default)]
pub struct SourceMedia {
    /// (source time, RGBA THUMB_W×THUMB_H)
    pub thumbs: Vec<(f64, Arc<Vec<u8>>)>,
    pub peaks: Vec<f32>,
    pub thumbs_done: bool,
}

/// Thumbnails and waveforms for every clip, made once in the background.
pub struct Strips {
    pub media: Arc<Mutex<HashMap<usize, SourceMedia>>>,
    stop: Arc<AtomicBool>,
}

impl Strips {
    pub fn start(ffmpeg: PathBuf, sources: Vec<Source>, repaint: Repaint) -> Strips {
        let media: Arc<Mutex<HashMap<usize, SourceMedia>>> = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (m2, s2) = (media.clone(), stop.clone());
        std::thread::spawn(move || {
            // Waveforms first (one quick pass each), then thumbnails.
            for (i, src) in sources.iter().enumerate() {
                if s2.load(Ordering::Relaxed) {
                    return;
                }
                let rate = 1000u32;
                let args = edit::waveform_args(src, rate);
                let peaks = if args.is_empty() {
                    Vec::new()
                } else {
                    edit::read_stdout(&ffmpeg, &args, None)
                        .map(|b| {
                            let s: Vec<i16> = b.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect();
                            edit::peaks(&s, rate as usize / PEAKS_PER_SEC)
                        })
                        .unwrap_or_default()
                };
                m2.lock().unwrap().entry(i).or_default().peaks = peaks;
                repaint();
            }
            for (i, src) in sources.iter().enumerate() {
                let n = ((src.duration / 2.0).ceil() as usize).clamp(4, 60);
                for k in 0..n {
                    if s2.load(Ordering::Relaxed) {
                        return;
                    }
                    let t = src.duration * (k as f64 + 0.5) / n as f64;
                    let args = edit::frame_args(&src.path, t, THUMB_W, THUMB_H, true);
                    if let Some(px) = edit::read_stdout(&ffmpeg, &args, Some((THUMB_W * THUMB_H * 4) as usize)) {
                        m2.lock().unwrap().entry(i).or_default().thumbs.push((t, Arc::new(px)));
                        if k % 4 == 3 {
                            repaint();
                        }
                    }
                }
                m2.lock().unwrap().entry(i).or_default().thumbs_done = true;
                repaint();
            }
        });
        Strips { media, stop }
    }
}

impl Drop for Strips {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

// ------------------------------------------------------------------ export

pub struct Export {
    /// (done seconds, total seconds)
    pub progress: Arc<Mutex<(f64, f64)>>,
    pub result: Arc<Mutex<Option<Result<PathBuf, String>>>>,
    cancel: Arc<AtomicBool>,
}

impl Export {
    pub fn start(
        ffmpeg: PathBuf,
        project: Project,
        mode: ExportMode,
        video_args: Vec<String>,
        out: PathBuf,
        repaint: Repaint,
    ) -> Export {
        let progress = Arc::new(Mutex::new((0.0, project.duration())));
        let result = Arc::new(Mutex::new(None));
        let cancel = Arc::new(AtomicBool::new(false));
        let (p2, r2, c2) = (progress.clone(), result.clone(), cancel.clone());
        std::thread::spawn(move || {
            let job = ExportJob { mode, video_args: &video_args, audio_kbps: 192, out: &out };
            let mut last = Instant::now();
            let res = edit::run_export(
                &ffmpeg,
                &project,
                &job,
                &mut |d, t| {
                    *p2.lock().unwrap() = (d, t);
                    if last.elapsed() > Duration::from_millis(200) {
                        last = Instant::now();
                        repaint();
                    }
                },
                &c2,
            );
            crate::controller::log(&match &res {
                Ok(p) => format!("editor: exported {}", p.display()),
                Err(e) => format!("editor: export failed: {e}"),
            });
            *r2.lock().unwrap() = Some(res);
            repaint();
        });
        Export { progress, result, cancel }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Export {
    fn drop(&mut self) {
        // Leaving the editor mid-export stops it (the partial file is removed).
        if self.result.lock().map(|r| r.is_none()).unwrap_or(false) {
            self.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn preview_sizes() {
        assert_eq!(fit(1920, 1080, 960, 540), (960, 540));
        assert_eq!(fit(1920, 1080, 800, 800), (800, 450));
        assert_eq!(fit(608, 1080, 960, 540), (304, 540));
        assert_eq!(fit(640, 360, 960, 540), (640, 360), "never upscaled");
        let (w, h) = fit(1001, 777, 500, 500);
        assert!(w % 2 == 0 && h % 2 == 0);
    }
}
