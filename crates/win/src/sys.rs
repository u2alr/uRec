//! Small Windows system helpers.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::OnceLock;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE};

/// Win32 functions return BOOL, HRESULT or Result depending on the binding version;
/// this lets call sites ask "did it work?" uniformly.
pub(crate) trait Win32Ok {
    fn succeeded(self) -> bool;
}

impl Win32Ok for windows::core::BOOL {
    fn succeeded(self) -> bool {
        self.as_bool()
    }
}

impl Win32Ok for windows::core::HRESULT {
    fn succeeded(self) -> bool {
        self.is_ok()
    }
}

impl<T> Win32Ok for windows::core::Result<T> {
    fn succeeded(self) -> bool {
        self.is_ok()
    }
}

/// Same idea for functions returning HANDLE or Result<HANDLE>.
pub(crate) trait IntoHandle {
    fn into_handle(self) -> Option<HANDLE>;
}

impl IntoHandle for HANDLE {
    fn into_handle(self) -> Option<HANDLE> {
        (!self.is_invalid()).then_some(self)
    }
}

impl IntoHandle for windows::core::Result<HANDLE> {
    fn into_handle(self) -> Option<HANDLE> {
        self.ok().filter(|h| !h.is_invalid())
    }
}

pub(crate) fn valid_handle(h: impl IntoHandle) -> Option<HANDLE> {
    h.into_handle()
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

fn wide_path(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// (free bytes available to the user, total bytes) for the volume containing `path`.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    // Walk up to an existing directory (the output folder may not exist yet).
    let mut p = path.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    let w = wide_path(&p);
    let mut free = 0u64;
    let mut total = 0u64;
    if !unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), Some(&mut total), None) }.succeeded() {
        return None;
    }
    Some((free, total))
}

fn reg_string(root: HKEY, key: &str, value: &str) -> Option<String> {
    let k = wide(key);
    let v = wide(value);
    let mut buf = vec![0u16; 512];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            root,
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut len),
        )
    };
    if !r.succeeded() {
        return None;
    }
    let n = (len as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n.min(buf.len())]))
}

/// Windows build number (19045 = Windows 10 22H2, 22000+ = Windows 11).
pub fn windows_build() -> Option<u32> {
    static BUILD: OnceLock<Option<u32>> = OnceLock::new();
    *BUILD.get_or_init(|| {
        reg_string(HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuildNumber")?
            .trim()
            .parse()
            .ok()
    })
}

/// Windows Graphics Capture needs Windows 10 version 1903 (build 18362) or newer.
pub fn supports_wgc() -> bool {
    windows_build().is_some_and(|b| b >= 18362)
}

/// Hiding a window from screen capture needs Windows 10 version 2004 (build 19041).
pub fn supports_exclude_from_capture() -> bool {
    windows_build().is_some_and(|b| b >= 19041)
}

pub fn set_dpi_aware() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Keep a window (e.g. the recording overlay) out of screen captures.
pub fn exclude_from_capture(hwnd: u64) -> bool {
    supports_exclude_from_capture()
        && unsafe { SetWindowDisplayAffinity(HWND(hwnd as usize as *mut c_void), WDA_EXCLUDEFROMCAPTURE) }.succeeded()
}

struct Job(HANDLE);
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

/// Tie a child process to this app: it is killed automatically if uRec exits or crashes.
pub fn bind_child_to_app(raw: *mut c_void) -> bool {
    static JOB: OnceLock<Option<Job>> = OnceLock::new();
    let job = JOB.get_or_init(|| unsafe {
        let job = valid_handle(CreateJobObjectW(None, PCWSTR::null()))?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if !SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .succeeded()
        {
            return None;
        }
        Some(Job(job))
    });
    match job {
        Some(j) => unsafe { AssignProcessToJobObject(j.0, HANDLE(raw)) }.succeeded(),
        None => false,
    }
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

pub fn startup_enabled(app: &str) -> bool {
    reg_string(HKEY_CURRENT_USER, RUN_KEY, app).is_some()
}

/// Add or remove the per-user "run at sign-in" entry.
pub fn set_startup(app: &str, command: Option<&str>) -> bool {
    unsafe {
        let key_w = wide(RUN_KEY);
        let name_w = wide(app);
        match command {
            Some(cmd) => {
                let mut key = HKEY::default();
                if !RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(key_w.as_ptr()), None, KEY_SET_VALUE, &mut key).succeeded()
                {
                    return false;
                }
                let data = wide(cmd);
                let bytes = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
                let ok = RegSetValueExW(key, PCWSTR(name_w.as_ptr()), None, REG_SZ, Some(bytes)).succeeded();
                let _ = RegCloseKey(key);
                ok
            }
            None => {
                RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(key_w.as_ptr()), PCWSTR(name_w.as_ptr())).succeeded()
                    || !startup_enabled(app)
            }
        }
    }
}

/// Open Explorer with a file selected (or a folder opened).
pub fn reveal_in_explorer(path: &Path) {
    let mut cmd = std::process::Command::new("explorer.exe");
    if path.is_file() {
        cmd.arg(format!("/select,{}", path.display()));
    } else {
        cmd.arg(path);
    }
    let _ = cmd.spawn();
}

/// Full path of a process's executable.
pub(crate) fn process_image(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let h = valid_handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid))?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).succeeded();
        let _ = CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}
