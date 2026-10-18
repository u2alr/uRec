//! The overlay panel's window (layout and drawing are in `panel.rs`).
//!
//! A plain top-level window: layered (per-pixel alpha), always on top, no taskbar button,
//! excluded from screen capture. No DLL injection or graphics hooks, so it works with
//! borderless and windowed games; exclusive-fullscreen games may minimise when it takes focus.
//!
//! Focus: opening the panel makes it the foreground window so it can take clicks and keys
//! (the game would otherwise keep the mouse). Esc, the panel hotkey or Close give the
//! foreground back to the window that had it. Clicking anywhere else closes the panel and
//! leaves focus where the click went. While hidden it has no timer and draws nothing.

use crate::overlay::{present, text_mask_weight};
use crate::panel::{self, Interaction, Layout, PanelAction, PanelModel, Texts};
use crate::sys::{wide, Win32Ok};
use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_DOWN, VK_ESCAPE,
    VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
    GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, IsWindow, KillTimer, LoadCursorW, PostMessageW,
    PostQuitMessage, RegisterClassW, SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    TranslateMessage, HWND_TOPMOST, IDC_ARROW, IDC_HAND, MSG, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOW, WM_ACTIVATE,
    WM_APP, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_SETCURSOR, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const WM_SHOW: u32 = WM_APP + 11;
const WM_HIDE: u32 = WM_APP + 12;
const WM_REFRESH: u32 = WM_APP + 13;
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_ID: usize = 7;
const OPEN_MS: f32 = 140.0;

/// What the controller tells the panel (shared with the panel thread).
#[derive(Clone)]
struct Shared {
    model: PanelModel,
    visible: bool,
    reduced_motion: bool,
    /// "top" | "center"
    position: String,
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);
type OnAction = Box<dyn Fn(PanelAction) + Send + Sync>;
static ON_ACTION: OnceLock<OnAction> = OnceLock::new();

fn default_shared() -> Shared {
    Shared { model: PanelModel::default(), visible: false, reduced_motion: false, position: "top".into() }
}

fn shared() -> Shared {
    SHARED.lock().ok().and_then(|g| g.clone()).unwrap_or_else(default_shared)
}

fn set_visible(v: bool) {
    if let Ok(mut g) = SHARED.lock() {
        if let Some(s) = g.as_mut() {
            s.visible = v;
        }
    }
}

/// State that lives on the panel thread only.
struct Local {
    texts: Texts,
    layout: Option<Layout>,
    pixels: Vec<u8>,
    model: PanelModel,
    ui: Interaction,
    /// Window to give the foreground back to.
    prev: isize,
    monitor: RECT,
    pos: (i32, i32),
    opened: Option<Instant>,
    timer: bool,
    tracking: bool,
    hiding: bool,
    shown: bool,
}

thread_local! {
    static LOCAL: RefCell<Option<Local>> = const { RefCell::new(None) };
}

/// Run `f` on the panel state. A message that arrives while the state is already in use
/// (sent back to us from inside a Windows call) is skipped instead of panicking.
fn with_local<R>(f: impl FnOnce(&mut Local) -> R) -> Option<R> {
    LOCAL.with(|l| l.try_borrow_mut().ok()?.as_mut().map(f))
}

fn scale(monitor: &RECT) -> f32 {
    ((monitor.bottom - monitor.top) as f32 / 1080.0).clamp(1.0, 2.5)
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

impl Local {
    fn rebuild(&mut self) {
        let k = scale(&self.monitor);
        let l = panel::layout(&self.model, k, &mut self.texts);
        // Keep hover/focus only while they still point at a button.
        if self.layout.as_ref().map(|o| o.buttons.len()) != Some(l.buttons.len()) {
            self.ui.hover = None;
            self.ui.pressed = None;
            self.ui.focus = panel::next_focus(&l, None, 1);
        }
        let (mw, mh) = (self.monitor.right - self.monitor.left, self.monitor.bottom - self.monitor.top);
        let x = self.monitor.left + (mw - l.w as i32) / 2;
        let y = if shared().position == "center" {
            self.monitor.top + (mh - l.h as i32) / 2
        } else {
            self.monitor.top + (64.0 * k) as i32 - l.card.y as i32
        };
        self.pos = (x, y);
        self.layout = Some(l);
        self.redraw_pixels();
    }

    fn redraw_pixels(&mut self) {
        if let Some(l) = &self.layout {
            self.pixels = panel::render(l, self.ui, &mut self.texts);
        }
    }

    unsafe fn present(&mut self, hwnd: HWND) {
        let Some(l) = &self.layout else { return };
        let k = l.k;
        let (alpha, dy) = match self.opened {
            Some(t0) => {
                let t = (t0.elapsed().as_secs_f32() * 1000.0 / OPEN_MS).clamp(0.0, 1.0);
                if t >= 1.0 {
                    self.opened = None;
                }
                let e = ease_out(t);
                (e, -(1.0 - e) * 8.0 * k)
            }
            None => (1.0, 0.0),
        };
        present(hwnd, self.pos.0, self.pos.1 + dy as i32, l.w as i32, l.h as i32, &self.pixels, (alpha * 255.0) as u8);
        let animating = self.opened.is_some();
        if animating && !self.timer {
            SetTimer(Some(hwnd), TIMER_ID, 16, None);
            self.timer = true;
        } else if !animating && self.timer {
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            self.timer = false;
        }
    }
}

/// Make `hwnd` the foreground window. Allowed straight away when uRec received the hotkey;
/// otherwise attach to the foreground thread's input for a moment, the usual workaround.
unsafe fn bring_to_front(hwnd: HWND) {
    if SetForegroundWindow(hwnd).succeeded() {
        let _ = SetFocus(Some(hwnd));
        return;
    }
    let fg = GetForegroundWindow();
    let fg_thread = GetWindowThreadProcessId(fg, None);
    let me = GetCurrentThreadId();
    if fg_thread != 0 && fg_thread != me {
        let _ = AttachThreadInput(me, fg_thread, true);
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        let _ = AttachThreadInput(me, fg_thread, false);
    }
}

unsafe fn monitor_for(window: HWND) -> RECT {
    let hmon = if !window.0.is_null() {
        MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST)
    } else {
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST)
    };
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if GetMonitorInfoW(hmon, &mut info).succeeded() {
        info.rcMonitor
    } else {
        RECT { left: 0, top: 0, right: 1920, bottom: 1080 }
    }
}

/// Tests: where each button is on screen ("Action x y" per line), for real mouse clicks.
fn write_hitmap(l: &Local) {
    let (Some(path), Some(lay)) = (std::env::var_os("UREC_PANEL_HITMAP"), l.layout.as_ref()) else { return };
    let lines: Vec<String> = lay
        .buttons
        .iter()
        .map(|b| {
            let x = l.pos.0 + (b.rect.x + b.rect.w / 2.0) as i32;
            let y = l.pos.1 + (b.rect.y + b.rect.h / 2.0) as i32;
            format!("{:?} {x} {y}", b.action)
        })
        .collect();
    let _ = std::fs::write(path, lines.join("\n"));
}

unsafe fn show(hwnd: HWND) {
    let fg = GetForegroundWindow();
    let st = shared();
    with_local(|l| {
        if l.shown {
            return;
        }
        if fg != hwnd {
            l.prev = fg.0 as isize;
        }
        l.monitor = monitor_for(fg);
        l.model = st.model.clone();
        l.layout = None;
        l.ui = Interaction::default();
        l.rebuild();
        write_hitmap(l);
        l.opened = if st.reduced_motion { None } else { Some(Instant::now()) };
        l.shown = true;
        l.present(hwnd);
    });
    set_visible(true);
    let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
    let _ = ShowWindow(hwnd, SW_SHOW);
    bring_to_front(hwnd);
}

/// Hide; with `restore`, give the foreground back to the window that had it.
unsafe fn hide(hwnd: HWND, restore: bool) {
    let prev = with_local(|l| {
        if !l.shown {
            return None;
        }
        l.shown = false;
        l.hiding = true;
        l.opened = None;
        if l.timer {
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            l.timer = false;
        }
        Some(l.prev)
    })
    .flatten();
    set_visible(false);
    let Some(prev) = prev else { return };
    let _ = ShowWindow(hwnd, SW_HIDE);
    let prev = HWND(prev as *mut _);
    if restore && !prev.0.is_null() && IsWindow(Some(prev)).succeeded() {
        let _ = SetForegroundWindow(prev);
    }
    with_local(|l| l.hiding = false);
}

/// Run a button's action. Close and Settings close the panel first.
unsafe fn act(hwnd: HWND, action: PanelAction) {
    match action {
        PanelAction::Close => hide(hwnd, true),
        PanelAction::OpenSettings => {
            hide(hwnd, false);
            if let Some(f) = ON_ACTION.get() {
                f(action);
            }
        }
        _ => {
            if let Some(f) = ON_ACTION.get() {
                f(action);
            }
        }
    }
}

fn mouse_xy(lp: LPARAM) -> (f32, f32) {
    let x = (lp.0 & 0xFFFF) as u16 as i16 as f32;
    let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
    (x + 0.5, y + 0.5)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SHOW => {
            show(hwnd);
            LRESULT(0)
        }
        WM_HIDE => {
            hide(hwnd, wp.0 == 1);
            LRESULT(0)
        }
        WM_REFRESH => {
            let model = shared().model;
            with_local(|l| {
                if l.shown && l.model != model {
                    l.model = model;
                    l.rebuild();
                    l.present(hwnd);
                }
            });
            LRESULT(0)
        }
        WM_TIMER => {
            with_local(|l| l.present(hwnd));
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            // Resolution changed (often a game switching modes): place it again.
            with_local(|l| {
                if l.shown {
                    l.monitor = monitor_for(HWND(l.prev as *mut _));
                    l.rebuild();
                    l.present(hwnd);
                }
            });
            LRESULT(0)
        }
        WM_ACTIVATE => {
            // Clicked somewhere else: close, and leave focus where the click went.
            if (wp.0 & 0xFFFF) == 0 && with_local(|l| l.shown && !l.hiding).unwrap_or(false) {
                hide(hwnd, false);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = mouse_xy(lp);
            with_local(|l| {
                if !l.tracking {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    l.tracking = TrackMouseEvent(&mut t).is_ok();
                }
                let hover = l.layout.as_ref().and_then(|lay| panel::hit(lay, x, y));
                if hover != l.ui.hover {
                    l.ui.hover = hover;
                    l.redraw_pixels();
                    l.present(hwnd);
                }
            });
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_local(|l| {
                l.tracking = false;
                if l.ui.hover.is_some() {
                    l.ui.hover = None;
                    l.redraw_pixels();
                    l.present(hwnd);
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = mouse_xy(lp);
            with_local(|l| {
                l.ui.pressed = l.layout.as_ref().and_then(|lay| panel::hit(lay, x, y));
                l.ui.hover = l.ui.pressed;
                l.redraw_pixels();
                l.present(hwnd);
            });
            let _ = SetCapture(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            let (x, y) = mouse_xy(lp);
            let action = with_local(|l| {
                let lay = l.layout.as_ref()?;
                let under = panel::hit(lay, x, y);
                let pressed = l.ui.pressed.take();
                let a = match (pressed, under) {
                    (Some(p), Some(u)) if p == u && lay.buttons[u].enabled => Some(lay.buttons[u].action),
                    _ => None,
                };
                l.redraw_pixels();
                l.present(hwnd);
                a
            })
            .flatten();
            if let Some(a) = action {
                act(hwnd, a);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wp.0 as u16;
            if vk == VK_ESCAPE.0 {
                hide(hwnd, true);
                return LRESULT(0);
            }
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            let action = with_local(|l| {
                let lay = l.layout.as_ref()?;
                let dir = if vk == VK_TAB.0 {
                    Some(if shift { -1 } else { 1 })
                } else if vk == VK_RIGHT.0 || vk == VK_DOWN.0 {
                    Some(1)
                } else if vk == VK_LEFT.0 || vk == VK_UP.0 {
                    Some(-1)
                } else {
                    None
                };
                let mut action = None;
                if let Some(d) = dir {
                    // The first key press only shows where the focus is.
                    if l.ui.show_focus {
                        l.ui.focus = panel::next_focus(lay, l.ui.focus, d);
                    }
                } else if vk == VK_RETURN.0 || vk == VK_SPACE.0 {
                    action = l.ui.focus.filter(|&f| lay.buttons[f].enabled).map(|f| lay.buttons[f].action);
                } else {
                    return None;
                }
                l.ui.show_focus = true;
                l.redraw_pixels();
                l.present(hwnd);
                action
            })
            .flatten();
            if let Some(a) = action {
                act(hwnd, a);
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let hand = with_local(|l| {
                l.ui.hover.zip(l.layout.as_ref()).is_some_and(|(h, lay)| lay.buttons.get(h).is_some_and(|b| b.enabled))
            })
            .unwrap_or(false);
            if let Ok(c) = LoadCursorW(None, if hand { IDC_HAND } else { IDC_ARROW }) {
                let _ = SetCursor(Some(c));
            }
            LRESULT(1)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            LOCAL.with(|l| {
                if let Ok(mut g) = l.try_borrow_mut() {
                    *g = None;
                }
            });
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub struct Panel {
    hwnd: Arc<Mutex<Option<isize>>>,
    thread: Option<JoinHandle<()>>,
}

impl Panel {
    /// `on_action` runs on the panel thread for every button pressed.
    pub fn new(on_action: impl Fn(PanelAction) + Send + Sync + 'static) -> Panel {
        let _ = ON_ACTION.set(Box::new(on_action));
        if let Ok(mut g) = SHARED.lock() {
            g.get_or_insert_with(default_shared);
        }
        let slot: Arc<Mutex<Option<isize>>> = Arc::new(Mutex::new(None));
        let slot2 = slot.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("overlay-panel".into())
            .spawn(move || unsafe {
                let class = wide("uRecPanel");
                let title = wide("uRec panel");
                let hinst = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: hinst,
                    lpszClassName: PCWSTR(class.as_ptr()),
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    ..Default::default()
                };
                RegisterClassW(&wc);
                let hwnd = CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                    PCWSTR(class.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    WS_POPUP,
                    0,
                    0,
                    10,
                    10,
                    None,
                    None,
                    Some(hinst),
                    None,
                );
                let Ok(hwnd) = hwnd else {
                    let _ = tx.send(());
                    return;
                };
                if std::env::var_os("UREC_OVERLAY_CAPTURABLE").is_none() {
                    crate::sys::exclude_from_capture(hwnd.0 as usize as u64);
                }
                LOCAL.with(|l| {
                    *l.borrow_mut() = Some(Local {
                        texts: Texts::new(|t, px, semibold| text_mask_weight(t, px, semibold)),
                        layout: None,
                        pixels: Vec::new(),
                        model: PanelModel::default(),
                        ui: Interaction::default(),
                        prev: 0,
                        monitor: RECT { left: 0, top: 0, right: 1920, bottom: 1080 },
                        pos: (0, 0),
                        opened: None,
                        timer: false,
                        tracking: false,
                        hiding: false,
                        shown: false,
                    })
                });
                *slot2.lock().unwrap() = Some(hwnd.0 as isize);
                let _ = tx.send(());
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            })
            .ok();
        let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
        Panel { hwnd: slot, thread }
    }

    fn post(&self, msg: u32, wp: usize) -> bool {
        match *self.hwnd.lock().unwrap() {
            Some(h) => unsafe { PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(wp), LPARAM(0)).succeeded() },
            None => false,
        }
    }

    pub fn is_visible(&self) -> bool {
        shared().visible
    }

    /// Update what the panel shows. Cheap; redraws only when something changed and it is open.
    pub fn set_model(&self, model: PanelModel) {
        let changed = match SHARED.lock() {
            Ok(mut g) => match g.as_mut() {
                Some(s) if s.model != model => {
                    s.model = model;
                    s.visible
                }
                _ => false,
            },
            Err(_) => false,
        };
        if changed {
            self.post(WM_REFRESH, 0);
        }
    }

    /// Open (or close, if open) the panel. `position` is "top" or "center".
    pub fn toggle(&self, model: PanelModel, position: &str, reduced_motion: bool) {
        let open = if let Ok(mut g) = SHARED.lock() {
            let s = g.get_or_insert_with(default_shared);
            s.model = model;
            s.position = position.to_string();
            s.reduced_motion = reduced_motion;
            // Flip now, so a quick second press closes instead of asking to open twice.
            let was = s.visible;
            s.visible = !was;
            was
        } else {
            false
        };
        if open {
            self.post(WM_HIDE, 1);
        } else {
            self.post(WM_SHOW, 0);
        }
    }

    /// Close it and give focus back.
    pub fn hide(&self) {
        if self.is_visible() {
            self.post(WM_HIDE, 1);
        }
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.post(WM_CLOSE, 0);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        set_visible(false);
    }
}
