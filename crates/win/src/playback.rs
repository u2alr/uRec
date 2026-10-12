//! Sound output for the editor's preview: WASAPI shared mode on the default output device,
//! 48 kHz stereo float (Windows converts to the device format). The amount actually played
//! is the preview's clock, so the picture follows the sound.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

pub const OUT_RATE: u32 = 48_000;
pub const OUT_CHANNELS: u32 = 2;
/// Queue limit (about half a second); the producer waits when it's full.
const QUEUE_MAX: usize = OUT_RATE as usize * OUT_CHANNELS as usize / 2;

struct Shared {
    queue: Mutex<VecDeque<f32>>,
    space: Condvar,
    played_frames: AtomicU64,
    stop: AtomicBool,
    /// No more samples will come; finish what's queued.
    finished: AtomicBool,
    /// Everything queued has been played after `finished`.
    drained: AtomicBool,
}

pub struct AudioOut {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl AudioOut {
    /// Open the default output device. Err if there's none (the preview then runs silent on
    /// a wall clock).
    pub fn open() -> Result<AudioOut, String> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::with_capacity(QUEUE_MAX)),
            space: Condvar::new(),
            played_frames: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            drained: AtomicBool::new(false),
        });
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let s2 = shared.clone();
        let thread = std::thread::Builder::new()
            .name("preview-audio".into())
            .spawn(move || unsafe { render(s2, tx) })
            .map_err(|e| e.to_string())?;
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(AudioOut { shared, thread: Some(thread) }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("the sound device didn't respond".into()),
        }
    }

    /// Queue interleaved stereo samples; waits while the queue is full. False once stopped.
    pub fn push(&self, samples: &[f32]) -> bool {
        let mut q = self.shared.queue.lock().unwrap();
        for chunk in samples.chunks(4096) {
            while q.len() + chunk.len() > QUEUE_MAX {
                if self.shared.stop.load(Ordering::Relaxed) {
                    return false;
                }
                q = self.shared.space.wait_timeout(q, Duration::from_millis(50)).unwrap().0;
            }
            q.extend(chunk.iter().copied());
        }
        !self.shared.stop.load(Ordering::Relaxed)
    }

    /// No more samples are coming.
    pub fn finish(&self) {
        self.shared.finished.store(true, Ordering::Relaxed);
    }

    /// Seconds of sound actually played so far (the preview clock).
    pub fn position(&self) -> f64 {
        self.shared.played_frames.load(Ordering::Relaxed) as f64 / OUT_RATE as f64
    }

    pub fn done(&self) -> bool {
        self.shared.drained.load(Ordering::Relaxed)
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.space.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

unsafe fn render(s: Arc<Shared>, ready: std::sync::mpsc::Sender<Result<(), String>>) {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let open = || -> windows::core::Result<(IAudioClient, IAudioRenderClient, u32)> {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let fmt = WAVEFORMATEX {
            wFormatTag: 3, // IEEE float
            nChannels: OUT_CHANNELS as u16,
            nSamplesPerSec: OUT_RATE,
            nAvgBytesPerSec: OUT_RATE * OUT_CHANNELS * 4,
            nBlockAlign: (OUT_CHANNELS * 4) as u16,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            1_000_000, // 100 ms
            0,
            &fmt,
            None,
        )?;
        let size = client.GetBufferSize()?;
        let rc: IAudioRenderClient = client.GetService()?;
        client.Start()?;
        Ok((client, rc, size))
    };
    let (client, rc, size) = match open() {
        Ok(x) => {
            let _ = ready.send(Ok(()));
            x
        }
        Err(e) => {
            let _ = ready.send(Err(e.message()));
            return;
        }
    };
    let ch = OUT_CHANNELS as usize;
    let mut written: u64 = 0;
    while !s.stop.load(Ordering::Relaxed) {
        let padding = client.GetCurrentPadding().unwrap_or(0);
        s.played_frames.store(written.saturating_sub(padding as u64), Ordering::Relaxed);
        let free = size.saturating_sub(padding) as usize;
        let n = {
            let mut q = s.queue.lock().unwrap();
            let n = (q.len() / ch).min(free);
            if n > 0 {
                if let Ok(buf) = rc.GetBuffer(n as u32) {
                    let out = std::slice::from_raw_parts_mut(buf as *mut f32, n * ch);
                    for (o, v) in out.iter_mut().zip(q.drain(..n * ch)) {
                        *o = v;
                    }
                    let _ = rc.ReleaseBuffer(n as u32, 0);
                }
            }
            n
        };
        if n > 0 {
            written += n as u64;
            s.space.notify_all();
        } else if s.finished.load(Ordering::Relaxed) && padding == 0 && s.queue.lock().unwrap().is_empty() {
            s.drained.store(true, Ordering::Relaxed);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = client.Stop();
}
