//! Minimal HTTPS GET using Windows' own WinHTTP (system TLS and proxy settings, no extra
//! dependencies). Used only by the update checker. HTTPS only; redirects are followed.

use crate::sys::{wide, Win32Ok};
use std::ffi::c_void;
use std::io::Write;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Networking::WinHttp::{
    WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts, INTERNET_DEFAULT_HTTPS_PORT,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_CONTENT_LENGTH,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};

/// Receives each chunk of the body plus the total length, if the server sent one.
type Sink<'a> = &'a mut dyn FnMut(&[u8], Option<u64>) -> Result<(), String>;

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn last_error(what: &str) -> String {
    let code = unsafe { windows::Win32::Foundation::GetLastError() }.0;
    // WinHTTP's own codes (12001-12192) have no system message text.
    let text = match code {
        12002 => "the connection timed out".to_string(),
        12007 => "the server name couldn't be resolved (offline?)".to_string(),
        12029 | 12030 => "couldn't connect".to_string(),
        12175 | 12038 | 12044 | 12045 | 12057 => "the secure connection couldn't be verified".to_string(),
        _ => windows::core::HRESULT::from_win32(code).message().trim().to_string(),
    };
    format!("{what} failed: {text} (error {code})")
}

/// Split "https://host/path?query" into (host, path).
fn split_url(url: &str) -> Result<(String, String), String> {
    let rest = url.strip_prefix("https://").ok_or("only https:// URLs are allowed")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if host.is_empty() || host.contains(':') || host.contains('@') {
        return Err(format!("unsupported URL host: {host}"));
    }
    Ok((host.to_string(), path.to_string()))
}

/// Perform a GET and stream the body to `sink`. Returns (status, content length if known).
fn get(url: &str, user_agent: &str, headers: &[&str], sink: Sink<'_>) -> Result<u32, String> {
    let (host, path) = split_url(url)?;
    let ua = wide(user_agent);
    let host_w = wide(&host);
    let path_w = wide(&path);
    let verb = wide("GET");
    unsafe {
        let session = Handle(WinHttpOpen(
            PCWSTR(ua.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err(last_error("WinHttpOpen"));
        }
        // resolve, connect, send, receive (ms)
        let _ = WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 60_000);
        let connect = Handle(WinHttpConnect(session.0, PCWSTR(host_w.as_ptr()), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if connect.0.is_null() {
            return Err(last_error("WinHttpConnect"));
        }
        let request = Handle(WinHttpOpenRequest(
            connect.0,
            PCWSTR(verb.as_ptr()),
            PCWSTR(path_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(last_error("WinHttpOpenRequest"));
        }
        for h in headers {
            let hw: Vec<u16> = h.encode_utf16().collect();
            let _ = WinHttpAddRequestHeaders(request.0, &hw, WINHTTP_ADDREQ_FLAG_ADD);
        }
        if !WinHttpSendRequest(request.0, None, None, 0, 0, 0).succeeded() {
            return Err(last_error("Sending the request"));
        }
        if !WinHttpReceiveResponse(request.0, std::ptr::null_mut()).succeeded() {
            return Err(last_error("Receiving the response"));
        }
        let mut status: u32 = 0;
        let mut len = 4u32;
        let _ = WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        );
        let mut content_len: u32 = 0;
        let mut len = 4u32;
        let known = WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut content_len as *mut u32 as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        )
        .succeeded();
        let total = known.then_some(content_len as u64);
        if status != 200 {
            return Ok(status);
        }
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            if !WinHttpReadData(request.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut read).succeeded() {
                return Err(last_error("Reading the download"));
            }
            if read == 0 {
                break;
            }
            sink(&buf[..read as usize], total)?;
        }
        Ok(status)
    }
}

/// GET a small resource (e.g. JSON) into memory, refusing anything over `max_bytes`.
pub fn https_get(url: &str, user_agent: &str, headers: &[&str], max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    let status = get(url, user_agent, headers, &mut |chunk, _| {
        if body.len() + chunk.len() > max_bytes {
            return Err("response too large".into());
        }
        body.extend_from_slice(chunk);
        Ok(())
    })?;
    if status != 200 {
        return Err(format!("server answered HTTP {status}"));
    }
    Ok(body)
}

/// Download to `dest`, reporting (bytes so far, total if known). Returns bytes written.
pub fn https_download(
    url: &str,
    user_agent: &str,
    dest: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<u64, String> {
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let mut file = std::fs::File::create(dest).map_err(|e| format!("Can't write {}: {e}", dest.display()))?;
    let mut done = 0u64;
    let status = get(url, user_agent, &["Accept: application/octet-stream"], &mut |chunk, total| {
        file.write_all(chunk).map_err(|e| e.to_string())?;
        done += chunk.len() as u64;
        progress(done, total);
        Ok(())
    })?;
    file.flush().map_err(|e| e.to_string())?;
    if status != 200 {
        let _ = std::fs::remove_file(dest);
        return Err(format!("server answered HTTP {status}"));
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::split_url;

    #[test]
    fn urls() {
        assert_eq!(
            split_url("https://api.github.com/repos/u2alr/uRec/releases/latest").unwrap(),
            ("api.github.com".into(), "/repos/u2alr/uRec/releases/latest".into())
        );
        assert_eq!(split_url("https://example.com").unwrap(), ("example.com".into(), "/".into()));
        assert!(split_url("http://example.com/x").is_err());
        assert!(split_url("https://user@evil.com/x").is_err());
    }
}
