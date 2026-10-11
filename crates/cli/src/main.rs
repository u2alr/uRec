//! urec-cli: drive the uRec engine from the command line.
//!
//!   urec-cli probe [--codecs h264,hevc,av1] [--check-args]
//!   urec-cli record --out FILE --seconds N [options]
//!   urec-cli replay --dir DIR --out FILE --run-seconds N --save-seconds N [options]
//!
//! Options: --method auto|wgc|dda|gdi|test  --encoder auto|nvenc|amf|qsv|mf_hw|software|mf
//!          --codec h264|hevc|av1  --res native|2160|1440|1080|900|720|WxH  --fps N
//!          --bitrate KBPS  --source WxH (capture size)  --monitor N  --audio-test
//!          --container mp4|mkv  --no-gpu-path  --replay-seconds N  --no-cursor

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
        println!("best_{:?}={}", c, probe::best(&results, c).map(|e| e.to_string()).unwrap_or("none".into()));
    }
    if args.flag("check-args") && arg_errors > 0 {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// Build a StartRequest from options. Picks the encoder by probing when `--encoder auto`.
fn request(ffmpeg: &Path, args: &Args, output: Output) -> Result<StartRequest, String> {
    let codec = parse_codec(args.get("codec").unwrap_or("h264"));
    let (sw, sh) = args.get("source").and_then(parse_size).unwrap_or((1920, 1080));
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
    let index = args.num("monitor", 0);
    let bounds = Rect { x: 0, y: 0, width: sw, height: sh };
    let target = CaptureTarget::Monitor { index, hmonitor: 0, bounds };
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

fn test_audio(args: &Args) -> Vec<AudioInput> {
    if !args.flag("audio-test") {
        return vec![];
    }
    vec![AudioInput {
        source: "sine=frequency=440:sample_rate=48000".into(),
        lavfi: true,
        sample_rate: 48000,
        channels: 1,
        title: "Test tone".into(),
    }]
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
    let mut audio = || test_audio(args);
    let session = match Session::start(ffmpeg, &log_dir, &req, &mut audio) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    println!("startup_ms={}", t0.elapsed().as_millis());
    std::thread::sleep(Duration::from_secs(seconds as u64));
    report_session(&session, t0);
    let t1 = Instant::now();
    match session.stop(Duration::from_secs(20)) {
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
    let mut audio = || test_audio(args);
    let session = match Session::start(ffmpeg, &dir.join("logs"), &req, &mut audio) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    std::thread::sleep(Duration::from_secs(args.num("run-seconds", 20) as u64));
    let files = std::fs::read_dir(&dir)
        .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".ts")).count())
        .unwrap_or(0);
    println!("ring_files={files} wrap={}", buffer.wrap());
    let t1 = Instant::now();
    let saved = buffer.save(ffmpeg, args.num("save-seconds", 10), &out, true);
    println!("save_ms={}", t1.elapsed().as_millis());
    report_session(&session, t0);
    let _ = session.stop(Duration::from_secs(10));
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
