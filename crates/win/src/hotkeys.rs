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

/// Push-to-talk: watches one key (or side mouse button) and reports press and release.
/// Polls the key state every 20 ms instead of registering a hotkey, so the key still reaches
/// the game (a registered hotkey would be swallowed). Runs only while it exists.
pub struct PushToTalk {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PushToTalk {
    pub fn start(key: Hotkey, on_change: impl Fn(bool) + Send + 'static) -> PushToTalk {
        use std::sync::atomic::{AtomicBool, Ordering};
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("push-to-talk".into())
            .spawn(move || {
                let down = |vk: i32| unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 };
                let mut held = false;
                while !s2.load(Ordering::Relaxed) {
                    let mods_ok = (key.mods & urec_engine::hotkey::MOD_CONTROL == 0 || down(VK_CONTROL.0 as i32))
                        && (key.mods & urec_engine::hotkey::MOD_ALT == 0 || down(VK_MENU.0 as i32))
                        && (key.mods & urec_engine::hotkey::MOD_SHIFT == 0 || down(VK_SHIFT.0 as i32));
                    // Releasing either the key or a modifier ends talking.
                    let now = down(key.vk as i32) && mods_ok;
                    if now != held {
                        held = now;
                        on_change(held);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                if held {
                    on_change(false);
                }
            })
            .ok();
        PushToTalk { stop, thread }
    }
}

impl Drop for PushToTalk {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        self.stop();
    }
}
