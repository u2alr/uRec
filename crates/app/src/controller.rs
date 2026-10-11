//! The recording controller. Runs on its own thread and owns every capture session,
//! the audio feed and the global hotkeys, so recording keeps working when the window is
//! hidden. The UI sends `Command`s and reads a `Status` snapshot.

use crate::settings::Settings;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use urec_engine::encoder::{Backend, Encoder};
use urec_engine::hotkey::Hotkey;
use urec_engine::naming::{self, Stamp};
use urec_engine::pipeline::{AudioInput, CaptureMethod, CaptureTarget, Output, Rect};
use urec_engine::probe::{self, ProbeResult};
use urec_engine::process::{locate_ffmpeg, Stats};
use urec_engine::replay::{self, ReplayBuffer};
use urec_engine::session::{Session, StartRequest};
use urec_engine::video::{self, Capabilities, Codec, Preset};
use urec_win::{AudioFeed, SourceState};

/// Stop recording if free space drops below this.
const HARD_MIN_FREE: u64 = 1_000_000_000;
const SEGMENT_SECONDS: u32 = 2;

#[derive(Clone, Debug)]
pub enum Command {
    ToggleRecording,
    SetReplay(bool),
    ToggleReplay,
    SaveReplay,
    ToggleMicMute,
    ToggleOverlay,
    /// New settings from the UI. The controller restarts Instant Replay if needed.
    Apply(Box<Settings>),
    Reprobe,
    Shutdown,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct Notice {
    pub level: Level,
    pub text: String,
    pub at: Instant,
    pub file: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct ActiveCapture {
    pub method: CaptureMethod,
    pub encoder: Encoder,
    pub gpu_path: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub fallbacks: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Recording {
    pub started: Instant,
    pub output: PathBuf,
    pub app: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ReplayState {
    #[default]
    Off,
    Starting,
    On,
    Error(String),
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub ready: bool,
    pub ffmpeg: Option<PathBuf>,
    pub ffmpeg_version: String,
    pub probing: bool,
    pub probe: Vec<ProbeResult>,
    pub recording: Option<Recording>,
    pub finishing: bool,
    pub starting: bool,
    pub replay: ReplayState,
    pub capture: Option<ActiveCapture>,
    pub stats: Stats,
    pub lagging: bool,
    pub audio: (Option<SourceState>, Option<SourceState>),
    pub levels: (f32, f32),
    pub mic_muted: bool,
    pub notices: VecDeque<Notice>,
    pub disk_free: Option<u64>,
    pub hotkey_failures: Vec<String>,
    pub overlay_visible: bool,
    pub last_saved: Option<PathBuf>,
}

impl Status {
    pub fn busy(&self) -> bool {
        self.recording.is_some() || self.finishing || self.replay == ReplayState::On
    }

    pub fn best_encoder(&self, codec: Codec) -> Option<Encoder> {
        probe::best(&self.probe, codec)
    }
}

type RepaintFn = Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>;

/// Handle used by the UI and hotkeys.
#[derive(Clone)]
pub struct Controller {
    tx: Arc<Mutex<Sender<Command>>>,
    pub status: Arc<Mutex<Status>>,
    /// True while capturing; background work (thumbnails) waits for this to clear.
    pub capturing: Arc<AtomicBool>,
    repaint: RepaintFn,
    /// Set when the controller thread has shut down cleanly.
    pub finished: Arc<AtomicBool>,
}

impl Controller {
    pub fn start(settings: Settings) -> Controller {
        let (tx, rx) = channel();
        let ctl = Controller {
            tx: Arc::new(Mutex::new(tx)),
            status: Arc::new(Mutex::new(Status { overlay_visible: settings.appearance.overlay, ..Status::default() })),
            capturing: Arc::new(AtomicBool::new(false)),
            repaint: Arc::new(Mutex::new(None)),
            finished: Arc::new(AtomicBool::new(false)),
        };
        let c2 = ctl.clone();
        std::thread::Builder::new()
            .name("controller".into())
            .spawn(move || Worker::new(c2, settings, rx).run())
            .expect("spawn controller");
        ctl
    }

    pub fn send(&self, cmd: Command) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(cmd);
        }
    }

    pub fn snapshot(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn set_repaint(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.repaint.lock().unwrap() = Some(Box::new(f));
    }

    fn repaint(&self) {
        if let Some(f) = self.repaint.lock().unwrap().as_ref() {
            f();
        }
    }

    /// Ask the controller to stop everything and wait (up to `timeout`) until it has.
    pub fn shutdown_and_wait(&self, timeout: Duration) {
        self.send(Command::Shutdown);
        let start = Instant::now();
        while !self.finished.load(Ordering::Relaxed) && start.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn dismiss_notice(&self, index: usize) {
        if let Ok(mut s) = self.status.lock() {
            if index < s.notices.len() {
                s.notices.remove(index);
            }
        }
    }
}

// ------------------------------------------------------------------ worker

/// Recording that piggybacks on the Instant Replay ring: closed segments are copied as
/// they appear and joined when recording stops. Nothing is encoded twice.
struct RingRecording {
    dir: PathBuf,
    /// Stream time (seconds) from which segments belong to this recording.
    from: f64,
    copied: Vec<PathBuf>,
    last_end: f64,
}

struct Live {
    session: Session,
    audio: Option<AudioFeed>,
    kind: LiveKind,
}

#[derive(PartialEq)]
enum LiveKind {
    File,
    Replay,
}

struct Worker {
    ctl: Controller,
    s: Settings,
    rx: Receiver<Command>,
    ffmpeg: Option<PathBuf>,
    live: Option<Live>,
    ring: Option<RingRecording>,
    recording: Option<Recording>,
    hotkeys: urec_win::Hotkeys,
    overlay: urec_win::Overlay,
    lag_since: Option<Instant>,
    lag_warned: bool,
    last_disk_check: Instant,
    low_space_warned: bool,
    crash_restarts: u32,
}

impl Worker {
    fn new(ctl: Controller, s: Settings, rx: Receiver<Command>) -> Self {
        let tx = ctl.tx.clone();
        log("controller: creating hotkeys");
        let hotkeys = urec_win::Hotkeys::new(move |i| {
            let cmd = match i {
                0 => Command::ToggleRecording,
                1 => Command::SaveReplay,
                2 => Command::ToggleReplay,
                3 => Command::ToggleMicMute,
                _ => Command::ToggleOverlay,
            };
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(cmd);
            }
        });
        Worker {
            ctl,
            s,
            rx,
            ffmpeg: None,
            live: None,
            ring: None,
            recording: None,
            hotkeys,
            overlay: {
                log("controller: creating overlay");
                let o = if std::env::var_os("UREC_NO_OVERLAY").is_some() {
                    urec_win::Overlay::disabled()
                } else {
                    urec_win::Overlay::new()
                };
                log("controller: overlay ready");
                o
            },
            lag_since: None,
            lag_warned: false,
            last_disk_check: Instant::now() - Duration::from_secs(60),
            low_space_warned: false,
            crash_restarts: 0,
        }
    }

    fn update(&self, f: impl FnOnce(&mut Status)) {
        if let Ok(mut s) = self.ctl.status.lock() {
            f(&mut s);
        }
        self.ctl.repaint();
    }

    fn notice(&self, level: Level, text: impl Into<String>, file: Option<PathBuf>) {
        let text = text.into();
        log(&format!("{level:?}: {text}"));
        self.update(|s| {
            s.notices.push_front(Notice { level, text, at: Instant::now(), file });
            s.notices.truncate(6);
        });
    }

    fn run(mut self) {
        self.init();
        loop {
            let tick = if self.s.general.performance_mode { 1000 } else { 250 };
            match self.rx.recv_timeout(Duration::from_millis(tick)) {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.tick();
        }
        self.shutdown();
        self.ctl.finished.store(true, Ordering::Relaxed);
    }

    // -------------------------------------------------------------- startup

    fn init(&mut self) {
        log("controller: init");
        let ffmpeg = locate_ffmpeg();
        let version = ffmpeg.as_deref().and_then(probe::ffmpeg_version).unwrap_or_default();
        self.update(|s| {
            s.ffmpeg = ffmpeg.clone();
            s.ffmpeg_version = version.clone();
        });
        self.ffmpeg = ffmpeg;
        if self.ffmpeg.is_none() {
            self.notice(
                Level::Error,
                "FFmpeg is missing, so recording isn't possible. Reinstall uRec to fix this.",
                None,
            );
            self.update(|s| s.ready = true);
            return;
        }
        self.register_hotkeys();
        log("controller: hotkeys registered");
        self.run_probe();
        log("controller: probe done");
        if !self.s.first_run_done {
            self.recommend_defaults();
        }
        self.update(|s| s.ready = true);
        if self.s.replay.enabled {
            self.start_replay();
        }
    }

    fn run_probe(&mut self) {
        let Some(ff) = self.ffmpeg.clone() else { return };
        self.update(|s| s.probing = true);
        let results = probe::probe_encoders(&ff, &Codec::ALL, &self.s.video_settings());
        for r in &results {
            log(&format!(
                "probe {}: ok={} {}",
                r.encoder.ffmpeg_name().unwrap_or("?"),
                r.ok,
                if r.ok { "" } else { &r.reason }
            ));
        }
        self.update(|s| {
            s.probe = results;
            s.probing = false;
        });
    }

    /// First launch: pick a preset that suits this PC.
    fn recommend_defaults(&mut self) {
        let st = self.ctl.snapshot();
        let mon = urec_win::monitors().into_iter().next();
        let caps = Capabilities {
            hardware_encoder: st.best_encoder(Codec::H264).is_some_and(|e| e.backend.is_hardware()),
            hevc: st.best_encoder(Codec::Hevc).is_some(),
            av1: st.best_encoder(Codec::Av1).is_some(),
            source_width: mon.as_ref().map(|m| m.bounds.width).unwrap_or(1920),
            source_height: mon.as_ref().map(|m| m.bounds.height).unwrap_or(1080),
            source_refresh: mon.as_ref().and_then(|m| m.refresh_hz),
            logical_cpus: probe::logical_cpus(),
        };
        let preset = if caps.hardware_encoder {
            Preset::High
        } else if caps.logical_cpus >= 8 {
            Preset::Balanced
        } else {
            Preset::LowEnd
        };
        let v = video::apply_preset(preset, &caps);
        self.s.set_video(&v);
        self.s.video.preset = preset_id(preset).into();
        self.s.first_run_done = true;
        let _ = self.s.save();
        log(&format!("first run: preset {preset:?}, hw={}", caps.hardware_encoder));
    }

    fn register_hotkeys(&mut self) {
        let h = &self.s.hotkeys;
        let names = [
            ("Start/stop recording", &h.record),
            ("Save Instant Replay", &h.save_replay),
            ("Turn Instant Replay on/off", &h.toggle_replay),
            ("Mute microphone", &h.mute_mic),
            ("Show/hide overlay", &h.overlay),
        ];
        let keys: Vec<Option<Hotkey>> = names.iter().map(|(_, k)| Hotkey::parse(k)).collect();
        let failed = self.hotkeys.set(&keys);
        let failures: Vec<String> =
            failed.iter().map(|&i| format!("{} ({}) is already used by another app", names[i].0, names[i].1)).collect();
        if !failures.is_empty() {
            self.notice(Level::Warning, format!("Some hotkeys couldn't be registered: {}", failures.join("; ")), None);
        }
        self.update(|s| s.hotkey_failures = failures);
    }

    // -------------------------------------------------------------- commands

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::ToggleRecording => {
                if self.recording.is_some() {
                    self.stop_recording()
                } else {
                    self.start_recording()
                }
            }
            Command::SetReplay(on) => self.set_replay(on),
            Command::ToggleReplay => self.set_replay(!self.s.replay.enabled),
            Command::SaveReplay => self.save_replay(),
            Command::ToggleMicMute => self.toggle_mute(),
            Command::ToggleOverlay => {
                self.update(|s| s.overlay_visible = !s.overlay_visible);
                self.update_overlay();
            }
            Command::Apply(new) => self.apply(*new),
            Command::Reprobe => {
                if !self.ctl.snapshot().busy() {
                    self.run_probe();
                }
            }
            Command::Shutdown => {}
        }
    }

    fn apply(&mut self, new: Settings) {
        let replay_relevant = new.video != self.s.video
            || new.audio != self.s.audio
            || new.capture != self.s.capture
            || new.replay.seconds != self.s.replay.seconds
            || new.replay.buffer_dir != self.s.replay.buffer_dir;
        let hotkeys_changed = new.hotkeys != self.s.hotkeys;
        let replay_toggled = new.replay.enabled != self.s.replay.enabled;
        let volumes_changed =
            new.audio.system_volume != self.s.audio.system_volume || new.audio.mic_volume != self.s.audio.mic_volume;
        self.s = new;
        if hotkeys_changed {
            self.register_hotkeys();
        }
        if volumes_changed {
            if let Some(feed) = self.live.as_ref().and_then(|l| l.audio.as_ref()) {
                feed.set_volumes(self.s.audio.system_volume, self.s.audio.mic_volume);
            }
        }
        if replay_toggled {
            self.set_replay(self.s.replay.enabled);
        } else if replay_relevant
            && self.s.replay.enabled
            && self.recording.is_none()
            && self.live.as_ref().is_some_and(|l| l.kind == LiveKind::Replay)
        {
            // Restart the buffer with the new settings (only when not recording into it).
            self.stop_live();
            self.start_replay();
        }
    }

    fn toggle_mute(&mut self) {
        if !self.s.audio.mic {
            self.notice(Level::Info, "The microphone is turned off in Settings › Audio.", None);
            return;
        }
        let muted = !self.ctl.snapshot().mic_muted;
        if let Some(feed) = self.live.as_ref().and_then(|l| l.audio.as_ref()) {
            feed.set_mic_muted(muted);
        }
        self.update(|s| s.mic_muted = muted);
    }

    // -------------------------------------------------------------- capture setup

    /// Turn the capture settings into a target plus the capture methods worth trying.
    fn resolve_target(&self) -> Result<(CaptureTarget, Vec<CaptureMethod>, String), String> {
        let c = &self.s.capture;
        let monitors = urec_win::monitors();
        let pick_monitor = || {
            monitors
                .iter()
                .find(|m| !c.monitor.is_empty() && m.device == c.monitor)
                .or_else(|| monitors.iter().find(|m| m.primary))
                .or(monitors.first())
                .cloned()
        };
        let (target, dda_ok, app) = match c.kind.as_str() {
            "window" => {
                let wins = urec_win::windows();
                let w = wins
                    .iter()
                    .find(|w| {
                        !c.window_exe.is_empty()
                            && w.exe.eq_ignore_ascii_case(&c.window_exe)
                            && w.title == c.window_title
                    })
                    .or_else(|| {
                        wins.iter().find(|w| !c.window_exe.is_empty() && w.exe.eq_ignore_ascii_case(&c.window_exe))
                    })
                    .or_else(|| wins.iter().find(|w| !c.window_title.is_empty() && w.title == c.window_title))
                    .ok_or_else(|| {
                        format!(
                            "The window \"{}\" isn't open. Open it, or pick another source on the Home page.",
                            if c.window_title.is_empty() { &c.window_exe } else { &c.window_title }
                        )
                    })?;
                (w.target(), false, w.app_name())
            }
            "region" => {
                let m = pick_monitor().ok_or("No display found.")?;
                let [x, y, w, h] = c.region;
                let region = Rect {
                    x: x.clamp(0, m.bounds.width as i32 - 16),
                    y: y.clamp(0, m.bounds.height as i32 - 16),
                    width: (w.max(16) as u32).min(m.bounds.width),
                    height: (h.max(16) as u32).min(m.bounds.height),
                };
                let t = CaptureTarget::Region {
                    index: m.dxgi_output.unwrap_or(0),
                    hmonitor: m.hmonitor,
                    bounds: m.bounds,
                    region,
                };
                (t, m.dda_capable(), foreground_app())
            }
            _ => {
                let m = pick_monitor().ok_or("No display found.")?;
                (m.target(), m.dda_capable(), foreground_app())
            }
        };
        let mut methods = match c.method.as_str() {
            "wgc" => vec![CaptureMethod::Wgc],
            "dda" => vec![CaptureMethod::Dda],
            "gdi" => vec![CaptureMethod::Gdi],
            _ => CaptureMethod::order_for(&target),
        };
        let wgc = urec_win::supports_wgc();
        methods.retain(|m| match m {
            CaptureMethod::Wgc => wgc,
            CaptureMethod::Dda => dda_ok,
            _ => true,
        });
        if methods.is_empty() {
            return Err("The selected capture method doesn't work for this source on this PC. Choose Automatic in Settings › Recording.".into());
        }
        Ok((target, methods, app))
    }

    fn pick_encoder(&self) -> Result<Encoder, String> {
        let st = self.ctl.snapshot();
        let codec = self.s.codec();
        if self.s.video.encoder == "auto" {
            return st
                .best_encoder(codec)
                .ok_or_else(|| format!("No encoder for {codec} works on this PC. Choose H.264 in Settings › Video."));
        }
        let backend = Backend::from_id(&self.s.video.encoder).ok_or("Unknown encoder in settings.")?;
        let enc = Encoder::new(backend, codec);
        if enc.ffmpeg_name().is_none() {
            return Err(format!("{} can't encode {codec}.", backend.label()));
        }
        if let Some(r) = st.probe.iter().find(|r| r.encoder == enc) {
            if !r.ok {
                return Err(format!("{} isn't available: {}", enc, r.reason));
            }
        }
        Ok(enc)
    }

    fn check_folder(&self, dir: &Path, need: u64, what: &str) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("Can't create the {what} folder {}: {e}", dir.display()))?;
        let probe_file = dir.join(".urec-write-test");
        std::fs::write(&probe_file, b"ok").map_err(|e| {
            format!(
                "Can't write to the {what} folder {} ({e}). Choose another folder in Settings › Storage.",
                dir.display()
            )
        })?;
        let _ = std::fs::remove_file(probe_file);
        if let Some((free, _)) = urec_win::disk_space(dir) {
            if free < need {
                return Err(format!(
                    "Not enough free space for the {what}: {} free on that drive, at least {} needed.",
                    video::format_bytes(free),
                    video::format_bytes(need)
                ));
            }
        }
        Ok(())
    }

    fn launch(&mut self, output: Output, kind: LiveKind) -> Result<(), String> {
        let ffmpeg = self.ffmpeg.clone().ok_or("FFmpeg is missing.")?;
        let (target, methods, _app) = self.resolve_target()?;
        let encoder = self.pick_encoder()?;
        let (sw, sh) = target.source_size();
        let refresh = urec_win::monitors().iter().find(|m| m.primary).and_then(|m| m.refresh_hz);
        let v = self.s.video_settings();
        let (w, h, fps) = video::validate(&v, sw, sh, refresh).map_err(|e| e.to_string())?;
        let req = StartRequest {
            target,
            methods,
            cursor: self.s.capture.cursor,
            encoder,
            video: v,
            out_width: w,
            out_height: h,
            fps,
            audio_kbps: self.s.audio.bitrate_kbps,
            output,
            prefer_gpu_path: true,
            first_frame_timeout: Duration::from_secs(10),
        };
        let audio_settings = self.s.audio_settings();
        let mut feed_slot: Option<AudioFeed> = None;
        let want_audio = audio_settings.system || audio_settings.mic;
        let mut make_audio = || -> Vec<AudioInput> {
            feed_slot = None; // free the previous attempt's devices and pipes first
            if !want_audio {
                return Vec::new();
            }
            let (feed, inputs) = AudioFeed::start(&audio_settings);
            feed_slot = Some(feed);
            inputs
        };
        let log_dir = crate::settings::local_dir().join("logs");
        self.update(|s| s.starting = true);
        let result = Session::start(&ffmpeg, &log_dir, &req, &mut make_audio);
        self.update(|s| s.starting = false);
        let session = result.map_err(|e| {
            log(&e.to_string());
            e.to_string()
        })?;
        #[cfg(windows)]
        urec_win::bind_child_to_app(session.process.raw_handle());
        let muted = self.ctl.snapshot().mic_muted;
        if let Some(f) = &feed_slot {
            f.set_mic_muted(muted);
        }
        let info = ActiveCapture {
            method: session.plan.method,
            encoder: session.plan.encoder,
            gpu_path: session.plan.frame_path == urec_engine::pipeline::FramePath::Gpu,
            width: session.plan.out_width,
            height: session.plan.out_height,
            fps: session.plan.fps,
            fallbacks: session
                .attempts
                .iter()
                .map(|a| format!("{}: {}", a.method.label(), last_line(&a.error)))
                .collect(),
        };
        log(&format!(
            "capture started: {} via {} ({}), {}x{}@{}",
            info.encoder,
            info.method.label(),
            if info.gpu_path { "GPU" } else { "system memory" },
            info.width,
            info.height,
            info.fps
        ));
        let (sys, mic) = feed_slot.as_ref().map(|f| f.states()).unwrap_or((SourceState::Off, SourceState::Off));
        self.update(|s| {
            s.capture = Some(info);
            s.audio = (Some(sys.clone()), Some(mic.clone()));
        });
        for (name, state) in [("Game & desktop sound", &sys), ("Microphone", &mic)] {
            if let SourceState::Failed { reason } = state {
                self.notice(
                    Level::Warning,
                    format!("{name} isn't being recorded: {}", explain_audio_failure(reason)),
                    None,
                );
            }
        }
        self.live = Some(Live { session, audio: feed_slot, kind });
        self.ctl.capturing.store(true, Ordering::Relaxed);
        self.lag_since = None;
        self.lag_warned = false;
        Ok(())
    }

    fn stop_live(&mut self) -> Option<Result<Option<PathBuf>, String>> {
        let live = self.live.take()?;
        let result = live.session.stop(Duration::from_secs(20));
        drop(live.audio);
        self.ctl.capturing.store(false, Ordering::Relaxed);
        self.update(|s| {
            s.capture = None;
            s.stats = Stats::default();
            s.audio = (None, None);
            s.levels = (0.0, 0.0);
            s.lagging = false;
        });
        Some(result)
    }

    // -------------------------------------------------------------- recording

    fn output_file(&self, app: &str, suffix: &str, ext: &str) -> PathBuf {
        let mut dir = self.s.output_dir();
        let app = naming::sanitize(app);
        if self.s.storage.folder_per_app {
            dir = dir.join(&app);
        }
        let _ = std::fs::create_dir_all(&dir);
        let (y, mo, d, h, mi, se) = urec_win::local_time();
        let stamp = Stamp { year: y, month: mo, day: d, hour: h, minute: mi, second: se };
        naming::unique_path(&dir, &app, &stamp, suffix, ext)
    }

    fn start_recording(&mut self) {
        if self.recording.is_some() || self.ctl.snapshot().finishing {
            return;
        }
        let need = HARD_MIN_FREE;
        if let Err(e) = self.check_folder(&self.s.output_dir(), need, "recordings") {
            self.notice(Level::Error, e, None);
            return;
        }
        let app = match self.resolve_target() {
            Ok((_, _, app)) => app,
            Err(e) => {
                self.notice(Level::Error, e, None);
                return;
            }
        };
        // Instant Replay running: record from its segments, no second encoder.
        if let Some(live) = &self.live {
            if live.kind == LiveKind::Replay {
                let buffer = self.replay_buffer();
                let from = buffer.segments().last().map(|s| s.end).unwrap_or(0.0);
                let dir = self.s.replay_dir().join(format!("rec-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&dir);
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    self.notice(Level::Error, format!("Can't prepare the recording: {e}"), None);
                    return;
                }
                self.ring = Some(RingRecording { dir, from, copied: Vec::new(), last_end: from });
                let output = self.output_file(&app, "", "mp4");
                self.begin_recording(output, app);
                return;
            }
        }
        let output = self.output_file(&app, "", self.s.container().extension());
        let out = Output::File { path: output.to_string_lossy().into(), container: self.s.container() };
        match self.launch(out, LiveKind::File) {
            Ok(()) => self.begin_recording(output, app),
            Err(e) => self.notice(Level::Error, format!("Recording didn't start. {e}"), None),
        }
    }

    fn begin_recording(&mut self, output: PathBuf, app: String) {
        let rec = Recording { started: Instant::now(), output, app };
        self.recording = Some(rec.clone());
        self.low_space_warned = false;
        self.update(|s| s.recording = Some(rec));
    }

    fn stop_recording(&mut self) {
        let Some(rec) = self.recording.take() else { return };
        self.update(|s| {
            s.recording = None;
            s.finishing = true;
        });
        if let Some(mut ring) = self.ring.take() {
            // Wait for the segment that was open when Stop was pressed, then join.
            let deadline = Instant::now() + Duration::from_secs(SEGMENT_SECONDS as u64 + 2);
            let target_end = ring.last_end + 0.01;
            while Instant::now() < deadline {
                self.copy_ring_segments(&mut ring);
                if ring.last_end > target_end && !ring.copied.is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            self.copy_ring_segments(&mut ring);
            let result = self.join_ring(&ring, &rec.output);
            let _ = std::fs::remove_dir_all(&ring.dir);
            self.finish_recording(result.map(|_| Some(rec.output.clone())), &rec);
            return;
        }
        let result = self.stop_live().unwrap_or(Ok(None));
        self.finish_recording(result, &rec);
        if self.s.replay.enabled {
            self.start_replay();
        }
    }

    fn finish_recording(&mut self, result: Result<Option<PathBuf>, String>, rec: &Recording) {
        self.update(|s| s.finishing = false);
        match result {
            Ok(Some(p)) => {
                let secs = rec.started.elapsed().as_secs();
                self.notice(
                    Level::Success,
                    format!("{} recording saved ({}:{:02}).", rec.app, secs / 60, secs % 60),
                    Some(p.clone()),
                );
                self.update(|s| s.last_saved = Some(p));
            }
            Ok(None) => {}
            Err(e) => {
                let kept = rec.output.metadata().map(|m| m.len() > 0).unwrap_or(false);
                if kept {
                    self.notice(
                        Level::Warning,
                        format!("Recording saved, but it may be incomplete: {e}"),
                        Some(rec.output.clone()),
                    );
                } else {
                    self.notice(Level::Error, format!("The recording couldn't be saved: {e}"), None);
                }
            }
        }
    }

    fn copy_ring_segments(&self, ring: &mut RingRecording) {
        let buffer = self.replay_buffer();
        for seg in buffer.segments() {
            if seg.start + 0.001 < ring.from || seg.end <= ring.last_end {
                continue;
            }
            let dst = ring.dir.join(format!("{:06}.ts", ring.copied.len()));
            if std::fs::copy(buffer.dir.join(&seg.file), &dst).is_ok() {
                ring.copied.push(dst);
                ring.last_end = seg.end;
            }
        }
    }

    fn join_ring(&self, ring: &RingRecording, out: &Path) -> Result<(), String> {
        if ring.copied.is_empty() {
            return Err("Nothing was recorded.".into());
        }
        let list: String = ring
            .copied
            .iter()
            .map(|p| format!("file '{}'\n", p.to_string_lossy().replace('\\', "/").replace('\'', "'\\''")))
            .collect();
        let list_path = ring.dir.join("list.txt");
        std::fs::write(&list_path, list).map_err(|e| e.to_string())?;
        let ff = self.ffmpeg.as_deref().ok_or("FFmpeg is missing.")?;
        replay::join_segments(ff, &list_path, out).map_err(|e| e.to_string())
    }

    // -------------------------------------------------------------- instant replay

    fn replay_buffer(&self) -> ReplayBuffer {
        ReplayBuffer::new(self.s.replay_dir(), self.s.replay.seconds, SEGMENT_SECONDS)
    }

    fn set_replay(&mut self, on: bool) {
        if self.s.replay.enabled != on {
            self.s.replay.enabled = on;
            let _ = self.s.save();
        }
        if on {
            if self.live.is_none() {
                self.start_replay();
            } else if self.live.as_ref().is_some_and(|l| l.kind == LiveKind::File) {
                self.notice(Level::Info, "Instant Replay will start when the current recording ends.", None);
            }
        } else {
            if self.recording.is_some() && self.ring.is_some() {
                // Recording depends on the buffer; finish it first.
                self.stop_recording();
            }
            if self.live.as_ref().is_some_and(|l| l.kind == LiveKind::Replay) {
                self.stop_live();
            }
            self.update(|s| s.replay = ReplayState::Off);
        }
    }

    fn start_replay(&mut self) {
        if self.live.is_some() {
            return;
        }
        self.update(|s| s.replay = ReplayState::Starting);
        let buffer = self.replay_buffer();
        let bps = video::bytes_per_second(self.s.video.bitrate_kbps, self.s.audio.bitrate_kbps);
        let need = replay::ring_bytes(self.s.replay.seconds, SEGMENT_SECONDS, bps) + HARD_MIN_FREE;
        if let Err(e) = self
            .check_folder(&buffer.dir, need, "Instant Replay buffer")
            .and_then(|_| buffer.clear().map_err(|e| e.to_string()))
        {
            self.notice(Level::Error, format!("Instant Replay couldn't start. {e}"), None);
            self.update(|s| s.replay = ReplayState::Error(e));
            return;
        }
        let output = Output::ReplayRing {
            dir: buffer.dir.to_string_lossy().into(),
            segment_seconds: SEGMENT_SECONDS,
            wrap: buffer.wrap(),
        };
        match self.launch(output, LiveKind::Replay) {
            Ok(()) => self.update(|s| s.replay = ReplayState::On),
            Err(e) => {
                self.notice(Level::Error, format!("Instant Replay couldn't start. {e}"), None);
                self.update(|s| s.replay = ReplayState::Error(e));
            }
        }
    }

    fn save_replay(&mut self) {
        if !self.live.as_ref().is_some_and(|l| l.kind == LiveKind::Replay) {
            self.notice(Level::Info, "Instant Replay is off. Turn it on to save the last moments.", None);
            return;
        }
        let Some(ff) = self.ffmpeg.clone() else { return };
        if let Err(e) = self.check_folder(&self.s.output_dir(), HARD_MIN_FREE, "recordings") {
            self.notice(Level::Error, e, None);
            return;
        }
        let out = self.output_file(&foreground_app(), " Replay", "mp4");
        let buffer = self.replay_buffer();
        let seconds = self.s.replay.seconds;
        let ctl = self.ctl.clone();
        let sound = self.s.general.sound_on_save;
        // Joining takes a moment; don't block hotkeys or the recording loop.
        std::thread::spawn(move || {
            let result = buffer.save(&ff, seconds, &out, true);
            let worker_notice = |level: Level, text: String, file: Option<PathBuf>| {
                if let Ok(mut s) = ctl.status.lock() {
                    s.notices.push_front(Notice { level, text, at: Instant::now(), file: file.clone() });
                    s.notices.truncate(6);
                    if let Some(f) = file {
                        s.last_saved = Some(f);
                    }
                }
                ctl.repaint();
            };
            match result {
                Ok(p) => {
                    if sound {
                        urec_win::play_ding();
                    }
                    log(&format!("replay saved: {}", p.display()));
                    worker_notice(Level::Success, format!("Instant Replay saved ({}).", format_secs(seconds)), Some(p));
                }
                Err(e) => worker_notice(Level::Error, format!("Instant Replay couldn't be saved: {e}"), None),
            }
        });
    }

    // -------------------------------------------------------------- periodic work

    fn tick(&mut self) {
        let mut crashed = None;
        if let Some(live) = self.live.as_mut() {
            if !live.session.running() {
                crashed = Some(live.session.process.log_tail(600));
            }
        }
        if let Some(log_tail) = crashed {
            self.on_crash(&log_tail);
        }
        if let Some(mut ring) = self.ring.take() {
            self.copy_ring_segments(&mut ring);
            self.ring = Some(ring);
        }
        if let Some(live) = &self.live {
            let stats = live.session.stats();
            let (levels, states, muted) = match &live.audio {
                Some(f) => {
                    let (a, b) = f.states();
                    (f.levels(), (Some(a), Some(b)), Some(f.mic_muted()))
                }
                None => ((0.0, 0.0), (None, None), None),
            };
            let lagging = stats.lagging();
            if lagging {
                let since = *self.lag_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_secs(10) && !self.lag_warned {
                    self.lag_warned = true;
                    self.on_lag();
                }
            } else {
                self.lag_since = None;
            }
            self.update(|s| {
                s.stats = stats;
                s.lagging = lagging;
                // Peak meter with a gentle fall-off.
                s.levels = (levels.0.max(s.levels.0 * 0.7), levels.1.max(s.levels.1 * 0.7));
                s.audio = states;
                if let Some(m) = muted {
                    s.mic_muted = m;
                }
            });
        }
        if self.last_disk_check.elapsed() > Duration::from_secs(10) {
            self.last_disk_check = Instant::now();
            self.check_disk();
        }
        self.update_overlay();
    }

    fn update_overlay(&self) {
        let st = self.ctl.snapshot();
        let replay_on = st.replay == ReplayState::On;
        let visible = self.s.appearance.overlay && st.overlay_visible && (self.recording.is_some() || replay_on);
        let mut o =
            urec_win::OverlayState { visible, corner: self.s.appearance.overlay_corner.clone(), ..Default::default() };
        if visible {
            if let Some(m) = urec_win::monitors().into_iter().find(|m| m.primary) {
                o.monitor = (m.bounds.x, m.bounds.y, m.bounds.width, m.bounds.height);
            }
            if let Some(r) = &self.recording {
                let s = r.started.elapsed().as_secs();
                o.dot = (229, 72, 77);
                o.text = format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60);
            } else {
                o.dot = (217, 164, 65);
                o.text = "Replay on".into();
            }
            let mut extra = Vec::new();
            if self.s.audio.mic && st.mic_muted {
                extra.push("Mic off".to_string());
            }
            if st.stats.drop_frames > 0 {
                extra.push(format!("{} dropped", st.stats.drop_frames));
            }
            o.extra = extra.join(", ");
        }
        self.overlay.set(o);
    }

    fn on_crash(&mut self, log_tail: &str) {
        let kind_replay = self.live.as_ref().is_some_and(|l| l.kind == LiveKind::Replay);
        log(&format!("ffmpeg exited unexpectedly: {log_tail}"));
        let _ = self.stop_live();
        if let Some(rec) = self.recording.take() {
            self.update(|s| s.recording = None);
            if self.ring.take().is_some() {
                self.notice(Level::Error, "Recording stopped because capture failed.", None);
            } else {
                let kept = rec.output.metadata().map(|m| m.len() > 0).unwrap_or(false);
                self.notice(
                    if kept { Level::Warning } else { Level::Error },
                    format!(
                        "Recording stopped unexpectedly ({}).{}",
                        last_line(log_tail),
                        if kept { " What was recorded so far was saved." } else { "" }
                    ),
                    kept.then(|| rec.output.clone()),
                );
            }
        }
        if kind_replay && self.s.replay.enabled && self.crash_restarts < 3 {
            self.crash_restarts += 1;
            self.notice(Level::Warning, "Instant Replay stopped unexpectedly and was restarted.", None);
            self.start_replay();
        } else if kind_replay {
            self.update(|s| s.replay = ReplayState::Error(last_line(log_tail)));
        }
    }

    fn on_lag(&mut self) {
        let replay_only = self.recording.is_none() && self.live.as_ref().is_some_and(|l| l.kind == LiveKind::Replay);
        if self.s.general.auto_adjust && replay_only && self.s.video.fps > 30 {
            let old = self.s.video.fps;
            self.s.video.fps = 30;
            let _ = self.s.save();
            self.notice(
                Level::Warning,
                format!("Your PC couldn't keep up at {old} FPS, so Instant Replay now records at 30 FPS. You can change this in Settings › Video."),
                None,
            );
            self.stop_live();
            self.start_replay();
        } else {
            self.notice(
                Level::Warning,
                "Recording is falling behind: frames are being dropped. Try a lower resolution, frame rate or the Low-end preset.",
                None,
            );
        }
    }

    fn check_disk(&mut self) {
        let free = urec_win::disk_space(&self.s.output_dir()).map(|(f, _)| f);
        self.update(|s| s.disk_free = free);
        let Some(free) = free else { return };
        if self.recording.is_some() {
            if free < HARD_MIN_FREE {
                self.notice(
                    Level::Error,
                    "The drive is almost full, so recording was stopped to protect your system.",
                    None,
                );
                self.stop_recording();
            } else if free < self.s.storage.low_space_gb as u64 * 1_000_000_000 && !self.low_space_warned {
                self.low_space_warned = true;
                self.notice(Level::Warning, format!("Low disk space: {} left.", video::format_bytes(free)), None);
            }
        }
    }

    fn shutdown(&mut self) {
        if self.recording.is_some() {
            self.stop_recording();
        }
        let _ = self.stop_live();
        self.hotkeys.stop();
    }
}

// ------------------------------------------------------------------ helpers

fn foreground_app() -> String {
    urec_win::foreground_window().map(|w| w.app_name()).unwrap_or_else(|| "Desktop".into())
}

fn last_line(s: &str) -> String {
    s.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().chars().take(160).collect()
}

pub fn format_secs(s: u32) -> String {
    if s < 60 {
        format!("{s} s")
    } else if s.is_multiple_of(60) {
        format!("{} min", s / 60)
    } else {
        format!("{}:{:02} min", s / 60, s % 60)
    }
}

pub fn preset_id(p: Preset) -> &'static str {
    match p {
        Preset::LowEnd => "low",
        Preset::Balanced => "balanced",
        Preset::High => "high",
        Preset::VeryHigh => "very_high",
        Preset::Custom => "custom",
    }
}

pub fn explain_audio_failure(reason: &str) -> String {
    let r = reason.to_ascii_lowercase();
    if r.contains("access is denied") {
        "Windows is blocking microphone access. Turn on Settings › Privacy & security › Microphone › Let desktop apps access your microphone.".into()
    } else if r.contains("element not found") || r.contains("not found") {
        "the device wasn't found. It may be unplugged or disabled.".into()
    } else {
        reason.to_string()
    }
}

/// Append to %LOCALAPPDATA%\uRec\logs\urec.log (rotated at 2 MB). Never logs recording content.
pub fn log(line: &str) {
    let dir = crate::settings::local_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("urec.log");
    if path.metadata().map(|m| m.len() > 2_000_000).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("urec.old.log"));
    }
    let (y, mo, d, h, mi, s) = urec_win::local_time();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        use std::io::Write;
        let _ = writeln!(f, "{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} {line}");
    }
}
