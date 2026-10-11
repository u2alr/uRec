//! Running FFmpeg: locating the binary, live progress statistics, graceful stop.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Find ffmpeg: $UREC_FFMPEG, next to the executable, ./third_party/ffmpeg, then PATH.
pub fn locate_ffmpeg() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Ok(p) = std::env::var("UREC_FFMPEG") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(me) = std::env::current_exe() {
        if let Some(d) = me.parent() {
            dirs.push(d.to_path_buf());
            dirs.push(d.join("ffmpeg"));
            // target/debug -> repo root during development
            for up in d.ancestors().skip(1).take(3) {
                dirs.push(up.join("third_party").join("ffmpeg"));
            }
        }
    }
    dirs.push(PathBuf::from("third_party").join("ffmpeg"));
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

/// A `Command` that never flashes a console window on Windows.
pub fn quiet_command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// Run ffmpeg to completion and capture stderr. Used for probes and short jobs.
pub fn run_capture(ffmpeg: &Path, args: &[String], timeout: Duration) -> std::io::Result<(bool, String)> {
    let mut child =
        quiet_command(ffmpeg).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
    let mut stderr = child.stderr.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stderr, &mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break Some(st);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let err = reader.join().unwrap_or_default();
    match status {
        Some(st) => Ok((st.success(), err)),
        None => Ok((false, format!("timed out after {:?}\n{err}", timeout))),
    }
}

/// Live statistics from `-progress pipe:1`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub frame: u64,
    pub fps: f64,
    pub dup_frames: u64,
    pub drop_frames: u64,
    /// Encoding speed relative to real time (1.0 = keeping up).
    pub speed: f64,
    pub out_time_us: i64,
    pub total_size: u64,
    pub ended: bool,
    pub updated: Option<Instant>,
}

impl Stats {
    /// Apply one `key=value` line. Returns true at the end of a progress block.
    pub fn apply_line(&mut self, line: &str) -> bool {
        let Some((k, v)) = line.trim().split_once('=') else { return false };
        let v = v.trim();
        match k {
            "frame" => self.frame = v.parse().unwrap_or(self.frame),
            "fps" => self.fps = v.parse().unwrap_or(self.fps),
            "dup_frames" => self.dup_frames = v.parse().unwrap_or(self.dup_frames),
            "drop_frames" => self.drop_frames = v.parse().unwrap_or(self.drop_frames),
            "speed" => self.speed = v.trim_end_matches('x').parse().unwrap_or(self.speed),
            "out_time_us" | "out_time_ms" => self.out_time_us = v.parse().unwrap_or(self.out_time_us),
            "total_size" => self.total_size = v.parse().unwrap_or(self.total_size),
            "progress" => {
                self.ended = v == "end";
                self.updated = Some(Instant::now());
                return true;
            }
            _ => {}
        }
        false
    }

    /// Falling behind: the encoder can't keep up with real time.
    pub fn lagging(&self) -> bool {
        self.frame > 60 && self.speed > 0.0 && self.speed < 0.95
    }
}

/// A running ffmpeg with progress reporting and a graceful stop.
pub struct FfmpegProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stats: Arc<Mutex<Stats>>,
    reader: Option<JoinHandle<()>>,
    pub log_path: PathBuf,
    pub args: Vec<String>,
    pub started: Instant,
}

impl FfmpegProcess {
    /// Start ffmpeg. stderr goes to `log_path`.
    pub fn spawn(ffmpeg: &Path, args: Vec<String>, log_path: &Path) -> std::io::Result<Self> {
        if let Some(dir) = log_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let log = std::fs::File::create(log_path)?;
        let mut child = quiet_command(ffmpeg)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped");
        let stats = Arc::new(Mutex::new(Stats::default()));
        let s2 = stats.clone();
        let reader = std::thread::Builder::new().name("ffmpeg-progress".into()).spawn(move || {
            let mut pending = Stats::default();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if pending.apply_line(&line) {
                    if let Ok(mut s) = s2.lock() {
                        *s = pending.clone();
                    }
                }
            }
        })?;
        Ok(FfmpegProcess {
            child,
            stdin,
            stats,
            reader: Some(reader),
            log_path: log_path.to_path_buf(),
            args,
            started: Instant::now(),
        })
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn stats(&self) -> Stats {
        self.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Still running?
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn try_status(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Wait until the first frame has been encoded, or ffmpeg exits. Returns Ok when frames flow.
    pub fn wait_until_flowing(&mut self, timeout: Duration) -> Result<(), String> {
        let start = Instant::now();
        loop {
            if let Some(st) = self.try_status() {
                return Err(format!("ffmpeg exited ({st}): {}", self.log_tail(1200)));
            }
            if self.stats().frame > 0 {
                return Ok(());
            }
            if start.elapsed() > timeout {
                return Err(format!("no frames after {:?}: {}", timeout, self.log_tail(1200)));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Ask ffmpeg to finish (it finalizes the file), killing it if it doesn't exit in time.
    pub fn stop(mut self, timeout: Duration) -> Result<ExitStatus, String> {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
            drop(stdin);
        }
        let start = Instant::now();
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(25)),
                _ => {
                    let _ = self.child.kill();
                    self.child.wait().map_err(|e| e.to_string())?;
                    if let Some(r) = self.reader.take() {
                        let _ = r.join();
                    }
                    return Err(format!("ffmpeg did not stop within {:?} and was terminated", timeout));
                }
            }
        };
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
        Ok(status)
    }

    /// Kill immediately (used when abandoning a failed start).
    pub fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn log_tail(&self, max: usize) -> String {
        read_tail(&self.log_path, max)
    }

    /// Raw OS handle of the process (for job objects on Windows).
    #[cfg(windows)]
    pub fn raw_handle(&self) -> std::os::windows::io::RawHandle {
        use std::os::windows::io::AsRawHandle;
        self.child.as_raw_handle()
    }
}

impl Drop for FfmpegProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            if let Some(mut stdin) = self.stdin.take() {
                let _ = stdin.write_all(b"q\n");
            }
            std::thread::sleep(Duration::from_millis(300));
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub fn read_tail(path: &Path, max: usize) -> String {
    let s = std::fs::read(path).unwrap_or_default();
    let start = s.len().saturating_sub(max);
    String::from_utf8_lossy(&s[start..]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_blocks() {
        let mut s = Stats::default();
        let block = "frame=120\nfps=59.9\ndup_frames=3\ndrop_frames=1\nout_time_us=2000000\nspeed=0.98x\ntotal_size=1024\nprogress=continue\n";
        let mut ends = 0;
        for l in block.lines() {
            if s.apply_line(l) {
                ends += 1;
            }
        }
        assert_eq!(ends, 1);
        assert_eq!(s.frame, 120);
        assert_eq!(s.drop_frames, 1);
        assert!((s.speed - 0.98).abs() < 1e-9);
        assert!(!s.ended);
        assert!(s.apply_line("progress=end"));
        assert!(s.ended);
        // N/A values keep the previous value
        s.apply_line("speed=N/A");
        assert!((s.speed - 0.98).abs() < 1e-9);
    }
}
