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

impl Win32Ok for windows::Win32::Foundation::WIN32_ERROR {
    fn succeeded(self) -> bool {
        self.0 == 0
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

/// Show a file selected in its Explorer folder, or open a folder.
pub fn reveal_in_explorer(path: &Path) {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems};
    if path.is_file() {
        let w = wide_path(path);
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let pidl = ILCreateFromPathW(PCWSTR(w.as_ptr()));
            if !pidl.is_null() {
                let ok = SHOpenFolderAndSelectItems(pidl, None, 0).succeeded();
                ILFree(Some(pidl));
                if ok {
                    return;
                }
            }
        }
        // Fall back to opening the containing folder.
        if let Some(dir) = path.parent() {
            open_file(dir);
        }
    } else {
        let _ = std::fs::create_dir_all(path);
        open_file(path);
    }
}

/// Open a file or folder with its default app (videos play, folders open in Explorer).
pub fn open_file(path: &Path) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let w = wide_path(path);
    let verb = wide("open");
    unsafe {
        ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(w.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// The user's Videos folder, wherever Windows or OneDrive put it.
pub fn videos_dir() -> Option<std::path::PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Videos, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Videos, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.map(std::path::PathBuf::from)
    }
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

/// Local wall-clock time: (year, month, day, hour, minute, second).
pub fn local_time() -> (i32, u32, u32, u32, u32, u32) {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    (t.wYear as i32, t.wMonth as u32, t.wDay as u32, t.wHour as u32, t.wMinute as u32, t.wSecond as u32)
}

/// Short confirmation sound (the Windows "notification" sound).
pub fn play_ding() {
    unsafe {
        let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(
            windows::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK,
        );
    }
}

/// Move a file to the Recycle Bin (never deletes permanently).
pub fn recycle(path: &Path) -> bool {
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW,
    };
    let mut from: Vec<u16> = wide_path(path);
    from.push(0); // double-null terminated list
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    unsafe { SHFileOperationW(&mut op) == 0 && !op.fAnyOperationsAborted.as_bool() }
}

/// Bring a window titled `title` to the front (used by the tray and second-instance launch).
pub fn show_window_titled(title: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};
    let t = wide(title);
    unsafe {
        let Ok(h) = FindWindowW(PCWSTR::null(), PCWSTR(t.as_ptr())) else { return false };
        if h.0.is_null() {
            return false;
        }
        let _ = ShowWindow(h, SW_SHOW);
        let _ = ShowWindow(h, SW_RESTORE);
        let _ = SetForegroundWindow(h);
        true
    }
}

/// Hide a window titled `title` (minimize-to-tray).
pub fn hide_window_titled(title: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, ShowWindow, SW_HIDE};
    let t = wide(title);
    unsafe {
        match FindWindowW(PCWSTR::null(), PCWSTR(t.as_ptr())) {
            Ok(h) if !h.0.is_null() => ShowWindow(h, SW_HIDE).as_bool() || true,
            _ => false,
        }
    }
}

/// Native handle of a window with this exact title.
pub fn find_window(title: &str) -> Option<u64> {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    let t = wide(title);
    unsafe {
        FindWindowW(PCWSTR::null(), PCWSTR(t.as_ptr())).ok().filter(|h| !h.0.is_null()).map(|h| h.0 as usize as u64)
    }
}

/// Single-instance guard. Returns false if another uRec is already running.
pub fn claim_single_instance(name: &str) -> bool {
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    let n = wide(name);
    unsafe {
        let h = CreateMutexW(None, true, PCWSTR(n.as_ptr()));
        let exists = GetLastError() == ERROR_ALREADY_EXISTS;
        // Keep the mutex for the life of the process.
        std::mem::forget(h);
        !exists
    }
}

/// Windows folder picker. Returns None if cancelled. Call from the UI thread.
pub fn pick_folder(title: &str, start: Option<&Path>) -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS,
        SIGDN_FILESYSPATH,
    };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dialog.GetOptions().ok()?;
        dialog.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        let t = wide(title);
        let _ = dialog.SetTitle(PCWSTR(t.as_ptr()));
        if let Some(dir) = start.filter(|d| d.exists()) {
            let w = wide_path(dir);
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(w.as_ptr()), None) {
                let _ = dialog.SetFolder(&item);
            }
        }
        dialog.Show(None).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = name.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(name.0 as *const _));
        s.map(std::path::PathBuf::from)
    }
}
