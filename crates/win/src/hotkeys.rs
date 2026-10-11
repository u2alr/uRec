//! Global hotkeys via RegisterHotKey. Works while games have focus and doesn't swallow
//! other keys (Windows only delivers the exact registered combination to us).

use crate::sys::Win32Ok;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use urec_engine::hotkey::Hotkey;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, PostThreadMessageW, MSG, WM_HOTKEY, WM_QUIT};

pub struct Hotkeys {
    on_press: Arc<dyn Fn(usize) + Send + Sync>,
    thread: Option<(u32, JoinHandle<()>)>,
}

impl Hotkeys {
    /// `on_press(index)` is called from the hotkey thread with the index into the slice given to `set`.
    pub fn new(on_press: impl Fn(usize) + Send + Sync + 'static) -> Self {
        Hotkeys { on_press: Arc::new(on_press), thread: None }
    }

    /// Register a new set of hotkeys, replacing the old ones.
    /// Returns the indices that could not be registered (taken by another app or invalid).
    pub fn set(&mut self, keys: &[Option<Hotkey>]) -> Vec<usize> {
        self.stop();
        let keys = keys.to_vec();
        let cb = self.on_press.clone();
        let (tx, rx) = mpsc::channel::<(u32, Vec<usize>)>();
        let handle = std::thread::Builder::new()
            .name("hotkeys".into())
            .spawn(move || unsafe {
                let tid = GetCurrentThreadId();
                let mut failed = Vec::new();
                let mut registered = Vec::new();
                for (i, k) in keys.iter().enumerate() {
                    let Some(k) = k else { continue };
                    let id = i as i32 + 1;
                    if RegisterHotKey(None, id, HOT_KEY_MODIFIERS(k.mods) | MOD_NOREPEAT, k.vk).succeeded() {
                        registered.push(id);
                    } else {
                        failed.push(i);
                    }
                }
                let _ = tx.send((tid, failed));
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                    if msg.message == WM_HOTKEY {
                        let id = msg.wParam.0;
                        if id >= 1 {
                            cb(id - 1);
                        }
                    }
                }
                for id in registered {
                    let _ = UnregisterHotKey(None, id);
                }
            })
            .expect("spawn hotkey thread");
        let (tid, failed) = rx.recv().unwrap_or((0, Vec::new()));
        self.thread = Some((tid, handle));
        failed
    }

    pub fn stop(&mut self) {
        if let Some((tid, handle)) = self.thread.take() {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            let _ = handle.join();
        }
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        self.stop();
    }
}
