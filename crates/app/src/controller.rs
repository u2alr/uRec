//! The recording controller. Runs on its own thread and owns every capture session,
//! the audio feed and the global hotkeys, so recording keeps working when the window is
//! hidden. The UI sends `Command`s and reads a `Status` snapshot.

use crate::settings::Settings;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use urec_engine::encoder::{Backend, Encoder};
use urec_engine::hotkey::Hotkey;
use urec_engine::naming::{self, Stamp};
use urec_engine::pipeline::{AudioInput, CaptureMethod, CaptureTarget, Output, Rect};
use urec_engine::probe::{self, ProbeResult};
use urec_engine::process::{locate_ffmpeg, Stats};
use urec_engine::replay::{self, ReplayBuffer, SavedClip};
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
    /// First-run setup: record 5 seconds with the current settings and check the result.
    TestRecording,
    Shutdown,
    /// Push-to-talk key pressed (true) or released (false).
    PushToTalk(bool),
    /// Open or close the overlay panel (its hotkey).
    TogglePanel,
    /// A button pressed in the overlay panel.
    Panel(urec_win::panel::PanelAction),
    /// From the save worker: a queued replay save has started / finished.
    ReplaySaveStarted(u64),
    ReplaySaveFinished(u64, Result<SavedClip, String>),
}

/// One line of the setup test recording's report. `ok: None` = not tested (e.g. turned off).
#[derive(Clone, Debug, PartialEq)]
pub struct TestCheck {
    pub name: &'static str,
    pub ok: Option<bool>,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TestReport {
    pub running: bool,
    pub stage: String,
    pub checks: Vec<TestCheck>,
    pub file: Option<PathBuf>,
}

impl TestReport {
    pub fn passed(&self) -> bool {
        !self.running && !self.checks.is_empty() && self.checks.iter().all(|c| c.ok != Some(false))
    }
}

/// Outcome of one Instant Replay save, for the UI and the overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplaySaveEvent {
    pub id: u64,
    pub result: Result<SavedClip, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

/// Something a notice offers to do. Handled by the UI (it owns the settings being edited).
#[derive(Clone, Debug, PartialEq)]
pub enum NoticeAction {
    /// The hardware encoder failed: switch to the CPU encoder and try again.
    UseCpuEncoder { retry_replay: bool },
}

#[derive(Clone, Debug)]
pub struct Notice {
    pub level: Level,
    pub text: String,
    pub at: Instant,
    pub file: Option<PathBuf>,
    pub action: Option<NoticeAction>,
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
    /// Instant Replay saves queued or running (at most 2).
    pub replay_saving: usize,
    pub last_replay_save: Option<ReplaySaveEvent>,
    pub test: Option<TestReport>,
    /// The controller changed a setting itself (hotkey, overlay panel, auto-adjust): the UI
    /// takes this copy so it doesn't write the old value back.
    pub settings_update: Option<Box<Settings>>,
    /// Page the UI should show (e.g. "settings" from the overlay panel).
    pub open_page: Option<String>,
}

impl Status {
    /// Something is being captured or written (no updates or re-probing now).
    pub fn busy(&self) -> bool {
        self.recording.is_some()
            || self.finishing
            || self.replay == ReplayState::On
            || self.replay_saving > 0
            || self.test.as_ref().is_some_and(|t| t.running)
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
    /// True while a recording is being written; thumbnail generation waits for it to clear.
    /// (Instant Replay alone doesn't count, so thumbnails still appear while it buffers.)
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

/// What the user should see about a replay save.
#[derive(Clone, Debug)]
pub enum SaveFeedback {
    Saving,
    Saved(SavedClip),
    Failed(String),
}

/// Recording that piggybacks on the Instant Replay ring: closed segments are copied as
/// they appear and joined when recording stops. Nothing is encoded twice.
struct RingRecording {
    dir: PathBuf,
    /// Stream time (seconds) from which segments belong to this recording.
    from: f64,
    copied: Vec<PathBuf>,
    last_end: f64,
}

/// One queued Instant Replay save. Everything needed is captured when the hotkey is pressed.
struct SaveJob {
    id: u64,
    ffmpeg: PathBuf,
    buffer_dir: PathBuf,
    replay_seconds: u32,
    save_seconds: u32,
    out: PathBuf,
}

/// Saves run one at a time on their own thread, so a burst of hotkey presses can't start
/// competing FFmpeg joins, and hotkeys/recording never wait for a save.
struct Saver {
    tx: Sender<SaveJob>,
    /// Queued + running saves.
    pending: Arc<AtomicUsize>,
    next_id: u64,
}

/// At most one save running and one waiting; further presses are folded into the waiting one
/// (it starts later, so it includes those moments too).
const MAX_PENDING_SAVES: usize = 2;

impl Saver {
    fn start(ctl: Controller) -> Saver {
        let (tx, rx) = channel::<SaveJob>();
        let pending = Arc::new(AtomicUsize::new(0));
        let p2 = pending.clone();
        let _ = std::thread::Builder::new().name("replay-save".into()).spawn(move || {
            for job in rx {
                ctl.send(Command::ReplaySaveStarted(job.id));
                let t0 = Instant::now();
                let buffer = ReplayBuffer::new(job.buffer_dir, job.replay_seconds, SEGMENT_SECONDS);
                let result = buffer.save(&job.ffmpeg, job.save_seconds, &job.out, true).map_err(|e| e.to_string());
                match &result {
                    Ok(c) => log(&format!(
                        "replay save #{} ok in {} ms: {} ({:.1} s, {} bytes)",
                        job.id,
                        t0.elapsed().as_millis(),
                        c.path.display(),
                        c.seconds,
                        c.bytes
                    )),
                    Err(e) => {
                        log(&format!("replay save #{} failed after {} ms: {e}", job.id, t0.elapsed().as_millis()))
                    }
                }
                p2.fetch_sub(1, Ordering::SeqCst);
                ctl.send(Command::ReplaySaveFinished(job.id, result));
            }
        });
        Saver { tx, pending, next_id: 1 }
    }

    fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    /// Wait until no save is queued or running (before the buffer is cleared or on exit).
    fn wait_idle(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        while self.pending() > 0 {
            if start.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        true
    }
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
    saver: Saver,
    /// Watches the push-to-talk key while something is being captured.
    ptt: Option<urec_win::PushToTalk>,
    live: Option<Live>,
    ring: Option<RingRecording>,
    recording: Option<Recording>,
    hotkeys: urec_win::Hotkeys,
    overlay: urec_win::Overlay,
    panel: urec_win::Panel,
    /// Primary monitor rectangle for the overlay, refreshed with the disk check (not every tick).
    overlay_monitor: (i32, i32, u32, u32),
    lag_since: Option<Instant>,
    lag_warned: bool,
    /// Windows Graphics Capture stalled on this PC: prefer Desktop Duplication (remembered in a file).
    prefer_dda: bool,
    stall: urec_engine::health::StallWatch,
    stall_clock: Instant,
    /// None until the first check (see `devices_at` in ui/mod.rs for why not `now() - 60 s`).
    last_disk_check: Option<Instant>,
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
                4 => Command::ToggleOverlay,
                _ => Command::TogglePanel,
            };
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(cmd);
            }
        });
        let saver = Saver::start(ctl.clone());
        let panel_tx = ctl.tx.clone();
        let panel = urec_win::Panel::new(move |a| {
            if let Ok(tx) = panel_tx.lock() {
                let _ = tx.send(Command::Panel(a));
            }
        });
        Worker {
            panel,
            ctl,
            s,
            rx,
            ffmpeg: None,
            saver,
            ptt: None,
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
            overlay_monitor: (0, 0, 1920, 1080),
            lag_since: None,
            lag_warned: false,
            prefer_dda: std::fs::read_to_string(crate::settings::local_dir().join("capture-hint.txt"))
                .is_ok_and(|t| t.trim() == "dda"),
            stall: Default::default(),
            stall_clock: Instant::now(),
            last_disk_check: None,
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

    /// Update the snapshot without waking the UI (it refreshes itself on its own schedule).
    fn update_quiet(&self, f: impl FnOnce(&mut Status)) {
        if let Ok(mut s) = self.ctl.status.lock() {
            f(&mut s);
        }
    }

    fn notice(&self, level: Level, text: impl Into<String>, file: Option<PathBuf>) {
        let text = text.into();
        log(&format!("{level:?}: {text}"));
        self.update(|s| {
            s.notices.push_front(Notice { level, text, at: Instant::now(), file, action: None });
            s.notices.truncate(6);
        });
    }

    fn run(mut self) {
        self.init();
        loop {
            // Idle: wake once a second. Capturing: 4×/s for stats and meters (1×/s in performance mode).
            let tick = if self.live.is_none() || self.s.general.performance_mode { 1000 } else { 250 };
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
        self.remove_leftovers();
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

    /// Clean up after a crash or power loss: staging copies and unfinished clips. Only uRec's
    /// own temporary names are removed; finished recordings are never touched.
    fn remove_leftovers(&self) {
        let rdir = self.s.replay_dir();
        replay::remove_stale_stages(&rdir);
        if let Ok(rd) = std::fs::read_dir(&rdir) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("rec-") && e.path().is_dir() {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        let n = replay::remove_temp_outputs(&self.s.output_dir());
        if n > 0 {
            log(&format!("removed {n} unfinished clip(s) from an interrupted save"));
        }
    }

    fn run_probe(&mut self) {
        let Some(ff) = self.ffmpeg.clone() else { return };
        self.update(|s| s.probing = true);
        // Probe with standard (VBR) settings: this answers "does this encoder exist and work".
        // A rejected constant-quality option must not hide a working hardware encoder and push
        // "Automatic" onto the CPU; that would show up as a start error with a clear choice.
        let mut v = self.s.video_settings();
        v.rate_control = urec_engine::video::RateControl::Vbr;
        let results = probe::probe_encoders(&ff, &Codec::ALL, &v);
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
            ("Open overlay panel", &h.panel),
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
            Command::TogglePanel => {
                let model = self.panel_model();
                let reduced = self.reduced_motion();
                self.panel.toggle(model, &self.s.appearance.panel_position, reduced);
            }
            Command::Panel(action) => self.panel_action(action),
            Command::ToggleOverlay => {
                self.update(|s| s.overlay_visible = !s.overlay_visible);
                self.update_overlay();
            }
            Command::Apply(new) => self.apply(*new),
            Command::TestRecording => self.test_recording(),
            Command::Reprobe => {
                if !self.ctl.snapshot().busy() {
                    self.run_probe();
                }
            }
            Command::Shutdown => {}
            Command::ReplaySaveStarted(id) => {
                log(&format!("replay save #{id} started"));
                let n = self.saver.pending();
                self.update(|s| s.replay_saving = n);
                self.save_feedback(SaveFeedback::Saving);
            }
            Command::ReplaySaveFinished(id, result) => self.replay_saved(id, result),
            Command::PushToTalk(down) => {
                if self.push_to_talk() {
                    if let Some(feed) = self.live.as_ref().and_then(|l| l.audio.as_ref()) {
                        feed.set_mic_muted(!down);
                    }
                    self.update(|s| s.mic_muted = !down);
                }
            }
        }
    }

    fn apply(&mut self, new: Settings) {
        let replay_relevant = new.video != self.s.video
            || new.audio != self.s.audio
            || new.capture != self.s.capture
            || new.replay.seconds != self.s.replay.seconds
            || new.replay.fps_cap != self.s.replay.fps_cap
            || new.replay.buffer_dir != self.s.replay.buffer_dir;
        let hotkeys_changed = new.hotkeys != self.s.hotkeys;
        let replay_toggled = new.replay.enabled != self.s.replay.enabled;
        let volumes_changed =
            new.audio.system_volume != self.s.audio.system_volume || new.audio.mic_volume != self.s.audio.mic_volume;
        let ptt_changed = (new.audio.mic, &new.audio.mic_mode, &new.hotkeys.push_to_talk)
            != (self.s.audio.mic, &self.s.audio.mic_mode, &self.s.hotkeys.push_to_talk);
        self.s = new;
        if hotkeys_changed {
            self.register_hotkeys();
        }
        if ptt_changed {
            if !self.push_to_talk() {
                // Leaving push-to-talk: back to an open microphone.
                if let Some(feed) = self.live.as_ref().and_then(|l| l.audio.as_ref()) {
                    feed.set_mic_muted(false);
                }
                self.update(|s| s.mic_muted = false);
            }
            self.update_push_to_talk();
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

    fn push_to_talk(&self) -> bool {
        self.s.audio.mic && self.s.audio.mic_mode == "push_to_talk"
    }

    /// Start or stop watching the push-to-talk key. It's watched only while capturing; outside
    /// push-to-talk mode the microphone follows the mute hotkey as before.
    fn update_push_to_talk(&mut self) {
        self.ptt = None;
        if !self.push_to_talk() {
            return;
        }
        // Silent until the key is held.
        if let Some(feed) = self.live.as_ref().and_then(|l| l.audio.as_ref()) {
            feed.set_mic_muted(true);
        }
        self.update(|s| s.mic_muted = true);
        if self.live.is_none() {
            return;
        }
        match Hotkey::parse(&self.s.hotkeys.push_to_talk) {
            Some(key) => {
                let tx = self.ctl.tx.clone();
                self.ptt = Some(urec_win::PushToTalk::start(key, move |down| {
                    if let Ok(tx) = tx.lock() {
                        let _ = tx.send(Command::PushToTalk(down));
                    }
                }));
            }
            None => self.notice(
                Level::Warning,
                "Push-to-talk is on but has no key. Choose one in Settings › Hotkeys; the microphone stays silent until then.",
                None,
            ),
        }
    }

    fn toggle_mute(&mut self) {
        if self.push_to_talk() {
            self.notice(
                Level::Info,
                format!("Push-to-talk is on: hold {} to talk.", self.s.hotkeys.push_to_talk.replace('+', " + ")),
                None,
            );
            return;
        }
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
        // Windows Graphics Capture stalled here before: use Desktop Duplication first (screens only).
        if self.prefer_dda && c.method == "auto" && !matches!(target, CaptureTarget::Window { .. }) {
            if let Some(i) = methods.iter().position(|m| *m == CaptureMethod::Dda) {
                let dda = methods.remove(i);
                methods.insert(0, dda);
            }
        }
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
        let (w, h, mut fps) = video::validate(&v, sw, sh, refresh).map_err(|e| e.to_string())?;
        // Instant Replay can run at a lower frame rate than recordings to spare the graphics card.
        if kind == LiveKind::Replay && self.s.replay.fps_cap >= 15 {
            fps = fps.min(self.s.replay.fps_cap);
        }
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
        let muted = self.push_to_talk() || self.ctl.snapshot().mic_muted;
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
                    format!("{name} isn't being recorded. {}", explain_audio_failure(reason)),
                    None,
                );
            }
        }
        self.live = Some(Live { session, audio: feed_slot, kind });
        self.update_push_to_talk();
        self.lag_since = None;
        self.lag_warned = false;
        self.stall.reset();
        Ok(())
    }

    fn stop_live(&mut self) -> Option<Result<Option<PathBuf>, String>> {
        let live = self.live.take()?;
        self.ptt = None;
        let result = live.session.stop(Duration::from_secs(20));
        drop(live.audio);
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
            Err(e) => self.start_failed(format!("Recording didn't start. {e}"), false),
        }
    }

    fn begin_recording(&mut self, output: PathBuf, app: String) {
        self.ctl.capturing.store(true, Ordering::Relaxed);
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
            self.finish_recording(result.map(Some), &rec);
            return;
        }
        let result = self.stop_live().unwrap_or(Ok(None));
        self.finish_recording(result, &rec);
        if self.s.replay.enabled {
            self.start_replay();
        }
    }

    fn finish_recording(&mut self, result: Result<Option<PathBuf>, String>, rec: &Recording) {
        self.ctl.capturing.store(false, Ordering::Relaxed);
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

    fn join_ring(&self, ring: &RingRecording, out: &Path) -> Result<PathBuf, String> {
        if ring.copied.is_empty() {
            return Err("Nothing was recorded.".into());
        }
        let list = replay::concat_list(&ring.copied);
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
            self.publish_settings();
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
        // Starting clears the buffer: let queued saves finish reading it first.
        if !self.saver.wait_idle(Duration::from_secs(30)) {
            log("start_replay: a save is still running after 30 s; continuing");
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
                self.start_failed(format!("Instant Replay couldn't start. {e}"), true);
                self.update(|s| s.replay = ReplayState::Error(e));
            }
        }
    }

    /// A capture didn't start. If a hardware encoder was in use and the CPU encoder works,
    /// offer to switch, saying plainly that it costs CPU. Never switches by itself.
    fn start_failed(&mut self, text: String, retry_replay: bool) {
        let hw = self.pick_encoder().is_ok_and(|e| e.backend.is_hardware());
        let cpu_ok = self
            .ctl
            .snapshot()
            .probe
            .iter()
            .any(|r| r.ok && r.encoder.codec == self.s.codec() && !r.encoder.backend.is_hardware());
        log(&text);
        if hw && cpu_ok {
            let text = format!(
                "{text} The CPU encoder works on this PC but uses much more processor time, which can lower game frame rates."
            );
            self.update(|s| {
                s.notices.push_front(Notice {
                    level: Level::Error,
                    text,
                    at: Instant::now(),
                    file: None,
                    action: Some(NoticeAction::UseCpuEncoder { retry_replay }),
                });
                s.notices.truncate(6);
            });
        } else {
            self.notice(Level::Error, text, None);
        }
    }

    fn save_replay(&mut self) {
        if !self.live.as_ref().is_some_and(|l| l.kind == LiveKind::Replay) {
            let starting = self.ctl.snapshot().replay == ReplayState::Starting;
            let text = if starting {
                "Instant Replay is still starting. Try again in a moment."
            } else {
                "Instant Replay is off. Turn it on to save the last moments."
            };
            self.notice(Level::Warning, text, None);
            self.save_feedback(SaveFeedback::Failed(text.into()));
            return;
        }
        let Some(ff) = self.ffmpeg.clone() else { return };
        let pending = self.saver.pending();
        if pending >= MAX_PENDING_SAVES {
            // The waiting save starts after the running one and covers this moment too.
            log("replay save: press folded into the save already waiting");
            self.notice(Level::Info, "Already saving. This moment is included in the next clip.", None);
            return;
        }
        let seconds = self.s.replay.seconds;
        let bps = video::bytes_per_second(self.s.video.bitrate_kbps, self.s.audio.bitrate_kbps);
        let need = bps * seconds as u64 * 12 / 10 + 256_000_000;
        if let Err(e) = self.check_folder(&self.s.output_dir(), need, "clip") {
            self.notice(Level::Error, format!("Instant Replay couldn't be saved. {e}"), None);
            self.save_feedback(SaveFeedback::Failed(e));
            return;
        }
        let job = SaveJob {
            id: self.saver.next_id,
            ffmpeg: ff,
            buffer_dir: self.s.replay_dir(),
            replay_seconds: seconds,
            save_seconds: seconds,
            out: self.output_file(&foreground_app(), " Replay", "mp4"),
        };
        self.saver.next_id += 1;
        self.saver.pending.fetch_add(1, Ordering::SeqCst);
        log(&format!("replay save #{} queued ({} pending before)", job.id, pending));
        if self.saver.tx.send(job).is_err() {
            self.saver.pending.fetch_sub(1, Ordering::SeqCst);
            self.notice(Level::Error, "Instant Replay couldn't be saved: the save worker stopped.", None);
            self.save_feedback(SaveFeedback::Failed("save worker stopped".into()));
            return;
        }
        self.update(|s| s.replay_saving = pending + 1);
    }

    fn replay_saved(&mut self, id: u64, result: Result<SavedClip, String>) {
        let n = self.saver.pending();
        self.update(|s| {
            s.replay_saving = n;
            s.last_replay_save = Some(ReplaySaveEvent { id, result: result.clone() });
        });
        match result {
            Ok(clip) => {
                if self.s.general.sound_on_save {
                    urec_win::play_ding();
                }
                self.notice(
                    Level::Success,
                    format!("Instant Replay saved ({}).", format_secs(clip.seconds.round() as u32)),
                    Some(clip.path.clone()),
                );
                let p = clip.path.clone();
                self.update(|s| s.last_saved = Some(p));
                self.save_feedback(SaveFeedback::Saved(clip));
            }
            Err(e) => {
                if self.s.general.sound_on_save {
                    urec_win::play_error();
                }
                self.notice(Level::Error, format!("Instant Replay couldn't be saved: {e}"), None);
                self.save_feedback(SaveFeedback::Failed(e));
            }
        }
    }

    /// Visual feedback for replay saves: the overlay pill, or a Windows notification when the
    /// overlay is turned off. The in-app notice is always added separately.
    fn save_feedback(&mut self, fb: SaveFeedback) {
        let overlay_on = self.s.appearance.overlay && self.ctl.snapshot().overlay_visible;
        let reduced = self.reduced_motion();
        let ev = match &fb {
            SaveFeedback::Saving => urec_win::OverlayEvent::Saving,
            SaveFeedback::Saved(_) => urec_win::OverlayEvent::Saved,
            SaveFeedback::Failed(_) => urec_win::OverlayEvent::Failed,
        };
        // The overlay is the user's choice: never shown just to deliver this message.
        if overlay_on && self.overlay.notify(ev, reduced) {
            return;
        }
        if !self.s.general.notifications {
            return;
        }
        let xml = match &fb {
            SaveFeedback::Saving => return,
            SaveFeedback::Saved(clip) => {
                let name = clip.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                urec_win::notify::toast_xml(
                    urec_win::pill::SUCCESS_TEXT,
                    &format!("{} · {name}", format_secs(clip.seconds.round() as u32)),
                    Some(&clip.path),
                )
            }
            SaveFeedback::Failed(e) => {
                let mut e = e.clone();
                if e.chars().count() > 140 {
                    e = e.chars().take(140).collect::<String>() + "…";
                }
                urec_win::notify::toast_xml(urec_win::pill::FAILURE_TEXT, &e, None)
            }
        };
        if let Err(e) = urec_win::show_toast(&xml) {
            log(&format!("notification not shown ({e}); the result is in the app"));
        }
    }

    // -------------------------------------------------------------- setup test

    fn test_report(&self, f: impl FnOnce(&mut TestReport)) {
        self.update(|s| f(s.test.get_or_insert_with(TestReport::default)));
    }

    /// Record 5 seconds through the real pipeline with the current settings, then check every
    /// stage and report exactly what passed. Nothing is assumed: untested parts say so.
    fn test_recording(&mut self) {
        let check = |name: &'static str, ok: Option<bool>, detail: String| TestCheck { name, ok, detail };
        self.update(|s| s.test = Some(TestReport { running: true, stage: "Starting…".into(), ..Default::default() }));
        if self.recording.is_some() {
            self.test_report(|t| {
                t.running = false;
                t.checks.push(check("Ready to test", Some(false), "Stop the current recording first.".into()));
            });
            return;
        }
        let Some(ff) = self.ffmpeg.clone() else {
            self.test_report(|t| {
                t.running = false;
                t.checks.push(check("FFmpeg", Some(false), "FFmpeg is missing. Reinstall uRec.".into()));
            });
            return;
        };
        // Instant Replay uses the same capture; pause it for the test.
        let replay_was_on = self.live.as_ref().is_some_and(|l| l.kind == LiveKind::Replay);
        if replay_was_on {
            self.saver.wait_idle(Duration::from_secs(30));
            self.stop_live();
        }
        let dir = self.s.replay_dir();
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("setup-test.mp4");
        let _ = std::fs::remove_file(&out);
        let output =
            Output::File { path: out.to_string_lossy().into(), container: urec_engine::pipeline::Container::Mp4 };
        self.test_report(|t| t.stage = "Recording 5 seconds…".into());
        let started = self.launch(output, LiveKind::File);
        let restore = |w: &mut Worker| {
            if replay_was_on && w.s.replay.enabled {
                w.start_replay();
            }
        };
        if let Err(e) = started {
            self.test_report(|t| {
                t.running = false;
                t.checks.push(check("Capture and encoder start", Some(false), e.clone()));
            });
            log(&format!("setup test: start failed: {e}"));
            restore(self);
            return;
        }
        let cap = self.ctl.snapshot().capture;
        if let Some(c) = &cap {
            let detail = format!(
                "{}, {} via {}, {}×{} at {} FPS{}",
                c.encoder.backend.label(),
                c.encoder.codec,
                c.method.label(),
                c.width,
                c.height,
                c.fps,
                if c.gpu_path { ", frames stay on the graphics card" } else { "" }
            );
            self.test_report(|t| t.checks.push(check("Capture and encoder start", Some(true), detail)));
        }
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(5) {
            if !self.live.as_mut().is_some_and(|l| l.session.running()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let crashed = !self.live.as_mut().is_some_and(|l| l.session.running());
        let audio_states = self.live.as_ref().and_then(|l| l.audio.as_ref()).map(|f| f.states());
        let stats = self.live.as_ref().map(|l| l.session.stats()).unwrap_or_default();
        let audio_settings = self.s.audio_settings();
        let mut expected_audio_tracks = 0usize;
        let mut audio_checks = Vec::new();
        for (name, wanted, state) in [
            ("Game and desktop sound", audio_settings.system, audio_states.as_ref().map(|s| s.0.clone())),
            ("Microphone", audio_settings.mic, audio_states.as_ref().map(|s| s.1.clone())),
        ] {
            let c = match (wanted, state) {
                (false, _) => check(name, None, "Turned off in Settings › Audio.".into()),
                (true, Some(SourceState::Active { device })) => {
                    expected_audio_tracks += 1;
                    check(name, Some(true), format!("Captured from {device}."))
                }
                (true, Some(SourceState::Failed { reason })) => {
                    check(name, Some(false), explain_audio_failure(&reason))
                }
                (true, Some(SourceState::Lost { device })) => {
                    check(name, Some(false), format!("{device} disappeared during the test."))
                }
                (true, _) => check(name, Some(false), "Didn't start.".into()),
            };
            audio_checks.push(c);
        }
        if audio_settings.layout == urec_win::TrackLayout::Mixed {
            expected_audio_tracks = expected_audio_tracks.min(1);
        }
        self.test_report(|t| t.stage = "Finishing the file…".into());
        let result = self.stop_live().unwrap_or(Ok(None));
        self.test_report(|t| t.checks.extend(audio_checks));
        if crashed {
            self.test_report(|t| {
                t.checks.push(check("Recording ran for 5 seconds", Some(false), "Capture stopped early.".into()))
            });
        } else {
            let dropped =
                if stats.drop_frames > 0 { format!(", {} frames dropped", stats.drop_frames) } else { String::new() };
            self.test_report(|t| {
                t.checks.push(check(
                    "Recording ran for 5 seconds",
                    Some(true),
                    format!("{} frames encoded{dropped}", stats.frame),
                ))
            });
        }
        let finalized = match (&result, &urec_engine::mp4check::check(&out)) {
            (Ok(_), Ok(info)) => {
                let d = format!(
                    "{} written{}",
                    video::format_bytes(info.bytes),
                    if info.fragmented { ", crash-safe MP4" } else { "" }
                );
                self.test_report(|t| t.checks.push(check("File saved and finalized", Some(true), d)));
                true
            }
            (Err(e), _) | (Ok(_), Err(e)) => {
                let e = e.clone();
                self.test_report(|t| t.checks.push(check("File saved and finalized", Some(false), e)));
                false
            }
        };
        if finalized {
            self.test_report(|t| t.stage = "Playing it back…".into());
            match urec_engine::verify::decode_file(&ff, &out) {
                Ok(r) => {
                    let video_ok = r.video_codec.is_some() && r.frames > 0 && r.errors.is_empty();
                    let detail = if video_ok {
                        format!(
                            "{} frames of {} {}×{} decoded without errors ({:.1} s)",
                            r.frames,
                            r.video_codec.clone().unwrap_or_default(),
                            r.width,
                            r.height,
                            r.duration
                        )
                    } else if !r.errors.is_empty() {
                        format!("Decoding errors: {}", r.errors.join(" "))
                    } else {
                        "No video frames could be decoded.".into()
                    };
                    let audio_ok = r.audio_streams == expected_audio_tracks;
                    let adetail =
                        format!("{} audio track(s) in the file, {} expected", r.audio_streams, expected_audio_tracks);
                    self.test_report(|t| {
                        t.checks.push(check("Video plays back", Some(video_ok), detail));
                        if expected_audio_tracks > 0 || r.audio_streams > 0 {
                            t.checks.push(check("Audio tracks in the file", Some(audio_ok), adetail));
                        }
                    });
                }
                Err(e) => self.test_report(|t| t.checks.push(check("Video plays back", Some(false), e))),
            }
        }
        let file = out.exists().then(|| out.clone());
        self.test_report(|t| {
            t.running = false;
            t.stage = String::new();
            t.file = file;
        });
        let rep = self.ctl.snapshot().test.unwrap_or_default();
        for c in &rep.checks {
            log(&format!("setup test: {} = {:?} ({})", c.name, c.ok, c.detail));
        }
        restore(self);
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
            let stalled = live.session.plan.method == CaptureMethod::Wgc
                && !matches!(live.session.plan.target, CaptureTarget::Window { .. })
                && !self.prefer_dda
                && self.s.capture.method == "auto"
                && self.stall.observe(
                    self.stall_clock.elapsed().as_secs_f64(),
                    stats.frame,
                    stats.dup_frames,
                    live.session.plan.fps,
                );
            let is_replay = live.kind == LiveKind::Replay;
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
            if stalled {
                self.on_capture_stalled(is_replay);
                return;
            }
            self.update_quiet(|s| {
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
        if self.last_disk_check.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
            self.last_disk_check = Some(Instant::now());
            self.check_disk();
            if let Some(m) = urec_win::monitors().into_iter().find(|m| m.primary) {
                self.overlay_monitor = (m.bounds.x, m.bounds.y, m.bounds.width, m.bounds.height);
            }
        }
        self.update_overlay();
        if self.panel.is_visible() {
            self.panel.set_model(self.panel_model());
        }
    }

    fn reduced_motion(&self) -> bool {
        match self.s.appearance.motion.as_str() {
            "reduced" => true,
            "full" => false,
            _ => urec_win::system_reduced_motion(),
        }
    }

    /// Hand the controller's settings to the UI after the controller changed one itself.
    fn publish_settings(&self) {
        let s = Box::new(self.s.clone());
        self.update(|st| st.settings_update = Some(s));
    }

    /// What the overlay panel shows, from the real state (nothing estimated).
    fn panel_model(&self) -> urec_win::panel::PanelModel {
        use urec_win::panel::{PanelModel, RecordView, ReplayView, Tone};
        let st = self.ctl.snapshot();
        let replay = match st.replay {
            ReplayState::Off => ReplayView::Off,
            ReplayState::Starting => ReplayView::Starting,
            ReplayState::On => ReplayView::On,
            ReplayState::Error(_) => ReplayView::Error,
        };
        let recording = match (&self.recording, st.finishing) {
            (_, true) => RecordView::Finishing,
            (Some(r), false) => RecordView::Recording { seconds: r.started.elapsed().as_secs() },
            (None, false) if st.starting && self.live.is_none() && !self.s.replay.enabled => RecordView::Starting,
            (None, false) => RecordView::Idle,
        };
        // Only when the microphone is really part of the capture and can be muted by a press.
        let mic_muted = (self.live.is_some() && self.s.audio.mic && !self.push_to_talk()).then_some(st.mic_muted);
        let message = st.notices.front().filter(|n| n.at.elapsed() < Duration::from_secs(8)).map(|n| {
            let tone = match n.level {
                Level::Success => Tone::Success,
                Level::Error | Level::Warning => Tone::Error,
                Level::Info => Tone::Info,
            };
            (n.text.clone(), tone)
        });
        let key = |k: &str| k.to_string();
        PanelModel {
            replay,
            replay_seconds: self.s.replay.seconds,
            saving: st.replay_saving > 0,
            recording,
            mic_muted,
            message,
            key_save: key(&self.s.hotkeys.save_replay),
            key_record: key(&self.s.hotkeys.record),
            key_mute: key(&self.s.hotkeys.mute_mic),
            key_panel: key(&self.s.hotkeys.panel),
        }
    }

    fn panel_action(&mut self, action: urec_win::panel::PanelAction) {
        use urec_win::panel::PanelAction;
        log(&format!("overlay panel: {action:?}"));
        match action {
            PanelAction::SaveReplay => self.save_replay(),
            PanelAction::ToggleReplay => self.set_replay(!self.s.replay.enabled),
            PanelAction::ToggleRecording => {
                if self.ctl.snapshot().finishing {
                    return;
                }
                if self.recording.is_some() {
                    self.stop_recording()
                } else {
                    self.start_recording()
                }
            }
            PanelAction::ToggleMic => self.toggle_mute(),
            PanelAction::ReplaySeconds(secs) => {
                if self.s.replay.seconds != secs {
                    let mut new = self.s.clone();
                    new.replay.seconds = secs;
                    let _ = new.save();
                    self.apply(new);
                    self.publish_settings();
                }
            }
            PanelAction::OpenSettings => {
                self.update(|s| s.open_page = Some("settings:instantreplay".into()));
                urec_win::show_window_titled(crate::ui::WINDOW_TITLE);
                self.ctl.repaint();
            }
            PanelAction::Close => {}
        }
    }

    /// Windows Graphics Capture is delivering almost no new frames (usually a fullscreen game).
    /// Switch to Desktop Duplication and remember it. Instant Replay restarts right away (its
    /// buffer starts over); a recording in progress is left alone and the next one uses it.
    fn on_capture_stalled(&mut self, is_replay: bool) {
        log("capture: Windows Graphics Capture is delivering almost no new frames; using Desktop Duplication from now on");
        self.prefer_dda = true;
        let _ = std::fs::create_dir_all(crate::settings::local_dir());
        let _ = std::fs::write(crate::settings::local_dir().join("capture-hint.txt"), "dda");
        if is_replay && self.recording.is_none() && self.s.replay.enabled {
            self.stop_live();
            self.start_replay();
        }
    }

    fn update_overlay(&self) {
        let st = self.ctl.snapshot();
        let replay_on = st.replay == ReplayState::On;
        let visible = self.s.appearance.overlay && st.overlay_visible && (self.recording.is_some() || replay_on);
        let mut o =
            urec_win::OverlayState { visible, corner: self.s.appearance.overlay_corner.clone(), ..Default::default() };
        o.size = match self.s.appearance.overlay_size.as_str() {
            "small" => 0.8,
            "large" => 1.3,
            _ => 1.0,
        };
        o.opacity = self.s.appearance.overlay_opacity as f32 / 100.0;
        o.hold_scale = match self.s.appearance.notice_length.as_str() {
            "short" => 0.6,
            "long" => 2.0,
            _ => 1.0,
        };
        // Keep the position even while hidden, so a save message appears in the right corner.
        o.monitor = self.overlay_monitor;
        if visible {
            o.dot = if self.recording.is_some() { (229, 72, 77) } else { (217, 164, 65) };
        }
        self.overlay.set(o);
    }

    fn on_crash(&mut self, log_tail: &str) {
        self.ctl.capturing.store(false, Ordering::Relaxed);
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
            self.publish_settings();
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
                "Recording is falling behind: frames are being dropped. Try a lower resolution, frame rate or the Low resource usage preset.",
                None,
            );
        }
    }

    fn check_disk(&mut self) {
        let free = urec_win::disk_space(&self.s.output_dir()).map(|(f, _)| f);
        let old = self.ctl.status.lock().ok().and_then(|s| s.disk_free);
        // Only wake the UI when the number shown would change noticeably.
        let changed = match (old, free) {
            (Some(a), Some(b)) => a.abs_diff(b) > 100_000_000,
            (a, b) => a != b,
        };
        if changed {
            self.update(|s| s.disk_free = free);
        }
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
        // Finish queued Instant Replay saves while the capture still runs (a save waits for
        // the segment that was open when the hotkey was pressed).
        if !self.saver.wait_idle(Duration::from_secs(30)) {
            log("shutdown: a replay save didn't finish within 30 s");
        }
        if self.recording.is_some() {
            self.stop_recording();
        }
        let _ = self.stop_live();
        self.hotkeys.stop();
    }
}

// ------------------------------------------------------------------ helpers

/// Name of the app in front, used to name clips. uRec itself counts as "Desktop".
fn foreground_app() -> String {
    urec_win::foreground_window()
        .filter(|w| !w.exe.eq_ignore_ascii_case("uRec"))
        .map(|w| w.app_name())
        .unwrap_or_else(|| "Desktop".into())
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
        Preset::Compact => "compact",
        Preset::Custom => "custom",
    }
}

pub fn explain_audio_failure(reason: &str) -> String {
    let r = reason.to_ascii_lowercase();
    if r.contains("access is denied") {
        "Windows is blocking microphone access. Turn on Settings › Privacy & security › Microphone › Let desktop apps access your microphone.".into()
    } else if r.contains("element not found") || r.contains("not found") {
        "The device wasn't found. It may be unplugged or disabled.".into()
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
