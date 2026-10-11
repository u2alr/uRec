//! Recording overlay: a tiny native window (no GPU, GDI only) in a screen corner.
//! Click-through, never takes focus, always on top, and excluded from screen capture on
//! Windows 10 2004+. Owned by the controller so it works while the main window is hidden.

use crate::sys::{wide, Win32Ok};
use crate::OverlayState;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, Ellipse, EndPaint, FillRect, InvalidateRect,
    SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DT_LEFT, DT_RIGHT,
    DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, HGDIOBJ, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW, PostQuitMessage, RegisterClassW,
    SetLayeredWindowAttributes, SetWindowPos, ShowWindow, TranslateMessage, HWND_TOPMOST, LWA_ALPHA, MSG,
    SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE, WM_APP, WM_CLOSE, WM_DESTROY, WM_PAINT, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

static STATE: OnceLock<Arc<Mutex<OverlayState>>> = OnceLock::new();
const WM_REFRESH: u32 = WM_APP + 1;

pub struct Overlay {
    hwnd: Arc<Mutex<Option<isize>>>,
    last: Mutex<OverlayState>,
    thread: Option<JoinHandle<()>>,
}

fn scale(st: &OverlayState) -> f32 {
    (st.monitor.3 as f32 / 1080.0).clamp(1.0, 2.0)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let st = STATE.get().and_then(|s| s.lock().ok().map(|g| g.clone())).unwrap_or_default();
            let k = scale(&st);
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rc);
            let bg = CreateSolidBrush(COLORREF(0x0016_1514));
            FillRect(hdc, &rc, bg);
            let _ = DeleteObject(HGDIOBJ(bg.0));
            let (r, g, b) = st.dot;
            let dot = CreateSolidBrush(COLORREF((b as u32) << 16 | (g as u32) << 8 | r as u32));
            let old = SelectObject(hdc, HGDIOBJ(dot.0));
            let pen_null = windows::Win32::Graphics::Gdi::GetStockObject(windows::Win32::Graphics::Gdi::NULL_PEN);
            let old_pen = SelectObject(hdc, pen_null);
            let cy = (rc.bottom - rc.top) / 2;
            let d = (10.0 * k) as i32;
            let x0 = (12.0 * k) as i32;
            let _ = Ellipse(hdc, x0, cy - d / 2, x0 + d, cy + d / 2);
            SelectObject(hdc, old_pen);
            SelectObject(hdc, old);
            let _ = DeleteObject(HGDIOBJ(dot.0));
            SetBkMode(hdc, TRANSPARENT);
            let face = wide("Segoe UI");
            let mk = |size: f32, weight: i32| {
                CreateFontW(
                    -(size * k) as i32,
                    0,
                    0,
                    0,
                    weight,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY,
                    FF_DONTCARE.0 as u32,
                    PCWSTR(face.as_ptr()),
                )
            };
            let f_main = mk(15.0, FW_SEMIBOLD.0 as i32);
            let f_small = mk(12.0, FW_NORMAL.0 as i32);
            let prev = SelectObject(hdc, HGDIOBJ(f_main.0));
            SetTextColor(hdc, COLORREF(0x00E6_EAEC));
            let mut text: Vec<u16> = st.text.encode_utf16().collect();
            let mut tr = RECT { left: x0 + d + (8.0 * k) as i32, ..rc };
            DrawTextW(hdc, &mut text, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            if !st.extra.is_empty() {
                SelectObject(hdc, HGDIOBJ(f_small.0));
                SetTextColor(hdc, COLORREF(0x0041_A4D9));
                let mut extra: Vec<u16> = st.extra.encode_utf16().collect();
                let mut er = RECT { right: rc.right - (10.0 * k) as i32, ..rc };
                DrawTextW(hdc, &mut extra, &mut er, DT_RIGHT | DT_VCENTER | DT_SINGLELINE);
            }
            SelectObject(hdc, prev);
            let _ = DeleteObject(HGDIOBJ(f_main.0));
            let _ = DeleteObject(HGDIOBJ(f_small.0));
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_REFRESH => {
            let st = STATE.get().and_then(|s| s.lock().ok().map(|g| g.clone())).unwrap_or_default();
            if st.visible {
                let k = scale(&st);
                let (w, h) = ((if st.extra.is_empty() { 150.0 } else { 220.0 } * k) as i32, (34.0 * k) as i32);
                let m = (18.0 * k) as i32;
                let (mx, my, mw, mh) = (st.monitor.0, st.monitor.1, st.monitor.2 as i32, st.monitor.3 as i32);
                let (x, y) = match st.corner.as_str() {
                    "top_left" => (mx + m, my + m),
                    "bottom_left" => (mx + m, my + mh - h - m * 3),
                    "bottom_right" => (mx + mw - w - m, my + mh - h - m * 3),
                    _ => (mx + mw - w - m, my + m),
                };
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let _ = InvalidateRect(Some(hwnd), None, true);
            } else {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

impl Overlay {
    pub fn new() -> Overlay {
        let state = STATE.get_or_init(|| Arc::new(Mutex::new(OverlayState::default()))).clone();
        let _ = state;
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
                    150,
                    34,
                    None,
                    None,
                    Some(hinst),
                    None,
                );
                let Ok(hwnd) = hwnd else {
                    let _ = tx.send(());
                    return;
                };
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 225, LWA_ALPHA);
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

    /// Update what the overlay shows. Cheap to call often; only repaints on change.
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
