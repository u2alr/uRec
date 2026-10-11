//! WASAPI audio capture (game/desktop loopback + microphone), streamed to FFmpeg as
//! 48 kHz stereo float PCM through named pipes.
//!
//! - Windows converts every device to 48 kHz stereo float for us (AUTOCONVERTPCM), so
//!   mixing is a simple sum. If a driver refuses, we take its mix format and convert.
//! - Loopback delivers nothing while no sound is playing, so each pipe writer runs on its
//!   own clock and fills gaps with silence. That keeps audio locked to real time (A/V sync).
//! - Rings are bounded (0.5 s); a source never grows memory without limit.

use crate::sys::{wide, Win32Ok};
use crate::{AudioDevice, AudioSettings, SourceState, TrackLayout};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use urec_engine::pipeline::AudioInput;
use windows::core::PCWSTR;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_OUTBOUND,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT};

const RATE: u32 = 48_000;
const CH: usize = 2;
const RING_MAX_FRAMES: usize = RATE as usize / 2;

fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

fn device_name(dev: &IMMDevice) -> String {
    unsafe {
        dev.OpenPropertyStore(STGM_READ)
            .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
            .map(|v| v.to_string())
            .unwrap_or_else(|_| "Audio device".into())
    }
}

fn device_id(dev: &IMMDevice) -> String {
    unsafe {
        match dev.GetId() {
            Ok(p) => {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                s
            }
            Err(_) => String::new(),
        }
    }
}

fn list(flow: EDataFlow) -> Vec<AudioDevice> {
    com_init();
    let Ok(en) = enumerator() else { return Vec::new() };
    let default_id = unsafe { en.GetDefaultAudioEndpoint(flow, eConsole) }.map(|d| device_id(&d)).unwrap_or_default();
    let mut out = Vec::new();
    unsafe {
        if let Ok(coll) = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) {
            for i in 0..coll.GetCount().unwrap_or(0) {
                if let Ok(d) = coll.Item(i) {
                    let id = device_id(&d);
                    out.push(AudioDevice { is_default: id == default_id, id, name: device_name(&d) });
                }
            }
        }
    }
    out
}

/// Playback devices (their output can be recorded via loopback).
pub fn audio_outputs() -> Vec<AudioDevice> {
    list(eRender)
}

/// Microphones and other capture devices.
pub fn audio_inputs() -> Vec<AudioDevice> {
    list(eCapture)
}

// ------------------------------------------------------------------ sources

struct Slot {
    ring: Mutex<VecDeque<f32>>,
    volume: AtomicU32,
    muted: AtomicBool,
    peak: AtomicU32,
    state: Mutex<SourceState>,
}

impl Slot {
    fn new(volume: f32) -> Self {
        Slot {
            ring: Mutex::new(VecDeque::with_capacity(RING_MAX_FRAMES * CH)),
            volume: AtomicU32::new(volume.to_bits()),
            muted: AtomicBool::new(false),
            peak: AtomicU32::new(0),
            state: Mutex::new(SourceState::Off),
        }
    }

    fn gain(&self) -> f32 {
        if self.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            f32::from_bits(self.volume.load(Ordering::Relaxed))
        }
    }

    /// Append interleaved stereo frames, applying volume and tracking the peak level.
    fn push(&self, samples: &[f32]) {
        let g = self.gain();
        let mut peak = 0f32;
        let mut ring = self.ring.lock().unwrap();
        for &s in samples {
            let v = s * g;
            peak = peak.max(v.abs());
            ring.push_back(v);
        }
        let excess = ring.len().saturating_sub(RING_MAX_FRAMES * CH);
        ring.drain(..excess);
        drop(ring);
        let old = f32::from_bits(self.peak.load(Ordering::Relaxed));
        if peak > old {
            self.peak.store(peak.to_bits(), Ordering::Relaxed);
        }
    }

    /// Add up to `out.len()` samples into `out` (missing samples stay as they are = silence).
    fn mix_into(&self, out: &mut [f32]) {
        let mut ring = self.ring.lock().unwrap();
        let n = out.len().min(ring.len());
        for (o, s) in out.iter_mut().zip(ring.drain(..n)) {
            *o += s;
        }
    }

    fn set_state(&self, s: SourceState) {
        *self.state.lock().unwrap() = s;
    }
}

/// Converts arbitrary float frames to 48 kHz stereo (only used when AUTOCONVERTPCM is refused).
struct Converter {
    rate: u32,
    channels: usize,
    pos: f64,
    last: [f32; 2],
}

impl Converter {
    fn convert(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let frames = input.len() / self.channels.max(1);
        let stereo = |i: usize| -> [f32; 2] {
            let base = i * self.channels;
            if self.channels == 1 {
                [input[base], input[base]]
            } else {
                [input[base], input[base + 1]]
            }
        };
        if self.rate == RATE {
            for i in 0..frames {
                out.extend_from_slice(&stereo(i));
            }
            return;
        }
        let step = self.rate as f64 / RATE as f64;
        while self.pos < frames as f64 {
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            let a = if i == 0 { self.last } else { stereo(i - 1) };
            let b = stereo(i);
            out.push(a[0] + (b[0] - a[0]) * t);
            out.push(a[1] + (b[1] - a[1]) * t);
            self.pos += step;
        }
        self.pos -= frames as f64;
        if frames > 0 {
            self.last = stereo(frames - 1);
        }
    }
}

struct Opened {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    name: String,
    convert: Option<Converter>,
}

fn open_device(loopback: bool, id: Option<&str>) -> windows::core::Result<Opened> {
    unsafe {
        let en = enumerator()?;
        let flow = if loopback { eRender } else { eCapture };
        let dev = match id {
            Some(id) => {
                let w = wide(id);
                en.GetDevice(PCWSTR(w.as_ptr()))?
            }
            None => en.GetDefaultAudioEndpoint(flow, eConsole)?,
        };
        let name = device_name(&dev);
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let loop_flag = if loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
        let want = WAVEFORMATEX {
            wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
            nChannels: CH as u16,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        let buffer_100ns = 2_000_000; // 200 ms
        let mut convert = None;
        let first = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            loop_flag | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            buffer_100ns,
            0,
            &want,
            None,
        );
        let client = if let Err(first_err) = first {
            // Fall back to the device's own format and convert ourselves.
            let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
            let mix = client.GetMixFormat()?;
            let fmt = &*mix;
            let is_float = fmt.wFormatTag == 3
                || (fmt.wFormatTag == 0xFFFE && fmt.cbSize >= 22 && {
                    let ext = &*(mix as *const WAVEFORMATEXTENSIBLE);
                    let sub = std::ptr::addr_of!(ext.SubFormat).read_unaligned();
                    sub.data1 == 3
                });
            if !is_float || fmt.wBitsPerSample != 32 {
                CoTaskMemFree(Some(mix as *const _));
                return Err(first_err);
            }
            convert = Some(Converter {
                rate: fmt.nSamplesPerSec,
                channels: fmt.nChannels as usize,
                pos: 0.0,
                last: [0.0; 2],
            });
            let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, loop_flag, buffer_100ns, 0, mix, None);
            CoTaskMemFree(Some(mix as *const _));
            r?;
            client
        } else {
            client
        };
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        Ok(Opened { client, capture, name, convert })
    }
}

/// Read everything currently available. Err means the device is gone.
fn drain(o: &mut Opened, slot: &Slot, scratch: &mut Vec<f32>) -> windows::core::Result<()> {
    unsafe {
        while o.capture.GetNextPacketSize()? > 0 {
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            o.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
            let channels = o.convert.as_ref().map(|c| c.channels).unwrap_or(CH);
            let n = frames as usize * channels;
            scratch.clear();
            if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                let silent = vec![0f32; n];
                match o.convert.as_mut() {
                    Some(c) => c.convert(&silent, scratch),
                    None => scratch.extend_from_slice(&silent),
                }
            } else {
                let src = std::slice::from_raw_parts(data as *const f32, n);
                match o.convert.as_mut() {
                    Some(c) => c.convert(src, scratch),
                    None => scratch.extend_from_slice(src),
                }
            }
            o.capture.ReleaseBuffer(frames)?;
            slot.push(scratch);
        }
    }
    Ok(())
}

fn capture_thread(
    loopback: bool,
    id: Option<String>,
    slot: Arc<Slot>,
    stop: Arc<AtomicBool>,
    ready: mpsc::Sender<Result<String, String>>,
) {
    com_init();
    let mut first = true;
    let mut scratch = Vec::with_capacity(RATE as usize);
    while !stop.load(Ordering::Relaxed) {
        let mut opened = match open_device(loopback, id.as_deref()) {
            Ok(o) => o,
            Err(e) => {
                if first {
                    let _ = ready.send(Err(e.message().to_string()));
                    slot.set_state(SourceState::Failed { reason: e.message().to_string() });
                    return;
                }
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        if first {
            first = false;
            let _ = ready.send(Ok(opened.name.clone()));
        }
        slot.set_state(SourceState::Active { device: opened.name.clone() });
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
            if drain(&mut opened, &slot, &mut scratch).is_err() {
                slot.set_state(SourceState::Lost { device: opened.name.clone() });
                break;
            }
        }
        unsafe {
            let _ = opened.client.Stop();
        }
        if !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

// ------------------------------------------------------------------ pipes

struct Pipe(HANDLE);
unsafe impl Send for Pipe {}

static PIPE_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn create_pipe(name: &str) -> Option<Pipe> {
    let w = wide(name);
    let h = unsafe {
        CreateNamedPipeW(PCWSTR(w.as_ptr()), PIPE_ACCESS_OUTBOUND, PIPE_TYPE_BYTE | PIPE_WAIT, 1, 1 << 20, 0, 0, None)
    };
    crate::sys::valid_handle(h).map(Pipe)
}

/// Unblock a writer that is still waiting for FFmpeg to connect.
fn poke_pipe(name: &str) {
    let w = wide(name);
    let h = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    };
    if let Some(h) = crate::sys::valid_handle(h) {
        unsafe {
            let _ = CloseHandle(h);
        }
    }
}

fn writer_thread(pipe: Pipe, slots: Vec<Arc<Slot>>, stop: Arc<AtomicBool>) {
    let h = pipe.0;
    let connected = unsafe { ConnectNamedPipe(h, None) }.succeeded()
        || unsafe { windows::Win32::Foundation::GetLastError() } == windows::Win32::Foundation::ERROR_PIPE_CONNECTED;
    if connected && !stop.load(Ordering::Relaxed) {
        // Start the clock now and drop anything captured before FFmpeg was listening.
        for s in &slots {
            s.ring.lock().unwrap().clear();
        }
        let t0 = Instant::now();
        let mut written: u64 = 0;
        let mut buf: Vec<f32> = Vec::new();
        let mut bytes: Vec<u8> = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            let due = (t0.elapsed().as_secs_f64() * RATE as f64) as u64;
            let mut owed = due.saturating_sub(written);
            if owed > RATE as u64 * 2 {
                // We were stalled (system sleep, debugger): skip ahead instead of bursting.
                written = due - RATE as u64 / 50;
                owed = RATE as u64 / 50;
            }
            if owed > 0 {
                buf.clear();
                buf.resize(owed as usize * CH, 0.0);
                for s in &slots {
                    s.mix_into(&mut buf);
                }
                bytes.clear();
                bytes.reserve(buf.len() * 4);
                for v in &buf {
                    bytes.extend_from_slice(&v.clamp(-1.0, 1.0).to_le_bytes());
                }
                let mut done = 0u32;
                if !unsafe { WriteFile(h, Some(&bytes), Some(&mut done), None) }.succeeded() {
                    break; // FFmpeg closed the pipe
                }
                written += owed;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    unsafe {
        let _ = CloseHandle(h);
    }
}

// ------------------------------------------------------------------ feed

/// Running audio capture feeding one recording session.
pub struct AudioFeed {
    system: Arc<Slot>,
    mic: Arc<Slot>,
    stop: Arc<AtomicBool>,
    pipe_names: Vec<String>,
    threads: Vec<JoinHandle<()>>,
}

impl AudioFeed {
    /// Open the configured devices and create pipes for FFmpeg. Sources that fail to
    /// initialize are left out (and reported via `states`), never faked.
    pub fn start(s: &AudioSettings) -> (AudioFeed, Vec<AudioInput>) {
        let stop = Arc::new(AtomicBool::new(false));
        let system = Arc::new(Slot::new(s.system_volume));
        let mic = Arc::new(Slot::new(s.mic_volume));
        mic.muted.store(s.mic_muted, Ordering::Relaxed);
        let mut feed = AudioFeed {
            system: system.clone(),
            mic: mic.clone(),
            stop: stop.clone(),
            pipe_names: vec![],
            threads: vec![],
        };

        let mut active: Vec<(Arc<Slot>, &'static str)> = Vec::new();
        let mut start_source =
            |enabled: bool, loopback: bool, id: Option<String>, slot: Arc<Slot>, title: &'static str| {
                if !enabled {
                    return;
                }
                let (tx, rx) = mpsc::channel();
                let (sl, st) = (slot.clone(), stop.clone());
                feed.threads.push(
                    std::thread::Builder::new()
                        .name(format!("audio-{title}"))
                        .spawn(move || capture_thread(loopback, id, sl, st, tx))
                        .expect("spawn audio thread"),
                );
                if let Ok(Ok(_)) = rx.recv_timeout(Duration::from_secs(3)) {
                    active.push((slot, title));
                } else if matches!(*slot.state.lock().unwrap(), SourceState::Off) {
                    slot.set_state(SourceState::Failed { reason: "The device didn't respond.".into() });
                }
            };
        start_source(s.system, true, s.system_device.clone(), system, "Game & desktop");
        start_source(s.mic, false, s.mic_device.clone(), mic, "Microphone");

        let groups: Vec<(Vec<Arc<Slot>>, String)> = match s.layout {
            TrackLayout::Mixed if !active.is_empty() => {
                let title = active.iter().map(|(_, t)| *t).collect::<Vec<_>>().join(" + ");
                vec![(active.iter().map(|(s, _)| s.clone()).collect(), title)]
            }
            _ => active.iter().map(|(s, t)| (vec![s.clone()], t.to_string())).collect(),
        };

        let mut inputs = Vec::new();
        for (slots, title) in groups {
            let n = PIPE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = format!(r"\\.\pipe\urec-{}-{n}", std::process::id());
            let Some(pipe) = create_pipe(&name) else { continue };
            let st = stop.clone();
            feed.threads.push(
                std::thread::Builder::new()
                    .name("audio-pipe".into())
                    .spawn(move || writer_thread(pipe, slots, st))
                    .expect("spawn pipe thread"),
            );
            feed.pipe_names.push(name.clone());
            inputs.push(AudioInput { source: name, lavfi: false, sample_rate: RATE, channels: CH as u16, title });
        }
        (feed, inputs)
    }

    /// (system, microphone) state.
    pub fn states(&self) -> (SourceState, SourceState) {
        (self.system.state.lock().unwrap().clone(), self.mic.state.lock().unwrap().clone())
    }

    /// Peak levels since the last call (0.0–1.0+).
    pub fn levels(&self) -> (f32, f32) {
        let take = |s: &Slot| f32::from_bits(s.peak.swap(0, Ordering::Relaxed));
        (take(&self.system), take(&self.mic))
    }

    pub fn set_volumes(&self, system: f32, mic: f32) {
        self.system.volume.store(system.to_bits(), Ordering::Relaxed);
        self.mic.volume.store(mic.to_bits(), Ordering::Relaxed);
    }

    pub fn set_mic_muted(&self, muted: bool) {
        self.mic.muted.store(muted, Ordering::Relaxed);
    }

    pub fn mic_muted(&self) -> bool {
        self.mic.muted.load(Ordering::Relaxed)
    }
}

impl Drop for AudioFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for name in &self.pipe_names {
            poke_pipe(name);
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}
