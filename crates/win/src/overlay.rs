//! Recording overlay: a small round dot in a screen corner. Red while recording, amber while
//! Instant Replay is buffering. When an Instant Replay save finishes, the dot turns into a
//! short "Instant Replay Saved" / "Instant Replay Failed" pill and back (see `pill.rs`).
//!
//! Drawn into a per-pixel-alpha bitmap, click-through, never takes focus, always on top, and
//! excluded from screen capture on Windows 10 2004+. Owned by the controller so it works while
//! the main window is hidden. A timer runs only while something moves; at rest the overlay
//! costs nothing.

use crate::pill::{self, Indicator, Kind, Look, Metrics, TextMask};
use crate::sys::{wide, Win32Ok};
use crate::{OverlayEvent, OverlayState};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GdiFlush, GetDC, GetTextExtentPoint32W,
    ReleaseDC, SelectObject, SetBkMode, SetTextColor, TextOutW, AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DIB_RGB_COLORS,
    FW_NORMAL, FW_SEMIBOLD, HDC, HGDIOBJ, OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, PostMessageW,
    PostQuitMessage, RegisterClassW, SetTimer, SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow,
    HWND_TOPMOST, MSG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_APP, WM_CLOSE,
    WM_DESTROY, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

static STATE: OnceLock<Arc<Mutex<OverlayState>>> = OnceLock::new();
/// Save events waiting for the overlay thread, with their reduced-motion flag.
static EVENTS: Mutex<Vec<(OverlayEvent, bool, Instant)>> = Mutex::new(Vec::new());
const WM_REFRESH: u32 = WM_APP + 1;
const WM_EVENT: u32 = WM_APP + 2;
const TIMER_ID: usize = 1;
/// ~60 fps while animating.
const FRAME_MS: u32 = 16;

/// State that lives on the overlay thread only.
struct Anim {
    indicator: Indicator,
    timer: bool,
    masks: HashMap<(Kind, u32), TextMask>,
}

static ANIM: Mutex<Option<Anim>> = Mutex::new(None);

fn scale(st: &OverlayState) -> f32 {
    (st.monitor.3 as f32 / 1080.0).clamp(1.0, 2.5) * st.size.clamp(0.6, 1.6)
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

/// Dot window size and the dot's centre on screen.
fn dot_geometry(st: &OverlayState) -> (i32, f32, f32) {
    let k = scale(st);
    let size = ((22.0 * k).round() as i32).max(12);
    let margin = (16.0 * k) as i32;
    let (mx, my, mw, mh) = (st.monitor.0, st.monitor.1, st.monitor.2 as i32, st.monitor.3 as i32);
    let (x, y) = match st.corner.as_str() {
        "top_left" => (mx + margin, my + margin),
        "bottom_left" => (mx + margin, my + mh - size - margin * 3),
        "bottom_right" => (mx + mw - size - margin, my + mh - size - margin * 3),
        _ => (mx + mw - size - margin, my + margin),
    };
    (size, x as f32 + size as f32 / 2.0, y as f32 + size as f32 / 2.0)
}

/// Put premultiplied BGRA pixels on screen at (x, y).
pub(crate) unsafe fn present(hwnd: HWND, x: i32, y: i32, w: i32, h: i32, pixels: &[u8], alpha: u8) {
    let screen = GetDC(None);
    let mem = CreateCompatibleDC(Some(screen));
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    if let Ok(bmp) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
        if !bits.is_null() && pixels.len() == (w * h * 4) as usize {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let pos = POINT { x, y };
            let sz = SIZE { cx: w, cy: h };
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

/// Render a line of text into a coverage mask with GDI (greyscale anti-aliasing, white on
/// black, so any channel is the coverage).
unsafe fn text_mask(text: &str, px_height: i32) -> TextMask {
    text_mask_weight(text, px_height, true)
}

/// Like `text_mask`, in Segoe UI Semibold or regular Segoe UI.
pub(crate) unsafe fn text_mask_weight(text: &str, px_height: i32, semibold: bool) -> TextMask {
    let w16: Vec<u16> = text.encode_utf16().collect();
    let screen = GetDC(None);
    let dc: HDC = CreateCompatibleDC(Some(screen));
    let face = wide(if semibold { "Segoe UI Semibold" } else { "Segoe UI" });
    let font = CreateFontW(
        -px_height,
        0,
        0,
        0,
        if semibold { FW_SEMIBOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        ANTIALIASED_QUALITY,
        0,
        PCWSTR(face.as_ptr()),
    );
    let old_font = SelectObject(dc, HGDIOBJ(font.0));
    let mut sz = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &w16, &mut sz);
    let (w, h) = ((sz.cx + 2).max(1), sz.cy.max(1));
    let mut mask = TextMask { w: w as usize, h: h as usize, cov: vec![0; (w * h) as usize] };
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    if let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
        if !bits.is_null() {
            let old = SelectObject(dc, HGDIOBJ(bmp.0));
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00FF_FFFF));
            let _ = TextOutW(dc, 1, 0, &w16);
            let _ = GdiFlush();
            let src = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
            for (i, c) in mask.cov.iter_mut().enumerate() {
                *c = src[i * 4].max(src[i * 4 + 1]).max(src[i * 4 + 2]);
            }
            SelectObject(dc, old);
        }
        let _ = DeleteObject(HGDIOBJ(bmp.0));
    }
    SelectObject(dc, old_font);
    let _ = DeleteObject(HGDIOBJ(font.0));
    let _ = DeleteDC(dc);
    ReleaseDC(None, screen);
    mask
}

fn current_state() -> OverlayState {
    STATE.get().and_then(|s| s.lock().ok().map(|g| g.clone())).unwrap_or_default()
}

/// Draw whatever the indicator says right now; start or stop the frame timer as needed.
unsafe fn draw(hwnd: HWND) {
    let st = current_state();
    let mut guard = ANIM.lock().unwrap();
    let anim =
        guard.get_or_insert_with(|| Anim { indicator: Indicator::default(), timer: false, masks: HashMap::new() });
    let now = Instant::now();
    let look = anim.indicator.look(now);
    let k = scale(&st);
    let (dot_size, cx, cy) = dot_geometry(&st);
    let shown = match look {
        Look::Dot { alpha } if st.visible => {
            let px = render_dot(dot_size as usize, st.dot);
            let (x, y) = ((cx - dot_size as f32 / 2.0) as i32, (cy - dot_size as f32 / 2.0) as i32);
            let a = alpha.clamp(0.0, 1.0) * st.opacity.clamp(0.3, 1.0);
            present(hwnd, x, y, dot_size, dot_size, &px, (a * 255.0) as u8);
            true
        }
        Look::Dot { .. } => false,
        Look::Pill(frame) => {
            let m = Metrics::for_scale(k);
            let key = (k * 100.0) as u32;
            for kind in [Kind::Success, Kind::Failure] {
                anim.masks.entry((kind, key)).or_insert_with(|| text_mask(kind.text(), (12.5 * k).round() as i32));
            }
            let masks = &anim.masks;
            let mask_for = |kind: Kind| masks.get(&(kind, key)).cloned().unwrap_or_default();
            let color = if st.visible { st.dot } else { (217, 164, 65) };
            let grow_left = !st.corner.ends_with("left");
            let img = pill::render(&frame, &m, color, grow_left, &mask_for);
            let x = (cx - img.anchor_x).round() as i32;
            let y = (cy - img.anchor_y).round() as i32;
            present(hwnd, x, y, img.w as i32, img.h as i32, &img.px, 255);
            true
        }
    };
    if shown {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    } else {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
    let animating = anim.indicator.animating();
    if animating && !anim.timer {
        SetTimer(Some(hwnd), TIMER_ID, FRAME_MS, None);
        anim.timer = true;
    } else if !animating && anim.timer {
        let _ = KillTimer(Some(hwnd), TIMER_ID);
        anim.timer = false;
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_REFRESH | WM_TIMER => {
            draw(hwnd);
            LRESULT(0)
        }
        WM_EVENT => {
            let events: Vec<_> = std::mem::take(&mut *EVENTS.lock().unwrap());
            {
                let mut guard = ANIM.lock().unwrap();
                let anim = guard.get_or_insert_with(|| Anim {
                    indicator: Indicator::default(),
                    timer: false,
                    masks: HashMap::new(),
                });
                let hold = current_state().hold_scale;
                for (ev, reduced, at) in events {
                    anim.indicator.set_reduced_motion(reduced);
                    anim.indicator.set_hold_scale(hold);
                    match ev {
                        OverlayEvent::Saving => anim.indicator.saving(at),
                        OverlayEvent::Saved => anim.indicator.finished(Kind::Success, at),
                        OverlayEvent::Failed => anim.indicator.finished(Kind::Failure, at),
                    }
                }
            }
            draw(hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Ok(mut g) = ANIM.lock() {
                if g.as_ref().is_some_and(|a| a.timer) {
                    let _ = KillTimer(Some(hwnd), TIMER_ID);
                }
                *g = None;
            }
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
                // Tests can opt out so screenshots show the overlay; users never do.
                if std::env::var_os("UREC_OVERLAY_CAPTURABLE").is_none() {
                    crate::sys::exclude_from_capture(hwnd.0 as usize as u64);
                }
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

    fn post(&self, msg: u32) -> bool {
        match *self.hwnd.lock().unwrap() {
            Some(h) => unsafe { PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(0), LPARAM(0)).succeeded() },
            None => false,
        }
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
        self.post(WM_REFRESH);
    }

    /// Show save progress or a save result. Returns false if the overlay isn't running (the
    /// caller should use another way to tell the user).
    pub fn notify(&self, ev: OverlayEvent, reduced_motion: bool) -> bool {
        if self.hwnd.lock().unwrap().is_none() {
            return false;
        }
        EVENTS.lock().unwrap().push((ev, reduced_motion, Instant::now()));
        self.post(WM_EVENT)
    }
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.post(WM_CLOSE);
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
