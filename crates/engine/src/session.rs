//! Starting and stopping a capture session with automatic fallback:
//! capture method (WGC → DDA → GDI) × frame path (GPU zero-copy → system memory).
//! The encoder is never switched silently; the caller chooses it.

use crate::encoder::Encoder;
use crate::pipeline::{AudioInput, CaptureMethod, CaptureTarget, FramePath, Output, Plan};
use crate::process::{FfmpegProcess, Stats};
use crate::video::VideoSettings;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct StartRequest {
    pub target: CaptureTarget,
    pub methods: Vec<CaptureMethod>,
    pub cursor: bool,
    pub encoder: Encoder,
    pub video: VideoSettings,
    pub out_width: u32,
    pub out_height: u32,
    pub fps: u32,
    pub audio_kbps: u32,
    pub output: Output,
    /// Try the zero-copy GPU path first when the method and encoder support it.
    pub prefer_gpu_path: bool,
    /// How long to wait for the first encoded frame.
    pub first_frame_timeout: Duration,
}

/// One failed attempt, kept so the UI can explain what happened.
#[derive(Clone, Debug, PartialEq)]
pub struct Attempt {
    pub method: CaptureMethod,
    pub frame_path: FramePath,
    pub error: String,
}

#[derive(Debug)]
pub struct StartError {
    pub attempts: Vec<Attempt>,
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Recording couldn't start with any capture method:")?;
        for a in &self.attempts {
            let first = a.error.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
            writeln!(f, "  {} ({:?}): {}", a.method.label(), a.frame_path, first)?;
        }
        Ok(())
    }
}

impl std::error::Error for StartError {}

pub struct Session {
    pub plan: Plan,
    pub process: FfmpegProcess,
    pub attempts: Vec<Attempt>,
}

impl Session {
    /// Start capture. `make_audio` is called before every attempt so audio pipes can be
    /// recreated (a named pipe instance is consumed by the ffmpeg that connected to it).
    pub fn start(
        ffmpeg: &Path,
        log_dir: &Path,
        req: &StartRequest,
        make_audio: &mut dyn FnMut() -> Vec<AudioInput>,
    ) -> Result<Session, StartError> {
        let mut attempts = Vec::new();
        for &method in &req.methods {
            let mut paths = Vec::new();
            if req.prefer_gpu_path && Plan::gpu_path_possible(method, &req.encoder) {
                paths.push(FramePath::Gpu);
            }
            paths.push(FramePath::System);
            for frame_path in paths {
                let plan = Plan {
                    method,
                    target: req.target.clone(),
                    cursor: req.cursor,
                    encoder: req.encoder,
                    video: req.video.clone(),
                    out_width: req.out_width,
                    out_height: req.out_height,
                    fps: req.fps,
                    frame_path,
                    audio: make_audio(),
                    audio_kbps: req.audio_kbps,
                    output: req.output.clone(),
                };
                let log = log_dir.join(format!("ffmpeg-{}-{:?}.log", method.id(), frame_path).to_lowercase());
                let mut process = match FfmpegProcess::spawn(ffmpeg, plan.ffmpeg_args(), &log) {
                    Ok(p) => p,
                    Err(e) => {
                        attempts.push(Attempt { method, frame_path, error: format!("couldn't start ffmpeg: {e}") });
                        continue;
                    }
                };
                match process.wait_until_flowing(req.first_frame_timeout) {
                    Ok(()) => return Ok(Session { plan, process, attempts }),
                    Err(e) => {
                        process.kill();
                        if let Output::File { path, .. } = &req.output {
                            let _ = std::fs::remove_file(path);
                        }
                        attempts.push(Attempt { method, frame_path, error: e });
                    }
                }
            }
        }
        Err(StartError { attempts })
    }

    pub fn stats(&self) -> Stats {
        self.process.stats()
    }

    pub fn running(&mut self) -> bool {
        self.process.running()
    }

    pub fn output_path(&self) -> Option<PathBuf> {
        match &self.plan.output {
            Output::File { path, .. } => Some(PathBuf::from(path)),
            Output::ReplayRing { .. } => None,
        }
    }

    /// Stop and finalize. Returns the output file for file recordings.
    pub fn stop(self, timeout: Duration) -> Result<Option<PathBuf>, String> {
        let out = self.output_path();
        let res = self.process.stop(timeout);
        match (&res, &out) {
            (_, Some(p)) if p.metadata().map(|m| m.len() > 0).unwrap_or(false) => Ok(out),
            (Err(e), _) => Err(e.clone()),
            (Ok(_), Some(_)) => Err("The recording file is empty.".into()),
            (Ok(_), None) => Ok(None),
        }
    }
}
