//! Builds the FFmpeg command line for a recording: capture source, GPU or
//! system-memory frame path, scaling, encoder, audio inputs and output.

use crate::encoder::Encoder;
use crate::video::VideoSettings;

/// How frames are grabbed from the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CaptureMethod {
    /// Windows Graphics Capture (Windows 10 1903+). Monitors and individual windows, GPU frames.
    Wgc,
    /// Desktop Duplication API (Windows 8+). Monitors only, GPU frames.
    Dda,
    /// GDI BitBlt. Works everywhere, including VMs and remote sessions; slowest.
    Gdi,
    /// Synthetic test pattern, for development and automated tests.
    Test,
}

impl CaptureMethod {
    pub fn label(&self) -> &'static str {
        match self {
            CaptureMethod::Wgc => "Windows Graphics Capture",
            CaptureMethod::Dda => "Desktop Duplication",
            CaptureMethod::Gdi => "GDI (compatibility)",
            CaptureMethod::Test => "Test pattern",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            CaptureMethod::Wgc => "wgc",
            CaptureMethod::Dda => "dda",
            CaptureMethod::Gdi => "gdi",
            CaptureMethod::Test => "test",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        [CaptureMethod::Wgc, CaptureMethod::Dda, CaptureMethod::Gdi, CaptureMethod::Test]
            .into_iter()
            .find(|m| m.id() == s)
    }

    /// Produces Direct3D 11 GPU frames.
    pub fn gpu_frames(&self) -> bool {
        matches!(self, CaptureMethod::Wgc | CaptureMethod::Dda)
    }

    /// Fallback order for a target.
    pub fn order_for(target: &CaptureTarget) -> Vec<CaptureMethod> {
        match target {
            CaptureTarget::Window { .. } => vec![CaptureMethod::Wgc, CaptureMethod::Gdi],
            _ => vec![CaptureMethod::Wgc, CaptureMethod::Dda, CaptureMethod::Gdi],
        }
    }
}

/// A rectangle in virtual-desktop pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// What to record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureTarget {
    /// A whole monitor. `index` is the DXGI output / enumeration index, `bounds` its desktop rectangle.
    Monitor { index: u32, hmonitor: u64, bounds: Rect },
    /// A single application window.
    Window { hwnd: u64, title: String, bounds: Rect },
    /// Part of a monitor; `region` is relative to the monitor's top-left corner.
    Region { index: u32, hmonitor: u64, bounds: Rect, region: Rect },
}

impl CaptureTarget {
    /// Size of the captured image before scaling.
    pub fn source_size(&self) -> (u32, u32) {
        match self {
            CaptureTarget::Monitor { bounds, .. } | CaptureTarget::Window { bounds, .. } => {
                (bounds.width, bounds.height)
            }
            CaptureTarget::Region { region, .. } => (region.width, region.height),
        }
    }
}

/// An audio stream fed to FFmpeg as raw 32-bit float PCM.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioInput {
    /// Named pipe path (`\\.\pipe\...`), a FIFO, or a lavfi source when `lavfi` is set.
    pub source: String,
    pub lavfi: bool,
    pub sample_rate: u32,
    pub channels: u16,
    /// Track title written into the file ("Game & desktop", "Microphone", ...).
    pub title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    /// Fragmented MP4: plays everywhere and stays playable if the PC crashes mid-recording.
    Mp4,
    /// Matroska: most robust, fewer players/editors support it.
    Mkv,
}

impl Container {
    pub fn extension(&self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    /// A single recording file.
    File { path: String, container: Container },
    /// Rolling instant-replay buffer of short MPEG-TS segments.
    ReplayRing { dir: String, segment_seconds: u32, wrap: u32 },
}

/// Which way frames travel from capture to encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramePath {
    /// Frames stay on the GPU (no copies through system memory).
    Gpu,
    /// Frames are downloaded/converted on the CPU. Always works.
    System,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub method: CaptureMethod,
    pub target: CaptureTarget,
    pub cursor: bool,
    pub encoder: Encoder,
    pub video: VideoSettings,
    pub out_width: u32,
    pub out_height: u32,
    pub fps: u32,
    pub frame_path: FramePath,
    pub audio: Vec<AudioInput>,
    pub audio_kbps: u32,
    pub output: Output,
}

impl Plan {
    /// Whether the GPU path is possible for this method + encoder.
    pub fn gpu_path_possible(method: CaptureMethod, encoder: &Encoder) -> bool {
        method.gpu_frames() && (encoder.accepts_d3d11() || encoder.needs_qsv_frames())
    }

    fn needs_resize(&self) -> bool {
        let (w, h) = self.target.source_size();
        (w, h) != (self.out_width, self.out_height) || matches!(self.target, CaptureTarget::Window { .. })
    }

    /// FFmpeg input arguments for the video source.
    fn video_input(&self) -> Vec<String> {
        let fps = self.fps;
        let w = self.out_width;
        let h = self.out_height;
        let cursor = u8::from(self.cursor);
        let mut a = Vec::new();
        match self.method {
            CaptureMethod::Wgc => {
                let mut f = String::from("gfxcapture=");
                match &self.target {
                    CaptureTarget::Monitor { index, hmonitor, .. } | CaptureTarget::Region { index, hmonitor, .. } => {
                        if *hmonitor != 0 {
                            f += &format!("hmonitor={hmonitor}");
                        } else {
                            f += &format!("monitor_idx={index}");
                        }
                    }
                    CaptureTarget::Window { hwnd, .. } => f += &format!("hwnd={hwnd}:capture_border=0"),
                }
                if let CaptureTarget::Region { bounds, region, .. } = &self.target {
                    let right = bounds.width as i64 - region.x as i64 - region.width as i64;
                    let bottom = bounds.height as i64 - region.y as i64 - region.height as i64;
                    f += &format!(
                        ":crop_left={}:crop_top={}:crop_right={}:crop_bottom={}",
                        region.x.max(0),
                        region.y.max(0),
                        right.max(0),
                        bottom.max(0)
                    );
                }
                f += &format!(":max_framerate={fps}:capture_cursor={cursor}");
                // Let the capture API scale on the GPU, keeping aspect ratio (black bars if needed).
                if self.needs_resize() {
                    f += &format!(":width={w}:height={h}:resize_mode=scale_aspect");
                }
                a.extend(["-f", "lavfi", "-i"].map(String::from));
                a.push(f);
            }
            CaptureMethod::Dda => {
                let (index, region) = match &self.target {
                    CaptureTarget::Monitor { index, .. } => (*index, None),
                    CaptureTarget::Region { index, region, .. } => (*index, Some(*region)),
                    CaptureTarget::Window { .. } => (0, None),
                };
                let mut f = format!("ddagrab=output_idx={index}:framerate={fps}:draw_mouse={cursor}");
                if let Some(r) = region {
                    f += &format!(":video_size={}x{}:offset_x={}:offset_y={}", r.width, r.height, r.x, r.y);
                }
                a.extend(["-f", "lavfi", "-i"].map(String::from));
                a.push(f);
            }
            CaptureMethod::Gdi => {
                a.extend(["-thread_queue_size", "512", "-f", "gdigrab"].map(String::from));
                a.extend(["-framerate".to_string(), fps.to_string(), "-draw_mouse".into(), cursor.to_string()]);
                match &self.target {
                    CaptureTarget::Window { title, .. } => {
                        a.extend(["-i".to_string(), format!("title={title}")]);
                    }
                    CaptureTarget::Monitor { bounds, .. } => {
                        a.extend(gdi_rect(*bounds));
                        a.extend(["-i", "desktop"].map(String::from));
                    }
                    CaptureTarget::Region { bounds, region, .. } => {
                        let abs = Rect { x: bounds.x + region.x, y: bounds.y + region.y, ..*region };
                        a.extend(gdi_rect(abs));
                        a.extend(["-i", "desktop"].map(String::from));
                    }
                }
            }
            CaptureMethod::Test => {
                let (sw, sh) = self.target.source_size();
                a.extend(["-re", "-f", "lavfi", "-i"].map(String::from));
                a.push(format!("testsrc2=size={sw}x{sh}:rate={fps}"));
            }
        }
        a
    }

    /// The `-vf` filter chain between capture and encoder.
    fn video_filter(&self) -> String {
        let w = self.out_width;
        let h = self.out_height;
        let gpu_in = self.method.gpu_frames();
        // WGC already scaled on the GPU; other methods need a scaler when sizes differ.
        let scaled_by_capture = self.method == CaptureMethod::Wgc;
        let resize = self.needs_resize() && !scaled_by_capture;
        let mut chain: Vec<String> = Vec::new();
        match self.frame_path {
            FramePath::Gpu => {
                if self.encoder.needs_qsv_frames() {
                    chain.push(format!("scale_d3d11=width={w}:height={h}:format=nv12"));
                    chain.push("hwmap=derive_device=qsv".into());
                    chain.push("format=qsv".into());
                } else if resize || self.encoder.backend == crate::encoder::Backend::MediaFoundationHw {
                    chain.push(format!("scale_d3d11=width={w}:height={h}:format=nv12"));
                }
                // NVENC/AMF take the BGRA D3D11 frames as they are: no conversion at all.
            }
            FramePath::System => {
                if gpu_in {
                    chain.push("hwdownload".into());
                    chain.push("format=bgra".into());
                }
                if resize {
                    chain.push(format!(
                        "scale={w}:{h}:flags=bilinear:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2"
                    ));
                }
                chain.push(format!("format={}", self.encoder.sw_pix_fmt()));
            }
        }
        chain.join(",")
    }

    /// Complete FFmpeg argument list (without the program name).
    pub fn ffmpeg_args(&self) -> Vec<String> {
        let mut a: Vec<String> = ["-hide_banner", "-loglevel", "warning", "-y"].map(String::from).to_vec();
        a.extend(["-progress", "pipe:1", "-stats_period", "0.5", "-nostats"].map(String::from));
        a.extend(self.video_input());
        for input in &self.audio {
            if input.lavfi {
                a.extend(["-f", "lavfi", "-i"].map(String::from));
                a.push(input.source.clone());
            } else {
                a.extend(["-thread_queue_size", "2048", "-f", "f32le"].map(String::from));
                a.extend(["-ar".to_string(), input.sample_rate.to_string(), "-ac".into(), input.channels.to_string()]);
                a.extend(["-i".to_string(), input.source.clone()]);
            }
        }
        a.extend(["-map", "0:v"].map(String::from));
        for i in 0..self.audio.len() {
            a.extend(["-map".to_string(), format!("{}:a", i + 1)]);
        }
        let vf = self.video_filter();
        if !vf.is_empty() {
            a.extend(["-vf".to_string(), vf]);
        }
        // Constant frame rate output: duplicates frames when the screen is static.
        a.extend(["-fps_mode".to_string(), "cfr".into(), "-r".into(), self.fps.to_string()]);
        a.extend(self.encoder.args(&self.video, self.fps));
        if !self.audio.is_empty() {
            a.extend(["-c:a".to_string(), "aac".into(), "-b:a".into(), format!("{}k", self.audio_kbps)]);
            for (i, input) in self.audio.iter().enumerate() {
                a.extend([format!("-metadata:s:a:{i}"), format!("title={}", input.title)]);
            }
        }
        match &self.output {
            Output::File { path, container } => {
                match container {
                    Container::Mp4 => a.extend(
                        ["-movflags", "+frag_keyframe+empty_moov+default_base_moof", "-f", "mp4"].map(String::from),
                    ),
                    Container::Mkv => a.extend(["-f", "matroska"].map(String::from)),
                }
                a.push(path.clone());
            }
            Output::ReplayRing { dir, segment_seconds, wrap } => {
                let list_size = wrap.saturating_sub(2).max(1);
                a.extend(["-f", "segment", "-segment_format", "mpegts", "-reset_timestamps", "1"].map(String::from));
                a.extend(["-segment_time".to_string(), segment_seconds.to_string()]);
                a.extend(["-segment_wrap".to_string(), wrap.to_string()]);
                a.extend(["-segment_list".to_string(), join_path(dir, "segments.csv")]);
                a.extend(["-segment_list_type".to_string(), "csv".into()]);
                a.extend(["-segment_list_size".to_string(), list_size.to_string()]);
                a.push(join_path(dir, "seg_%05d.ts"));
            }
        }
        a
    }
}

fn gdi_rect(r: Rect) -> Vec<String> {
    vec![
        "-offset_x".into(),
        r.x.to_string(),
        "-offset_y".into(),
        r.y.to_string(),
        "-video_size".into(),
        format!("{}x{}", r.width, r.height),
    ]
}

pub(crate) fn join_path(dir: &str, file: &str) -> String {
    let sep = if dir.contains('\\') { '\\' } else { '/' };
    if dir.ends_with(['/', '\\']) {
        format!("{dir}{file}")
    } else {
        format!("{dir}{sep}{file}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::Backend;
    use crate::video::Codec;

    fn monitor() -> CaptureTarget {
        CaptureTarget::Monitor { index: 0, hmonitor: 0, bounds: Rect { x: 0, y: 0, width: 2560, height: 1440 } }
    }

    fn plan(method: CaptureMethod, backend: Backend, path: FramePath, out: (u32, u32)) -> Plan {
        Plan {
            method,
            target: monitor(),
            cursor: true,
            encoder: Encoder::new(backend, Codec::H264),
            video: VideoSettings::default(),
            out_width: out.0,
            out_height: out.1,
            fps: 60,
            frame_path: path,
            audio: vec![],
            audio_kbps: 160,
            output: Output::File { path: "C:\\out.mp4".into(), container: Container::Mp4 },
        }
    }

    fn vf(p: &Plan) -> String {
        let a = p.ffmpeg_args();
        a.iter().position(|x| x == "-vf").map(|i| a[i + 1].clone()).unwrap_or_default()
    }

    #[test]
    fn zero_copy_native_has_no_filters() {
        let p = plan(CaptureMethod::Dda, Backend::Amf, FramePath::Gpu, (2560, 1440));
        assert_eq!(vf(&p), "");
        assert!(p.ffmpeg_args().join(" ").contains("ddagrab=output_idx=0:framerate=60:draw_mouse=1"));
    }

    #[test]
    fn gpu_scaling_stays_on_gpu() {
        let p = plan(CaptureMethod::Dda, Backend::Nvenc, FramePath::Gpu, (1920, 1080));
        assert_eq!(vf(&p), "scale_d3d11=width=1920:height=1080:format=nv12");
        let q = plan(CaptureMethod::Dda, Backend::Qsv, FramePath::Gpu, (1920, 1080));
        assert!(vf(&q).ends_with("hwmap=derive_device=qsv,format=qsv"));
        // WGC scales inside the capture filter
        let w = plan(CaptureMethod::Wgc, Backend::Amf, FramePath::Gpu, (1920, 1080));
        assert_eq!(vf(&w), "");
        assert!(w.ffmpeg_args().join(" ").contains("width=1920:height=1080:resize_mode=scale_aspect"));
    }

    #[test]
    fn system_path_downloads_once() {
        let p = plan(CaptureMethod::Dda, Backend::Software, FramePath::System, (1280, 720));
        let f = vf(&p);
        assert!(f.starts_with("hwdownload,format=bgra,scale=1280:720"));
        assert!(f.ends_with("format=yuv420p"));
        let g = plan(CaptureMethod::Gdi, Backend::Software, FramePath::System, (2560, 1440));
        assert_eq!(vf(&g), "format=yuv420p");
        assert!(g.ffmpeg_args().join(" ").contains(
            "-f gdigrab -framerate 60 -draw_mouse 1 -offset_x 0 -offset_y 0 -video_size 2560x1440 -i desktop"
        ));
    }

    #[test]
    fn region_crops() {
        let mut p = plan(CaptureMethod::Wgc, Backend::Amf, FramePath::Gpu, (800, 600));
        p.target = CaptureTarget::Region {
            index: 1,
            hmonitor: 77,
            bounds: Rect { x: 2560, y: 0, width: 1920, height: 1080 },
            region: Rect { x: 100, y: 50, width: 800, height: 600 },
        };
        let j = p.ffmpeg_args().join(" ");
        assert!(j.contains("gfxcapture=hmonitor=77:crop_left=100:crop_top=50:crop_right=1020:crop_bottom=430"), "{j}");
        p.method = CaptureMethod::Gdi;
        p.frame_path = FramePath::System;
        assert!(p.ffmpeg_args().join(" ").contains("-offset_x 2660 -offset_y 50 -video_size 800x600"));
    }

    #[test]
    fn audio_and_replay_output() {
        let mut p = plan(CaptureMethod::Dda, Backend::Amf, FramePath::Gpu, (2560, 1440));
        p.audio = vec![
            AudioInput {
                source: r"\\.\pipe\urec-sys".into(),
                lavfi: false,
                sample_rate: 48000,
                channels: 2,
                title: "Game & desktop".into(),
            },
            AudioInput {
                source: r"\\.\pipe\urec-mic".into(),
                lavfi: false,
                sample_rate: 48000,
                channels: 1,
                title: "Microphone".into(),
            },
        ];
        p.output = Output::ReplayRing { dir: r"C:\buf".into(), segment_seconds: 2, wrap: 65 };
        let a = p.ffmpeg_args();
        let j = a.join(" ");
        assert!(j.contains(r"-f f32le -ar 48000 -ac 1 -i \\.\pipe\urec-mic"));
        assert!(j.contains("-map 0:v -map 1:a -map 2:a"));
        assert!(j.contains("-metadata:s:a:1 title=Microphone"));
        assert!(j.contains(r"-segment_list C:\buf\segments.csv"));
        assert_eq!(a.last().unwrap(), r"C:\buf\seg_%05d.ts");
    }
}
