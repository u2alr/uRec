//! urec-cli: drive the uRec engine from the command line.
//!
//!   urec-cli probe [--codecs h264,hevc,av1] [--check-args]
//!   urec-cli record --out FILE --seconds N [options]
//!   urec-cli replay --dir DIR --out FILE --run-seconds N --save-seconds N [options]
//!   urec-cli devices
//!
//! Options: --method auto|wgc|dda|gdi|test  --encoder auto|nvenc|amf|qsv|mf_hw|software|mf
//!          --codec h264|hevc|av1  --res native|2160|1440|1080|900|720|WxH  --fps N
//!          --bitrate KBPS  --source WxH (capture size)  --monitor N  --audio-test
//!          --container mp4|mkv  --no-gpu-path  --replay-seconds N  --no-cursor
//!          --audio none|test|system|mic|both  --layout mixed|separate  --window TITLE

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};
use urec_engine::encoder::{Backend, Encoder};
use urec_engine::pipeline::{AudioInput, CaptureMethod, CaptureTarget, Container, Output, Rect};
use urec_engine::probe;
use urec_engine::process::locate_ffmpeg;
use urec_engine::replay::ReplayBuffer;
use urec_engine::session::{Session, StartRequest};
use urec_engine::video::{self, Codec, FrameRate, Resolution, VideoSettings};
use urec_win::{AudioFeed, AudioSettings, TrackLayout};

struct Args {
    cmd: String,
    opts: HashMap<String, String>,
    flags: Vec<String>,
}

fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().unwrap_or_else(|| "help".into());
    let mut opts = HashMap::new();
    let mut flags = Vec::new();
    let rest: Vec<String> = it.collect();
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i].trim_start_matches("--").to_string();
        let takes_value = i + 1 < rest.len() && !rest[i + 1].starts_with("--");
        if takes_value {
            opts.insert(a, rest[i + 1].clone());
            i += 2;
        } else {
            flags.push(a);
            i += 1;
        }
    }
    Args { cmd, opts, flags }
}

impl Args {
    fn get(&self, k: &str) -> Option<&str> {
        self.opts.get(k).map(String::as_str)
    }
    fn num(&self, k: &str, default: u32) -> u32 {
        self.get(k).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
    fn flag(&self, k: &str) -> bool {
        self.flags.iter().any(|f| f == k)
    }
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(['x', 'X'])?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn parse_codec(s: &str) -> Codec {
    match s {
        "hevc" | "h265" => Codec::Hevc,
        "av1" => Codec::Av1,
        _ => Codec::H264,
    }
}

fn main() -> ExitCode {
    let args = parse_args();
    urec_win::set_dpi_aware();
    if args.cmd == "devices" {
        return cmd_devices();
    }
    let Some(ffmpeg) = locate_ffmpeg() else {
        eprintln!("ffmpeg not found (set UREC_FFMPEG or put ffmpeg next to urec-cli)");
        return ExitCode::from(2);
    };
    match args.cmd.as_str() {
        "probe" => cmd_probe(&ffmpeg, &args),
        "record" => cmd_record(&ffmpeg, &args),
        "replay" => cmd_replay(&ffmpeg, &args),
        _ => {
            println!(
                "{}",
                include_str!("main.rs")
                    .lines()
                    .take(10)
                    .map(|l| l.trim_start_matches("//!"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            ExitCode::SUCCESS
        }
    }
}

fn cmd_probe(ffmpeg: &Path, args: &Args) -> ExitCode {
    let codecs: Vec<Codec> = args.get("codecs").unwrap_or("h264,hevc,av1").split(',').map(parse_codec).collect();
    println!("ffmpeg={}", probe::ffmpeg_version(ffmpeg).unwrap_or_default());
    let results = probe::probe_encoders(ffmpeg, &codecs, &VideoSettings::default());
    let mut arg_errors = 0;
    for r in &results {
        println!(
            "encoder={:<12} ok={} ms={:<5} arg_error={} reason={}",
            r.encoder.ffmpeg_name().unwrap_or("?"),
            r.ok,
            r.millis,
            r.argument_error,
            r.reason
        );
        if r.argument_error {
            arg_errors += 1;
            println!("    raw: {}", r.raw_error.lines().take(3).collect::<Vec<_>>().join(" | "));
        }
    }
    for c in codecs {
        println!(
            "best_{}={}",
            format!("{c:?}").to_lowercase(),
            probe::best(&results, c).map(|e| e.to_string()).unwrap_or("none".into())
        );
    }
    if args.flag("check-args") && arg_errors > 0 {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// Build a StartRequest from options. Picks the encoder by probing when `--encoder auto`.
fn request(ffmpeg: &Path, args: &Args, output: Output) -> Result<StartRequest, String> {
    let codec = parse_codec(args.get("codec").unwrap_or("h264"));
    let (sw, sh) = match args.get("source").and_then(parse_size) {
        Some(s) => s,
        None => pick_target(args, 1920, 1080)?.source_size(),
    };
    let fps = args.num("fps", 30);
    let resolution = match args.get("res").unwrap_or("native") {
        "native" => Resolution::Native,
        s => match parse_size(s) {
            Some((width, height)) => Resolution::Custom { width, height },
            None => Resolution::Height(s.trim_end_matches('p').parse().map_err(|_| format!("bad --res {s}"))?),
        },
    };
    let mut v = VideoSettings { resolution, frame_rate: FrameRate::Fixed(fps), codec, ..VideoSettings::default() };
    let (w, h, fps) = video::validate(&v, sw, sh, None).map_err(|e| e.to_string())?;
    v.bitrate_kbps = args.num("bitrate", video::recommended_bitrate_kbps(w, h, fps, codec, 1.0));

    let encoder = match args.get("encoder").unwrap_or("auto") {
        "auto" => {
            let results = probe::probe_encoders(ffmpeg, &[codec], &v);
            let e = probe::best(&results, codec).ok_or("no working encoder")?;
            println!("auto_encoder={}", e.ffmpeg_name().unwrap());
            e
        }
        id => Encoder::new(Backend::from_id(id).ok_or(format!("unknown encoder {id}"))?, codec),
    };
    let target = pick_target(args, sw, sh)?;
    let methods = match args.get("method").unwrap_or("auto") {
        "auto" => CaptureMethod::order_for(&target),
        m => vec![CaptureMethod::from_id(m).ok_or(format!("unknown method {m}"))?],
    };
    Ok(StartRequest {
        target,
        methods,
        cursor: !args.flag("no-cursor"),
        encoder,
        video: v,
        out_width: w,
        out_height: h,
        fps,
        audio_kbps: 160,
        output,
        prefer_gpu_path: !args.flag("no-gpu-path"),
        first_frame_timeout: Duration::from_secs(10),
    })
}

/// Monitor (by --monitor index into the real monitor list) or window (--window substring).
fn pick_target(args: &Args, sw: u32, sh: u32) -> Result<CaptureTarget, String> {
    if let Some(q) = args.get("window") {
        let q = q.to_lowercase();
        let w = urec_win::windows()
            .into_iter()
            .find(|w| w.title.to_lowercase().contains(&q) || w.exe.to_lowercase().contains(&q))
            .ok_or(format!("no window matching '{q}'"))?;
        return Ok(w.target());
    }
    let index = args.num("monitor", 0) as usize;
    let monitors = urec_win::monitors();
    match monitors.get(index) {
        Some(m) if args.get("source").is_none() => Ok(m.target()),
        _ => Ok(CaptureTarget::Monitor {
            index: index as u32,
            hmonitor: 0,
            bounds: Rect { x: 0, y: 0, width: sw, height: sh },
        }),
    }
}

/// Audio inputs for one start attempt. Keeps the WASAPI feed alive in `holder`.
fn make_audio(args: &Args, holder: &RefCell<Option<AudioFeed>>) -> Vec<AudioInput> {
    let mode = if args.flag("audio-test") { "test" } else { args.get("audio").unwrap_or("none") };
    match mode {
        "test" => vec![AudioInput {
            source: "sine=frequency=440:sample_rate=48000".into(),
            lavfi: true,
            sample_rate: 48000,
            channels: 1,
            title: "Test tone".into(),
        }],
        "system" | "mic" | "both" => {
            *holder.borrow_mut() = None; // release the previous attempt's devices and pipes
            let settings = AudioSettings {
                system: mode != "mic",
                mic: mode != "system",
                layout: if args.get("layout") == Some("separate") { TrackLayout::Separate } else { TrackLayout::Mixed },
                ..AudioSettings::default()
            };
            let (feed, inputs) = AudioFeed::start(&settings);
            let (sys, mic) = feed.states();
            println!("audio_system_state={sys:?}");
            println!("audio_mic_state={mic:?}");
            *holder.borrow_mut() = Some(feed);
            inputs
        }
        _ => vec![],
    }
}

#[cfg(windows)]
fn bind(s: &Session) {
    println!("job_bound={}", urec_win::bind_child_to_app(s.process.raw_handle()));
}

#[cfg(not(windows))]
fn bind(_s: &Session) {}

fn cmd_devices() -> ExitCode {
    println!("windows_build={:?} wgc={}", urec_win::windows_build(), urec_win::supports_wgc());
    for (i, m) in urec_win::monitors().iter().enumerate() {
        println!(
            "monitor {i}: {} device={} bounds={:?} primary={} dxgi={:?}/{:?} dda={}",
            m.name,
            m.device,
            m.bounds,
            m.primary,
            m.dxgi_adapter,
            m.dxgi_output,
            m.dda_capable()
        );
    }
    for w in urec_win::windows().iter().take(15) {
        println!("window: '{}' exe={} app={} {:?}", w.title, w.exe, w.app_name(), w.bounds);
    }
    for d in urec_win::audio_outputs() {
        println!("audio_out: {} default={}", d.name, d.is_default);
    }
    for d in urec_win::audio_inputs() {
        println!("audio_in: {} default={}", d.name, d.is_default);
    }
    if let Some((free, total)) = urec_win::disk_space(&std::env::current_dir().unwrap_or_default()) {
        println!("disk_free={} of {}", video::format_bytes(free), video::format_bytes(total));
    }
    ExitCode::SUCCESS
}

fn report_session(s: &Session, started: Instant) {
    let st = s.stats();
    println!("method={}", s.plan.method.id());
    println!("frame_path={:?}", s.plan.frame_path);
    println!("encoder={}", s.plan.encoder.ffmpeg_name().unwrap_or("?"));
    println!("size={}x{} fps={}", s.plan.out_width, s.plan.out_height, s.plan.fps);
    println!("frames={} drop={} dup={} speed={:.2}", st.frame, st.drop_frames, st.dup_frames, st.speed);
    println!("elapsed_ms={}", started.elapsed().as_millis());
    for a in &s.attempts {
        println!("fallback_from={}/{:?}: {}", a.method.id(), a.frame_path, a.error.lines().last().unwrap_or(""));
    }
}

fn report_audio(holder: &RefCell<Option<AudioFeed>>) {
    if let Some(feed) = holder.borrow().as_ref() {
        let (sys, mic) = feed.states();
        let (ls, lm) = feed.levels();
        println!("audio_end_system={sys:?} peak={ls:.3}");
        println!("audio_end_mic={mic:?} peak={lm:.3}");
    }
}

fn cmd_record(ffmpeg: &Path, args: &Args) -> ExitCode {
    let Some(out) = args.get("out").map(PathBuf::from) else {
        eprintln!("--out is required");
        return ExitCode::from(2);
    };
    let container = if args.get("container") == Some("mkv") { Container::Mkv } else { Container::Mp4 };
    let output = Output::File { path: out.to_string_lossy().into(), container };
    let req = match request(ffmpeg, args, output) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let seconds = args.num("seconds", 5);
    let log_dir = out.parent().unwrap_or(Path::new(".")).join("logs");
    let t0 = Instant::now();
    let holder = RefCell::new(None);
    let mut audio = || make_audio(args, &holder);
    let session = match Session::start(ffmpeg, &log_dir, &req, &mut audio) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    bind(&session);
    println!("startup_ms={}", t0.elapsed().as_millis());
    std::thread::sleep(Duration::from_secs(seconds as u64));
    report_session(&session, t0);
    report_audio(&holder);
    let t1 = Instant::now();
    let stopped = session.stop(Duration::from_secs(20));
    holder.borrow_mut().take(); // stop audio capture after ffmpeg has finished
    match stopped {
        Ok(Some(p)) => {
            println!("stop_ms={}", t1.elapsed().as_millis());
            println!("output={}", p.display());
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("stop failed: {e}");
            ExitCode::from(1)
        }
    }
}

fn cmd_replay(ffmpeg: &Path, args: &Args) -> ExitCode {
    let (Some(dir), Some(out)) = (args.get("dir").map(PathBuf::from), args.get("out").map(PathBuf::from)) else {
        eprintln!("--dir and --out are required");
        return ExitCode::from(2);
    };
    let replay_seconds = args.num("replay-seconds", 30);
    let buffer = ReplayBuffer::new(dir.clone(), replay_seconds, 2);
    if let Err(e) = buffer.clear() {
        eprintln!("can't prepare buffer: {e}");
        return ExitCode::from(1);
    }
    let output = Output::ReplayRing { dir: dir.to_string_lossy().into(), segment_seconds: 2, wrap: buffer.wrap() };
    let req = match request(ffmpeg, args, output) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let t0 = Instant::now();
    let holder = RefCell::new(None);
    let mut audio = || make_audio(args, &holder);
    let session = match Session::start(ffmpeg, &dir.join("logs"), &req, &mut audio) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    bind(&session);
    std::thread::sleep(Duration::from_secs(args.num("run-seconds", 20) as u64));
    let files = std::fs::read_dir(&dir)
        .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".ts")).count())
        .unwrap_or(0);
    println!("ring_files={files}");
    println!("wrap={}", buffer.wrap());
    let t1 = Instant::now();
    let saved = buffer.save(ffmpeg, args.num("save-seconds", 10), &out, true);
    println!("save_ms={}", t1.elapsed().as_millis());
    report_session(&session, t0);
    report_audio(&holder);
    let _ = session.stop(Duration::from_secs(10));
    holder.borrow_mut().take(); // stop audio capture after ffmpeg has finished
    match saved {
        Ok(p) => {
            println!("output={}", p.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("save failed: {e}");
            ExitCode::from(1)
        }
    }
}
