//! The clip editor's model and its FFmpeg commands.
//!
//! A `Project` is an ordered list of pieces, each a time range of one source clip. Cutting
//! "this, this and this" is splitting at the playhead and deleting pieces; trimming moves a
//! piece's edges; pieces can be reordered and can come from several clips. Export renders
//! the timeline into a new file (originals are never touched), either frame-exact by
//! re-encoding or, when nothing needs re-encoding, losslessly by stream copy.
//!
//! Std-only and platform-neutral, so all of it is unit-tested.

use std::path::{Path, PathBuf};

/// Shortest piece the editor allows (seconds).
pub const MIN_PIECE: f64 = 0.1;

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub path: PathBuf,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub video_codec: String,
    pub audio_streams: usize,
}

/// A time range [start, end) of one source, in source seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub source: usize,
    pub start: f64,
    pub end: f64,
}

impl Piece {
    pub fn len(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }
    pub fn is_empty(&self) -> bool {
        self.len() <= 0.0
    }
}

/// Crop rectangle as fractions (0..1) of the output frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Crop {
    pub const FULL: Crop = Crop { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

    /// The largest centred rectangle with aspect `ratio` (width / height) in a `w`×`h` frame.
    pub fn centered(ratio: f32, w: u32, h: u32) -> Crop {
        let frame = w as f32 / h.max(1) as f32;
        if ratio >= frame {
            let ch = frame / ratio;
            Crop { x: 0.0, y: (1.0 - ch) / 2.0, w: 1.0, h: ch }
        } else {
            let cw = ratio / frame;
            Crop { x: (1.0 - cw) / 2.0, y: 0.0, w: cw, h: 1.0 }
        }
    }

    /// Keep inside the frame and at least 5 % in each direction.
    pub fn clamped(self) -> Crop {
        let w = self.w.clamp(0.05, 1.0);
        let h = self.h.clamp(0.05, 1.0);
        Crop { x: self.x.clamp(0.0, 1.0 - w), y: self.y.clamp(0.0, 1.0 - h), w, h }
    }

    /// Pixel rectangle (x, y, w, h) in a `fw`×`fh` frame, with even sizes for the encoder.
    pub fn pixels(&self, fw: u32, fh: u32) -> (u32, u32, u32, u32) {
        let c = self.clamped();
        let even = |v: f32| ((v.round() as u32) / 2 * 2).max(2);
        let w = even(c.w * fw as f32).min(fw / 2 * 2);
        let h = even(c.h * fh as f32).min(fh / 2 * 2);
        let x = ((c.x * fw as f32).round() as u32).min(fw - w);
        let y = ((c.y * fh as f32).round() as u32).min(fh - h);
        (x, y, w, h)
    }

    pub fn is_full(&self) -> bool {
        *self == Crop::FULL
    }
}

/// Everything an edit changes (what undo restores).
#[derive(Clone, Debug, PartialEq)]
pub struct EditState {
    pub pieces: Vec<Piece>,
    pub crop: Option<Crop>,
    pub mute: bool,
    /// 0..=2 (1 = unchanged).
    pub volume: f32,
}

#[derive(Clone, Debug)]
pub struct Project {
    pub sources: Vec<Source>,
    pub state: EditState,
    past: Vec<EditState>,
    future: Vec<EditState>,
}

const HISTORY: usize = 200;

impl Project {
    /// One piece per source, whole clips, in the given order.
    pub fn new(sources: Vec<Source>) -> Project {
        let pieces = sources
            .iter()
            .enumerate()
            .map(|(i, s)| Piece { source: i, start: 0.0, end: s.duration.max(MIN_PIECE) })
            .collect();
        Project {
            sources,
            state: EditState { pieces, crop: None, mute: false, volume: 1.0 },
            past: Vec::new(),
            future: Vec::new(),
        }
    }

    pub fn pieces(&self) -> &[Piece] {
        &self.state.pieces
    }

    /// Output frame size: the first clip's.
    pub fn frame_size(&self) -> (u32, u32) {
        self.sources.first().map(|s| (s.width.max(2), s.height.max(2))).unwrap_or((1920, 1080))
    }

    pub fn fps(&self) -> f64 {
        self.sources.first().map(|s| s.fps).filter(|f| *f > 1.0).unwrap_or(30.0)
    }

    pub fn duration(&self) -> f64 {
        self.state.pieces.iter().map(Piece::len).sum()
    }

    /// Timeline start of each piece.
    pub fn starts(&self) -> Vec<f64> {
        let mut t = 0.0;
        self.state
            .pieces
            .iter()
            .map(|p| {
                let s = t;
                t += p.len();
                s
            })
            .collect()
    }

    /// Which piece is at timeline time `t`, and the source time there.
    pub fn locate(&self, t: f64) -> Option<(usize, f64)> {
        let mut acc = 0.0;
        let n = self.state.pieces.len();
        for (i, p) in self.state.pieces.iter().enumerate() {
            if t < acc + p.len() || i + 1 == n {
                let into = (t - acc).clamp(0.0, p.len());
                return Some((i, p.start + into));
            }
            acc += p.len();
        }
        None
    }

    fn record(&mut self) {
        self.past.push(self.state.clone());
        if self.past.len() > HISTORY {
            self.past.remove(0);
        }
        self.future.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        match self.past.pop() {
            Some(s) => {
                self.future.push(std::mem::replace(&mut self.state, s));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.future.pop() {
            Some(s) => {
                self.past.push(std::mem::replace(&mut self.state, s));
                true
            }
            None => false,
        }
    }

    /// Split the piece under timeline time `t` in two. False if `t` is too close to an edge.
    pub fn split_at(&mut self, t: f64) -> bool {
        let Some((i, src_t)) = self.locate(t) else { return false };
        let p = self.state.pieces[i];
        if src_t - p.start < MIN_PIECE || p.end - src_t < MIN_PIECE {
            return false;
        }
        self.record();
        self.state.pieces[i].end = src_t;
        self.state.pieces.insert(i + 1, Piece { start: src_t, ..p });
        true
    }

    /// Remove a piece. The last remaining piece can't be removed.
    pub fn delete(&mut self, i: usize) -> bool {
        if i >= self.state.pieces.len() || self.state.pieces.len() <= 1 {
            return false;
        }
        self.record();
        self.state.pieces.remove(i);
        true
    }

    /// Move piece `from` so it ends up at index `to`.
    pub fn move_piece(&mut self, from: usize, to: usize) -> bool {
        let n = self.state.pieces.len();
        if from >= n || to >= n || from == to {
            return false;
        }
        self.record();
        let p = self.state.pieces.remove(from);
        self.state.pieces.insert(to, p);
        true
    }

    /// Set a piece's start or end (source seconds), clamped to the source and a minimum length.
    /// `record` = false while dragging (call `commit_drag` once at the end for one undo step).
    pub fn set_edge(&mut self, i: usize, start: bool, value: f64, record: bool) -> bool {
        let Some(p) = self.state.pieces.get(i).copied() else { return false };
        let dur = self.sources.get(p.source).map(|s| s.duration).unwrap_or(p.end);
        let (s, e) = if start {
            (value.clamp(0.0, p.end - MIN_PIECE), p.end)
        } else {
            (p.start, value.clamp(p.start + MIN_PIECE, dur.max(p.start + MIN_PIECE)))
        };
        if (s, e) == (p.start, p.end) {
            return false;
        }
        if record {
            self.record();
        }
        self.state.pieces[i].start = s;
        self.state.pieces[i].end = e;
        true
    }

    /// Record the state before a drag started as one undo step.
    pub fn commit_drag(&mut self, before: EditState) {
        if before != self.state {
            self.past.push(before);
            if self.past.len() > HISTORY {
                self.past.remove(0);
            }
            self.future.clear();
        }
    }

    pub fn set_crop(&mut self, crop: Option<Crop>) {
        let crop = crop.map(Crop::clamped).filter(|c| !c.is_full());
        if crop != self.state.crop {
            self.record();
            self.state.crop = crop;
        }
    }

    pub fn set_mute(&mut self, mute: bool) {
        if mute != self.state.mute {
            self.record();
            self.state.mute = mute;
        }
    }

    pub fn set_volume(&mut self, v: f32, record: bool) {
        let v = v.clamp(0.0, 2.0);
        if (v - self.state.volume).abs() > f32::EPSILON {
            if record {
                self.record();
            }
            self.state.volume = v;
        }
    }

    /// Output picture size after cropping.
    pub fn output_size(&self) -> (u32, u32) {
        let (w, h) = self.frame_size();
        match self.state.crop {
            Some(c) => {
                let (_, _, cw, ch) = c.pixels(w, h);
                (cw, ch)
            }
            None => (w / 2 * 2, h / 2 * 2),
        }
    }

    /// Can this be exported without re-encoding? Needs: no crop, unchanged audio, and every clip
    /// with the same codec and size. Cuts then land on the keyframe at or before each start.
    pub fn lossless_possible(&self) -> Result<(), &'static str> {
        if self.state.crop.is_some() {
            return Err("cropping needs a re-encode");
        }
        if !self.state.mute && (self.state.volume - 1.0).abs() > 0.001 {
            return Err("changing the volume needs a re-encode");
        }
        let used: Vec<&Source> = self.state.pieces.iter().filter_map(|p| self.sources.get(p.source)).collect();
        let Some(first) = used.first() else { return Err("nothing to export") };
        if used.iter().any(|s| {
            s.video_codec != first.video_codec
                || (s.width, s.height) != (first.width, first.height)
                || s.audio_streams != first.audio_streams
        }) {
            return Err("the clips differ in size or format");
        }
        Ok(())
    }
}

fn f(t: f64) -> String {
    format!("{:.3}", t.max(0.0))
}

/// How the audio of each piece is fed into the timeline: all of a clip's tracks mixed, or
/// silence for clips without sound (so pieces always line up).
fn piece_audio(k: usize, src: &Source, len: f64, volume: f32) -> String {
    let tail = format!("aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,volume={volume:.3}[a{k}]");
    match src.audio_streams {
        0 => format!("anullsrc=r=48000:cl=stereo:d={},{tail}", f(len)),
        1 => format!("[{k}:a:0]asetpts=PTS-STARTPTS,{tail}"),
        n => {
            let ins: String = (0..n).map(|j| format!("[{k}:a:{j}]")).collect();
            format!("{ins}amix=inputs={n}:normalize=0,asetpts=PTS-STARTPTS,{tail}")
        }
    }
}

/// The input and filter part of a render of `pieces` (each piece opened with a fast, accurate
/// input seek), producing `[v]` and, unless muted, `[a]`.
fn graph(
    p: &Project,
    pieces: &[Piece],
    (out_w, out_h): (u32, u32),
    fps: f64,
    with_audio: bool,
    apply_crop: bool,
) -> (Vec<String>, String) {
    let (fw, fh) = p.frame_size();
    let mut inputs = Vec::new();
    let mut chains = Vec::new();
    let mut concat_in = String::new();
    for (k, piece) in pieces.iter().enumerate() {
        let src = &p.sources[piece.source];
        inputs.extend([
            "-ss".into(),
            f(piece.start),
            "-t".into(),
            f(piece.len()),
            "-i".into(),
            src.path.to_string_lossy().to_string(),
        ]);
        // Every piece is brought to the timeline's frame size and rate so they can be joined.
        chains.push(format!(
            "[{k}:v:0]setpts=PTS-STARTPTS,scale={fw}:{fh}:force_original_aspect_ratio=decrease,pad={fw}:{fh}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps:.3},format=yuv420p[v{k}]"
        ));
        concat_in += &format!("[v{k}]");
        if with_audio {
            chains.push(piece_audio(k, src, piece.len(), p.state.volume));
            concat_in += &format!("[a{k}]");
        }
    }
    let n = pieces.len();
    let outs = if with_audio { "[vc][a]" } else { "[vc]" };
    let mut g = chains.join(";");
    g += &format!(";{concat_in}concat=n={n}:v=1:a={}{outs}", u8::from(with_audio));
    let mut post = Vec::new();
    let mut base = (fw / 2 * 2, fh / 2 * 2);
    if let (Some(c), true) = (p.state.crop, apply_crop) {
        let (x, y, w, h) = c.pixels(fw, fh);
        post.push(format!("crop={w}:{h}:{x}:{y}"));
        base = (w, h);
    }
    let (cw, ch) = base;
    if (out_w, out_h) != (cw, ch) {
        post.push(format!("scale={out_w}:{out_h}"));
    }
    if post.is_empty() {
        post.push("null".into());
    }
    g += &format!(";[vc]{}[v]", post.join(","));
    (inputs, g)
}

/// Pieces of the timeline from time `from` onwards.
pub fn pieces_from(p: &Project, from: f64) -> Vec<Piece> {
    let Some((i, src_t)) = p.locate(from) else { return Vec::new() };
    let mut out = Vec::new();
    let first = p.state.pieces[i];
    if first.end - src_t > 0.02 {
        out.push(Piece { start: src_t, ..first });
    }
    out.extend_from_slice(&p.state.pieces[i + 1..]);
    out
}

/// Frame-exact export, re-encoding video with `video_args` (from `Encoder::args`).
pub fn export_args(p: &Project, video_args: &[String], audio_kbps: u32, out: &Path) -> Vec<String> {
    let (w, h) = p.output_size();
    let with_audio = !p.state.mute;
    let (inputs, g) = graph(p, &p.state.pieces, (w, h), p.fps(), with_audio, true);
    let mut a: Vec<String> =
        ["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-progress", "pipe:1", "-nostats"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    a.extend(inputs);
    a.extend(["-filter_complex".into(), g, "-map".into(), "[v]".into()]);
    if with_audio {
        a.extend(["-map".into(), "[a]".into(), "-c:a".into(), "aac".into(), "-b:a".into(), format!("{audio_kbps}k")]);
    }
    a.extend(video_args.iter().cloned());
    a.extend(["-movflags".into(), "+faststart".into(), "-f".into(), "mp4".into(), out.to_string_lossy().into()]);
    a
}

/// Lossless step 1: copy one piece out of its clip (no re-encode). The copy starts at the
/// keyframe at or before the piece's start, so it can begin up to one keyframe interval early.
pub fn lossless_piece_args(p: &Project, k: usize, out_ts: &Path) -> Vec<String> {
    let piece = p.state.pieces[k];
    let src = &p.sources[piece.source];
    let mut a: Vec<String> =
        ["-hide_banner", "-nostdin", "-y", "-loglevel", "error"].iter().map(|s| s.to_string()).collect();
    a.extend([
        "-ss".into(),
        f(piece.start),
        "-i".into(),
        src.path.to_string_lossy().into(),
        "-t".into(),
        f(piece.len()),
    ]);
    a.extend(["-map".into(), "0:v:0".into()]);
    if p.state.mute {
        a.push("-an".into());
    } else {
        a.extend(["-map".into(), "0:a?".into()]);
    }
    a.extend(["-c", "copy", "-avoid_negative_ts", "make_zero", "-f", "mpegts"].iter().map(|s| s.to_string()));
    a.push(out_ts.to_string_lossy().into());
    a
}

/// Lossless step 2: join the copied pieces (listed in a concat file) into an MP4.
pub fn lossless_join_args(list: &Path, out: &Path) -> Vec<String> {
    let mut a: Vec<String> =
        ["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    a.push(list.to_string_lossy().into());
    a.extend(["-map", "0", "-c", "copy", "-movflags", "+faststart", "-f", "mp4"].iter().map(|s| s.to_string()));
    a.push(out.to_string_lossy().into());
    a
}

/// Preview video from timeline time `from`: raw RGBA frames of `w`×`h` at `fps` on stdout.
/// Always the full frame (the editor shows the crop itself, so the crop box can be adjusted).
pub fn preview_video_args(p: &Project, from: f64, w: u32, h: u32, fps: f64) -> Vec<String> {
    let pieces = pieces_from(p, from);
    if pieces.is_empty() {
        return Vec::new();
    }
    let (inputs, g) = graph(p, &pieces, (w, h), fps, false, false);
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error"].iter().map(|s| s.to_string()).collect();
    a.extend(inputs);
    a.extend(["-filter_complex".into(), g, "-map".into(), "[v]".into()]);
    a.extend(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"].iter().map(|s| s.to_string()));
    a
}

/// Preview audio from timeline time `from`: interleaved f32 at `rate` Hz, `channels` on stdout.
pub fn preview_audio_args(p: &Project, from: f64, rate: u32, channels: u32) -> Vec<String> {
    let pieces = pieces_from(p, from);
    if pieces.is_empty() || p.state.mute {
        return Vec::new();
    }
    let (inputs, g) = graph(p, &pieces, (64, 64), 10.0, true, false);
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error"].iter().map(|s| s.to_string()).collect();
    a.extend(inputs);
    a.extend(["-filter_complex".into(), g, "-map".into(), "[a]".into()]);
    a.extend([
        "-ac".to_string(),
        channels.to_string(),
        "-ar".into(),
        rate.to_string(),
        "-f".into(),
        "f32le".into(),
        "pipe:1".into(),
    ]);
    a
}

/// One frame of `path` at `t` seconds, scaled to `w`×`h`, as raw RGBA on stdout.
pub fn frame_args(path: &Path, t: f64, w: u32, h: u32, fast: bool) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error"].iter().map(|s| s.to_string()).collect();
    a.extend(["-ss".into(), f(t)]);
    if fast {
        a.push("-noaccurate_seek".into());
    }
    a.extend(["-i".into(), path.to_string_lossy().into()]);
    a.extend(
        ["-frames:v", "1", "-vf"]
            .iter()
            .map(|s| s.to_string())
            .chain([format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2")]),
    );
    a.extend(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"].iter().map(|s| s.to_string()));
    a
}

/// Mono 16-bit audio at `rate` Hz (all tracks mixed) for the timeline waveform.
pub fn waveform_args(src: &Source, rate: u32) -> Vec<String> {
    if src.audio_streams == 0 {
        return Vec::new();
    }
    let mut a: Vec<String> =
        ["-hide_banner", "-nostdin", "-loglevel", "error", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(src.path.to_string_lossy().into());
    if src.audio_streams > 1 {
        let ins: String = (0..src.audio_streams).map(|j| format!("[0:a:{j}]")).collect();
        a.extend(["-filter_complex".into(), format!("{ins}amix=inputs={}:normalize=0[m]", src.audio_streams)]);
        a.extend(["-map".into(), "[m]".into()]);
    } else {
        a.extend(["-map".into(), "0:a:0".into()]);
    }
    a.extend([
        "-ac".to_string(),
        "1".into(),
        "-ar".into(),
        rate.to_string(),
        "-f".into(),
        "s16le".into(),
        "pipe:1".into(),
    ]);
    a
}

/// Peak level (0..1) per bucket of `per_bucket` samples.
pub fn peaks(samples: &[i16], per_bucket: usize) -> Vec<f32> {
    samples
        .chunks(per_bucket.max(1))
        .map(|c| c.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0) as f32 / 32768.0)
        .collect()
}

/// Parse `out_time_us=` / `out_time_ms=` from FFmpeg's `-progress` output (seconds).
pub fn progress_seconds(line: &str) -> Option<f64> {
    let v = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms="))?;
    v.trim().parse::<i64>().ok().filter(|n| *n >= 0).map(|n| n as f64 / 1e6)
}

/// Read a clip's duration, size, frame rate, codec and audio tracks from `ffmpeg -i` output.
pub fn parse_source(path: &Path, stderr: &str) -> Option<Source> {
    let r = crate::verify::parse_decode(stderr);
    let codec = r.video_codec?;
    (r.width > 0 && r.duration > 0.0).then(|| Source {
        path: path.to_path_buf(),
        duration: r.duration,
        width: r.width,
        height: r.height,
        fps: if r.fps > 0.0 { r.fps } else { 30.0 },
        video_codec: codec,
        audio_streams: r.audio_streams,
    })
}

// ------------------------------------------------------------------ running FFmpeg

/// Read a clip's details with `ffmpeg -i`.
pub fn probe_source(ffmpeg: &Path, path: &Path) -> Result<Source, String> {
    let args: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into(), "-i".into(), path.to_string_lossy().into()];
    let out = crate::process::quiet_command(ffmpeg).args(&args).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stderr);
    parse_source(path, &text).ok_or_else(|| {
        if !path.exists() {
            "the file was moved or deleted".to_string()
        } else {
            "it isn't a video uRec can read".to_string()
        }
    })
}

/// Run FFmpeg and return its stdout bytes (frames, waveforms). None on failure.
pub fn read_stdout(ffmpeg: &Path, args: &[String], expect: Option<usize>) -> Option<Vec<u8>> {
    let out = crate::process::quiet_command(ffmpeg)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    match expect {
        Some(n) if out.stdout.len() < n => None,
        Some(n) => Some(out.stdout[..n].to_vec()),
        None => Some(out.stdout),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportMode {
    /// Re-encode: frame-exact cuts, crop and volume.
    Exact,
    /// Stream copy: no quality loss and very fast; cuts start at the keyframe at or before.
    Lossless,
}

/// Render the project to `out` (or a free " (n)" variant; nothing is ever overwritten).
/// `progress(done_seconds, total_seconds)`; `cancel` stops it and removes the partial file.
/// The result is checked as a complete MP4 before it's returned.
/// What to export and how.
pub struct ExportJob<'a> {
    pub mode: ExportMode,
    /// Encoder arguments from `Encoder::args` (unused for lossless).
    pub video_args: &'a [String],
    pub audio_kbps: u32,
    pub out: &'a Path,
}

pub fn run_export(
    ffmpeg: &Path,
    p: &Project,
    job: &ExportJob,
    progress: &mut dyn FnMut(f64, f64),
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<PathBuf, String> {
    use std::io::BufRead;
    use std::sync::atomic::Ordering;
    let (mode, video_args, audio_kbps, out) = (job.mode, job.video_args, job.audio_kbps, job.out);
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = crate::replay::temp_path(out);
    let list = tmp.with_extension("txt");
    let stage = tmp.with_extension("parts");
    let args = match mode {
        ExportMode::Exact => export_args(p, video_args, audio_kbps, &tmp),
        ExportMode::Lossless => {
            p.lossless_possible().map_err(|e| format!("A lossless export isn't possible: {e}."))?;
            std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
            let total = p.duration();
            let mut done = 0.0;
            let mut files = Vec::new();
            for k in 0..p.state.pieces.len() {
                if cancel.load(Ordering::Relaxed) {
                    let _ = std::fs::remove_dir_all(&stage);
                    return Err("Export cancelled.".into());
                }
                let ts = stage.join(format!("{k:04}.ts"));
                let (ok, err) = crate::process::run_capture(
                    ffmpeg,
                    &lossless_piece_args(p, k, &ts),
                    std::time::Duration::from_secs(600),
                )
                .map_err(|e| e.to_string())?;
                if !ok {
                    let _ = std::fs::remove_dir_all(&stage);
                    return Err(format!("Export failed: {}", err.lines().last().unwrap_or("FFmpeg failed").trim()));
                }
                files.push(ts);
                done += p.state.pieces[k].len();
                progress(done * 0.95, total);
            }
            std::fs::write(&list, crate::replay::concat_list(&files)).map_err(|e| e.to_string())?;
            lossless_join_args(&list, &tmp)
        }
    };
    let total = p.duration();
    let cleanup = || {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&list);
        let _ = std::fs::remove_dir_all(&stage);
    };
    let mut child = crate::process::quiet_command(ffmpeg)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stderr = child.stderr.take().expect("piped");
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stderr, &mut s);
        s
    });
    let stdout = child.stdout.take().expect("piped");
    let (tx, rx) = std::sync::mpsc::channel::<f64>();
    let out_reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(t) = progress_seconds(&line) {
                let _ = tx.send(t);
            }
        }
    });
    let status = loop {
        while let Ok(t) = rx.try_recv() {
            progress(t.min(total), total);
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_reader.join();
            cleanup();
            return Err("Export cancelled.".into());
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) => {
                cleanup();
                return Err(e.to_string());
            }
        }
    };
    let _ = out_reader.join();
    let err = err_reader.join().unwrap_or_default();
    let _ = std::fs::remove_file(&list);
    let _ = std::fs::remove_dir_all(&stage);
    if !status.success() {
        cleanup();
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("FFmpeg failed").trim().to_string();
        return Err(format!("Export failed: {last}"));
    }
    if let Err(e) = crate::mp4check::check(&tmp) {
        cleanup();
        return Err(format!("The export came out damaged ({e}) and was discarded."));
    }
    progress(total, total);
    crate::replay::place_no_overwrite(&tmp, out).map_err(|e| {
        cleanup();
        e.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(name: &str, dur: f64, audio: usize) -> Source {
        Source {
            path: PathBuf::from(format!("C:/v/{name}.mp4")),
            duration: dur,
            width: 1920,
            height: 1080,
            fps: 60.0,
            video_codec: "h264".into(),
            audio_streams: audio,
        }
    }

    #[test]
    fn cut_this_this_and_this() {
        let mut p = Project::new(vec![src("a", 60.0, 1)]);
        assert_eq!(p.duration(), 60.0);
        // Cut out 10-20 s and 40-45 s.
        assert!(p.split_at(10.0) && p.split_at(20.0) && p.split_at(40.0) && p.split_at(45.0));
        assert_eq!(p.pieces().len(), 5);
        assert!(p.delete(3)); // 40-45
        assert!(p.delete(1)); // 10-20
        let kept: Vec<(f64, f64)> = p.pieces().iter().map(|x| (x.start, x.end)).collect();
        assert_eq!(kept, vec![(0.0, 10.0), (20.0, 40.0), (45.0, 60.0)]);
        assert!((p.duration() - 45.0).abs() < 1e-9);
        // Timeline 15 s is source 25 s of the second piece.
        assert_eq!(p.locate(15.0), Some((1, 25.0)));
        assert_eq!(p.starts(), vec![0.0, 10.0, 30.0]);
    }

    #[test]
    fn undo_redo_and_limits() {
        let mut p = Project::new(vec![src("a", 30.0, 1)]);
        assert!(!p.split_at(0.05), "too close to the start");
        assert!(!p.delete(0), "the last piece stays");
        assert!(p.split_at(12.0));
        assert!(p.delete(0));
        assert_eq!(p.pieces()[0].start, 12.0);
        assert!(p.undo() && p.undo());
        assert_eq!(p.pieces().len(), 1);
        assert!(!p.undo());
        assert!(p.redo());
        assert_eq!(p.pieces().len(), 2);
        // A new edit clears redo.
        assert!(p.split_at(5.0));
        assert!(!p.can_redo());
    }

    #[test]
    fn trim_reorder_and_drag_is_one_undo_step() {
        let mut p = Project::new(vec![src("a", 30.0, 1), src("b", 10.0, 0)]);
        assert_eq!(p.duration(), 40.0);
        let before = p.state.clone();
        for v in [2.0, 3.0, 4.0] {
            p.set_edge(0, true, v, false);
        }
        p.set_edge(0, false, 99.0, false); // clamped to the source
        p.commit_drag(before);
        assert_eq!((p.pieces()[0].start, p.pieces()[0].end), (4.0, 30.0));
        assert!(p.undo());
        assert_eq!((p.pieces()[0].start, p.pieces()[0].end), (0.0, 30.0));
        assert!(!p.set_edge(0, true, 29.99, true) || p.pieces()[0].len() >= MIN_PIECE);
        assert!(p.move_piece(1, 0));
        assert_eq!(p.pieces()[0].source, 1);
    }

    #[test]
    fn crop_rectangles() {
        let c = Crop::centered(9.0 / 16.0, 1920, 1080);
        let (x, y, w, h) = c.pixels(1920, 1080);
        assert_eq!((w, h, y), (608, 1080, 0));
        assert_eq!(x, 656);
        assert_eq!(Crop::centered(16.0 / 9.0, 1920, 1080), Crop::FULL);
        let sq = Crop::centered(1.0, 1920, 1080).pixels(1920, 1080);
        assert_eq!((sq.2, sq.3), (1080, 1080));
        // Out-of-range input is pulled back inside, sizes stay even.
        let (x, y, w, h) = Crop { x: 0.9, y: -1.0, w: 0.5, h: 3.0 }.pixels(1001, 777);
        assert!(x + w <= 1001 && y + h <= 777 && w % 2 == 0 && h % 2 == 0);
        let mut p = Project::new(vec![src("a", 10.0, 1)]);
        p.set_crop(Some(Crop::centered(9.0 / 16.0, 1920, 1080)));
        assert_eq!(p.output_size(), (608, 1080));
        p.set_crop(Some(Crop::FULL));
        assert_eq!(p.state.crop, None, "a full-frame crop is no crop");
    }

    #[test]
    fn export_command() {
        let mut p = Project::new(vec![src("a", 60.0, 2), src("b", 20.0, 0)]);
        p.split_at(10.0);
        p.delete(0); // keep a 10-60 and b 0-20
        p.set_crop(Some(Crop::centered(9.0 / 16.0, 1920, 1080)));
        let v = vec!["-c:v".to_string(), "h264_amf".into()];
        let a = export_args(&p, &v, 160, Path::new("C:/out/x.mp4"));
        let j = a.join(" ");
        assert!(j.contains("-ss 10.000 -t 50.000 -i C:/v/a.mp4"), "{j}");
        assert!(j.contains("-ss 0.000 -t 20.000 -i C:/v/b.mp4"));
        let g = &a[a.iter().position(|x| x == "-filter_complex").unwrap() + 1];
        // Two tracks of clip a are mixed; clip b (no sound) gets silence of its length.
        assert!(g.contains("[0:a:0][0:a:1]amix=inputs=2"), "{g}");
        assert!(g.contains("anullsrc=r=48000:cl=stereo:d=20.000"));
        assert!(g.contains("concat=n=2:v=1:a=1[vc][a]"));
        assert!(g.contains("[vc]crop=608:1080:656:0[v]"));
        assert!(j.contains("-map [v] -map [a] -c:a aac -b:a 160k -c:v h264_amf"));
        assert!(j.ends_with("-movflags +faststart -f mp4 C:/out/x.mp4"));
        // Muted: no audio graph at all.
        p.set_mute(true);
        let m = export_args(&p, &v, 160, Path::new("o.mp4")).join(" ");
        assert!(!m.contains("[a]") && !m.contains("anullsrc") && m.contains("concat=n=2:v=1:a=0[vc]"));
    }

    #[test]
    fn lossless_rules() {
        let mut p = Project::new(vec![src("a", 60.0, 1)]);
        p.split_at(10.0);
        p.split_at(20.0);
        p.delete(1);
        assert!(p.lossless_possible().is_ok());
        let piece = lossless_piece_args(&p, 1, Path::new("p1.ts")).join(" ");
        assert!(
            piece.contains("-ss 20.000 -i C:/v/a.mp4 -t 40.000")
                && piece.contains("-c copy")
                && piece.ends_with("mpegts p1.ts")
        );
        assert!(lossless_join_args(Path::new("l.txt"), Path::new("o.mp4"))
            .join(" ")
            .contains("-f concat -safe 0 -i l.txt -map 0 -c copy"));
        p.set_volume(1.5, true);
        assert!(p.lossless_possible().is_err());
        p.set_volume(1.0, true);
        p.set_crop(Some(Crop::centered(1.0, 1920, 1080)));
        assert!(p.lossless_possible().is_err());
        let mut mixed = Project::new(vec![src("a", 5.0, 1), Source { width: 1280, ..src("b", 5.0, 1) }]);
        assert!(mixed.lossless_possible().is_err());
        mixed.delete(1);
        assert!(mixed.lossless_possible().is_ok(), "only clips still on the timeline count");
    }

    #[test]
    fn preview_starts_at_the_playhead() {
        let mut p = Project::new(vec![src("a", 30.0, 1)]);
        p.split_at(10.0);
        p.delete(0);
        let pieces = pieces_from(&p, 5.0);
        assert_eq!(pieces, vec![Piece { source: 0, start: 15.0, end: 30.0 }]);
        let a = preview_video_args(&p, 5.0, 640, 360, 30.0).join(" ");
        assert!(a.contains("-ss 15.000 -t 15.000") && a.contains("scale=640:360") && a.contains("rawvideo"));
        let au = preview_audio_args(&p, 5.0, 48000, 2).join(" ");
        assert!(au.contains("-ac 2 -ar 48000 -f f32le"));
        p.set_mute(true);
        assert!(preview_audio_args(&p, 0.0, 48000, 2).is_empty());
    }

    #[test]
    fn helpers() {
        assert_eq!(progress_seconds("out_time_us=2500000"), Some(2.5));
        assert_eq!(progress_seconds("out_time_us=N/A"), None);
        assert_eq!(peaks(&[0, 16384, -32768, 100], 2), vec![0.5, 1.0]);
        let s = "Input #0, mov,mp4, from 'x.mp4':\n  Duration: 00:00:12.50, start: 0.000000, bitrate: 1 kb/s\n  Stream #0:0: Video: hevc (Main), yuv420p, 2560x1440, 60 fps, 60 tbr\n  Stream #0:1: Audio: aac, 48000 Hz, stereo\n  Stream #0:2: Audio: aac, 48000 Hz, stereo\n";
        let src = parse_source(Path::new("x.mp4"), s).unwrap();
        assert_eq!((src.width, src.height, src.audio_streams), (2560, 1440, 2));
        assert_eq!(src.video_codec, "hevc");
        assert!((src.duration - 12.5).abs() < 1e-9);
        let fa = frame_args(Path::new("x.mp4"), 3.0, 320, 180, true).join(" ");
        assert!(fa.contains("-ss 3.000 -noaccurate_seek -i x.mp4 -frames:v 1"));
    }
}
