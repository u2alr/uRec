//! Recording overlay: a small round dot in a screen corner. Red while recording, amber while
//! Instant Replay is buffering. Drawn into a per-pixel-alpha bitmap (anti-aliased, perfectly
//! round), click-through, never takes focus, always on top, and excluded from screen capture
//! on Windows 10 2004+. Owned by the controller so it works while the main window is hidden.

use crate::sys::{wide, Win32Ok};
use crate::OverlayState;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW, PostQuitMessage,
    RegisterClassW, SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow, HWND_TOPMOST, MSG, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_APP, WM_CLOSE, WM_DESTROY, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

static STATE: OnceLock<Arc<Mutex<OverlayState>>> = OnceLock::new();
const WM_REFRESH: u32 = WM_APP + 1;

fn scale(st: &OverlayState) -> f32 {
    (st.monitor.3 as f32 / 1080.0).clamp(1.0, 2.5)
}

/// Premultiplied BGRA pixels of the dot: a soft dark halo for contrast on bright scenes,
/// then the coloured disc. 4×4 supersampling gives smooth, round edges.
pub fn render_dot(size: usize, color: (u8, u8, u8)) -> Vec<u8> {
    let mut px = vec![0u8; size * size * 4];
    let c = size as f32 / 2.0;
    let r_dot = size as f32 * 0.30;
    let r_halo = size as f32 * 0.42;
    const SS: usize = 4;
    for y in 0..size {
        for x in 0..size {
            let (mut dot, mut halo) = (0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let fx = x as f32 + (sx as f32 + 0.5) / SS as f32 - c;
                    let fy = y as f32 + (sy as f32 + 0.5) / SS as f32 - c;
                    let d = (fx * fx + fy * fy).sqrt();
                    if d <= r_dot {
                        dot += 1.0;
                    }
                    if d <= r_halo {
                        halo += 1.0;
                    }
                }
            }
            let n = (SS * SS) as f32;
            let (dot, halo) = (dot / n, halo / n);
            // Halo: black at 35% opacity; dot drawn over it at full opacity.
            let halo_a = halo * 0.35 * (1.0 - dot);
            let a = dot + halo_a;
            let i = (y * size + x) * 4;
            // Premultiplied: colour channels already multiplied by alpha.
            px[i] = (color.2 as f32 * dot).round() as u8;
            px[i + 1] = (color.1 as f32 * dot).round() as u8;
            px[i + 2] = (color.0 as f32 * dot).round() as u8;
            px[i + 3] = (a * 255.0).round().min(255.0) as u8;
        }
    }
    px
}

unsafe fn redraw(hwnd: HWND, st: &OverlayState) {
    let k = scale(st);
    let size = ((22.0 * k).round() as usize).max(12);
    let margin = (16.0 * k) as i32;
    let (mx, my, mw, mh) = (st.monitor.0, st.monitor.1, st.monitor.2 as i32, st.monitor.3 as i32);
    let s = size as i32;
    let (x, y) = match st.corner.as_str() {
        "top_left" => (mx + margin, my + margin),
        "bottom_left" => (mx + margin, my + mh - s - margin * 3),
        "bottom_right" => (mx + mw - s - margin, my + mh - s - margin * 3),
        _ => (mx + mw - s - margin, my + margin),
    };
    let pixels = render_dot(size, st.dot);

    let screen = GetDC(None);
    let mem = CreateCompatibleDC(Some(screen));
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: s,
            biHeight: -s, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    if let Ok(bmp) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
        if !bits.is_null() {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let pos = POINT { x, y };
            let sz = SIZE { cx: s, cy: s };
            let src = POINT { x: 0, y: 0 };
            let _ = UpdateLayeredWindow(
                hwnd,
                Some(screen),
                Some(&pos),
                Some(&sz),
                Some(mem),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            SelectObject(mem, old);
        }
        let _ = DeleteObject(HGDIOBJ(bmp.0));
    }
    let _ = DeleteDC(mem);
    ReleaseDC(None, screen);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_REFRESH => {
            let st = STATE.get().and_then(|s| s.lock().ok().map(|g| g.clone())).unwrap_or_default();
            if st.visible {
                redraw(hwnd, &st);
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE);
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            } else {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub struct Overlay {
    hwnd: Arc<Mutex<Option<isize>>>,
    last: Mutex<OverlayState>,
    thread: Option<JoinHandle<()>>,
}

impl Overlay {
    pub fn new() -> Overlay {
        STATE.get_or_init(|| Arc::new(Mutex::new(OverlayState::default())));
        let hwnd_slot: Arc<Mutex<Option<isize>>> = Arc::new(Mutex::new(None));
        let slot = hwnd_slot.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("overlay".into())
            .spawn(move || unsafe {
                let class = wide("uRecOverlay");
                let title = wide("uRec overlay");
                let hinst = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: hinst,
                    lpszClassName: PCWSTR(class.as_ptr()),
                    ..Default::default()
                };
                RegisterClassW(&wc);
                let hwnd = CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    PCWSTR(class.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    WS_POPUP,
                    0,
                    0,
                    22,
                    22,
                    None,
                    None,
                    Some(hinst),
                    None,
                );
                let Ok(hwnd) = hwnd else {
                    let _ = tx.send(());
                    return;
                };
                crate::sys::exclude_from_capture(hwnd.0 as usize as u64);
                *slot.lock().unwrap() = Some(hwnd.0 as isize);
                let _ = tx.send(());
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            })
            .ok();
        let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
        Overlay { hwnd: hwnd_slot, last: Mutex::new(OverlayState::default()), thread }
    }

    /// An overlay that never shows (for troubleshooting).
    pub fn disabled() -> Overlay {
        Overlay { hwnd: Arc::new(Mutex::new(None)), last: Mutex::new(OverlayState::default()), thread: None }
    }

    /// Update what the overlay shows. Cheap to call often; only redraws on change.
    pub fn set(&self, st: OverlayState) {
        {
            let mut last = self.last.lock().unwrap();
            if *last == st {
                return;
            }
            *last = st.clone();
        }
        if let Some(s) = STATE.get() {
            *s.lock().unwrap() = st;
        }
        if let Some(h) = *self.hwnd.lock().unwrap() {
            unsafe {
                let _ = PostMessageW(Some(HWND(h as *mut _)), WM_REFRESH, WPARAM(0), LPARAM(0)).succeeded();
            }
        }
    }
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        if let Some(h) = *self.hwnd.lock().unwrap() {
            unsafe {
                let _ = PostMessageW(Some(HWND(h as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::render_dot;

    #[test]
    fn dot_is_round_and_symmetric() {
        let n = 33;
        let px = render_dot(n, (229, 72, 77));
        let a = |x: usize, y: usize| px[(y * n + x) * 4 + 3];
        // centre fully opaque, corners fully transparent
        assert_eq!(a(16, 16), 255);
        assert_eq!(a(0, 0), 0);
        // 4-way and diagonal symmetry
        for y in 0..n {
            for x in 0..n {
                assert_eq!(a(x, y), a(n - 1 - x, y));
                assert_eq!(a(x, y), a(x, n - 1 - y));
                assert_eq!(a(x, y), a(y, x));
            }
        }
    }
}
