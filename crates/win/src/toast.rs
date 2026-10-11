//! Windows toast notifications (WinRT), used for Instant Replay results when the overlay is
//! off. uRec is a normal desktop app, so it registers a per-user AppUserModelID (display name
//! for the notification) and the `urec:` link scheme its buttons use. Both are removed by the
//! uninstaller.

use crate::sys::wide;
use std::path::Path;
use windows::core::{HSTRING, PCWSTR};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
    REG_SZ,
};
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

pub const AUMID: &str = "u2alr.uRec";

fn set_value(subkey: &str, name: Option<&str>, value: &str) -> bool {
    unsafe {
        let k = wide(subkey);
        let mut key = HKEY::default();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(k.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .is_err()
        {
            return false;
        }
        let n = name.map(wide);
        let data = wide(value);
        let bytes = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
        let ok = RegSetValueExW(
            key,
            n.as_ref().map(|n| PCWSTR(n.as_ptr())).unwrap_or(PCWSTR::null()),
            None,
            REG_SZ,
            Some(bytes),
        )
        .is_ok();
        let _ = RegCloseKey(key);
        ok
    }
}

/// Register the notification identity and the `urec:` scheme for this user, pointing at
/// `exe`. Cheap and idempotent; call at startup.
pub fn register_notifications(exe: &Path) -> bool {
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    let exe_s = exe.to_string_lossy();
    let id_key = format!(r"Software\Classes\AppUserModelId\{AUMID}");
    let mut ok = set_value(&id_key, Some("DisplayName"), "uRec");
    let proto = r"Software\Classes\urec";
    ok &= set_value(proto, None, "URL:uRec clip link");
    ok &= set_value(proto, Some("URL Protocol"), "");
    ok &= set_value(&format!(r"{proto}\DefaultIcon"), None, &format!("\"{exe_s}\",0"));
    ok &= set_value(&format!(r"{proto}\shell\open\command"), None, &format!("\"{exe_s}\" --clip-link \"%1\""));
    let id = wide(AUMID);
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(PCWSTR(id.as_ptr()));
    }
    ok
}

/// Show a toast. Errors mean Windows refused (notifications off for uRec, Focus assist,
/// or no notification platform); the caller keeps the in-app notice either way.
pub fn show_toast(xml: &str) -> Result<(), String> {
    let doc = XmlDocument::new().map_err(|e| e.message())?;
    doc.LoadXml(&HSTRING::from(xml)).map_err(|e| e.message())?;
    let toast = ToastNotification::CreateToastNotification(&doc).map_err(|e| e.message())?;
    let notifier =
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID)).map_err(|e| e.message())?;
    notifier.Show(&toast).map_err(|e| e.message())
}

/// How many uRec notifications are in Action Center (for tests).
pub fn toast_history_count() -> Option<u32> {
    let h = ToastNotificationManager::History().ok()?;
    h.GetHistoryWithId(&HSTRING::from(AUMID)).ok()?.Size().ok()
}
