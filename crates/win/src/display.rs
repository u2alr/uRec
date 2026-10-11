//! Monitor and window enumeration.

use crate::sys::Win32Ok;
use crate::{MonitorInfo, Rect, WindowInfo};
use std::ffi::c_void;
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, DEVMODEW, ENUM_CURRENT_SETTINGS, HDC, HMONITOR,
    MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, GWL_EXSTYLE, MONITORINFOF_PRIMARY, WS_EX_TOOLWINDOW,
};

fn rect(r: RECT) -> Rect {
    Rect { x: r.left, y: r.top, width: (r.right - r.left).max(0) as u32, height: (r.bottom - r.top).max(0) as u32 }
}

fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

unsafe extern "system" fn monitor_cb(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let list = &mut *(data.0 as *mut Vec<HMONITOR>);
    list.push(h);
    true.into()
}

/// (adapter, output) for each DXGI output, keyed by HMONITOR.
fn dxgi_outputs() -> Vec<(u64, u32, u32)> {
    let mut out = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                if let Ok(desc) = output.GetDesc() {
                    out.push((desc.Monitor.0 as usize as u64, a, o));
                }
                o += 1;
            }
            a += 1;
        }
    }
    out
}

/// Connected monitors, primary first.
pub fn monitors() -> Vec<MonitorInfo> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(monitor_cb), LPARAM(&mut handles as *mut _ as isize));
    }
    let dxgi = dxgi_outputs();
    let mut list = Vec::new();
    for (i, h) in handles.iter().enumerate() {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !unsafe { GetMonitorInfoW(*h, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) }.succeeded() {
            continue;
        }
        let device = from_wide(&info.szDevice);
        let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        let refresh = unsafe {
            EnumDisplaySettingsW(windows::core::PCWSTR(info.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode)
        }
        .succeeded()
        .then_some(mode.dmDisplayFrequency)
        .filter(|&f| f > 1);
        let hm = h.0 as usize as u64;
        let (adapter, output) =
            dxgi.iter().find(|(m, _, _)| *m == hm).map(|(_, a, o)| (Some(*a), Some(*o))).unwrap_or((None, None));
        let bounds = rect(info.monitorInfo.rcMonitor);
        let primary = info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;
        let mut name = format!("Display {}", i + 1);
        if primary {
            name += " (main)";
        }
        list.push(MonitorInfo {
            name: format!(
                "{name} · {}×{}{}",
                bounds.width,
                bounds.height,
                refresh.map(|r| format!(" · {r} Hz")).unwrap_or_default()
            ),
            device,
            hmonitor: hm,
            bounds,
            primary,
            refresh_hz: refresh,
            dxgi_adapter: adapter,
            dxgi_output: output,
        });
    }
    list.sort_by_key(|m| !m.primary);
    list
}

fn window_info(hwnd: HWND) -> Option<WindowInfo> {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]).trim().to_string();
        let mut r = RECT::default();
        if !DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .succeeded()
        {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut r);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let exe = crate::sys::process_image(pid)
            .and_then(|p| std::path::Path::new(&p).file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_default();
        Some(WindowInfo { hwnd: hwnd.0 as usize as u64, title, exe, bounds: rect(r) })
    }
}

unsafe extern "system" fn window_cb(hwnd: HWND, data: LPARAM) -> BOOL {
    let list = &mut *(data.0 as *mut Vec<WindowInfo>);
    if !IsWindowVisible(hwnd).succeeded() || IsIconic(hwnd).succeeded() {
        return true.into();
    }
    if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
        return true.into();
    }
    let mut cloaked = 0u32;
    let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4);
    if cloaked != 0 {
        return true.into();
    }
    if let Some(w) = window_info(hwnd) {
        if w.bounds.width >= 64 && w.bounds.height >= 64 && w.title != "Program Manager" {
            list.push(w);
        }
    }
    true.into()
}

/// Visible, capturable application windows (top of Z-order first).
pub fn windows() -> Vec<WindowInfo> {
    let mut list: Vec<WindowInfo> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(window_cb), LPARAM(&mut list as *mut _ as isize));
    }
    let me = std::process::id();
    list.retain(|w| {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(HWND(w.hwnd as usize as *mut c_void), Some(&mut pid)) };
        pid != me
    });
    list
}

/// The window the user is looking at (used to name clips after the game).
pub fn foreground_window() -> Option<WindowInfo> {
    let h = unsafe { GetForegroundWindow() };
    if h.0.is_null() {
        return None;
    }
    window_info(h)
}
