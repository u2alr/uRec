//! Update checks and automatic updates.
//!
//! Once a day (and when asked) uRec reads the latest release of github.com/u2alr/uRec. If it is
//! newer, the installer is downloaded in the background to %LOCALAPPDATA%\uRec\updates and
//! verified against the release's SHA256SUMS.txt. A verified installer is run silently when uRec
//! quits, the next time it starts, or when "Restart now" is pressed — never during a recording.
//! Portable copies (no installer next to the exe) are only told that an update exists.
//!
//! Nothing is sent except a normal HTTPS request to api.github.com / github.com with a
//! "uRec/<version>" User-Agent.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const REPO: &str = "u2alr/uRec";
pub const RELEASES_PAGE: &str = "https://github.com/u2alr/uRec/releases/latest";
const DAY: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Default)]
pub enum State {
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// Newer version exists but won't be installed automatically (portable copy or auto-update off).
    Available {
        version: String,
        url: String,
    },
    Downloading {
        version: String,
        percent: Option<u8>,
    },
    /// Downloaded and verified; installs on quit/next start or via "Restart now".
    Ready {
        version: String,
    },
    /// The last automatic install of this version didn't take.
    InstallFailed {
        version: String,
    },
    Failed(String),
}

enum Msg {
    Check,
    Configure { check: bool, auto: bool },
}

#[derive(Clone)]
pub struct Updater {
    pub state: Arc<Mutex<State>>,
    tx: Arc<Mutex<Sender<Msg>>>,
}

impl Updater {
    pub fn start(check: bool, auto: bool, repaint: Arc<dyn Fn() + Send + Sync>) -> Updater {
        let (tx, rx) = channel();
        let state = Arc::new(Mutex::new(initial_state()));
        let u = Updater { state: state.clone(), tx: Arc::new(Mutex::new(tx)) };
        let _ =
            std::thread::Builder::new().name("updater".into()).spawn(move || worker(rx, state, check, auto, repaint));
        u
    }

    pub fn check_now(&self) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(Msg::Check);
        }
    }

    pub fn configure(&self, check: bool, auto: bool) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(Msg::Configure { check, auto });
        }
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// The version this build reports. `UREC_FAKE_VERSION` exists for the end-to-end update test.
pub fn current_version() -> String {
    std::env::var("UREC_FAKE_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

/// Parse "v1.2.3" / "1.2.3". Pre-release suffixes ("-rc1") are not accepted.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let mut it = s.split('.');
    let v = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    if it.next().is_some() {
        return None;
    }
    Some(v)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn updates_dir() -> PathBuf {
    crate::settings::local_dir().join("updates")
}

/// Installed copies have Inno Setup's uninstaller next to the exe; portable ones don't.
pub fn is_installed() -> bool {
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("unins000.exe").exists())).unwrap_or(false)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

// ------------------------------------------------------------------ pending update marker

struct Pending {
    version: String,
    file: PathBuf,
    sha256: String,
}

fn pending_path() -> PathBuf {
    updates_dir().join("pending.txt")
}

fn read_pending() -> Option<Pending> {
    let text = read(&pending_path())?;
    let mut l = text.lines();
    let version = l.next()?.trim().to_string();
    let name = l.next()?.trim().to_string();
    let sha256 = l.next()?.trim().to_uppercase();
    // Only a plain file name inside the updates folder.
    if name.contains(['/', '\\', ':']) || name.starts_with('.') {
        return None;
    }
    Some(Pending { version, file: updates_dir().join(name), sha256 })
}

fn write_pending(p: &Pending) -> std::io::Result<()> {
    let name = p.file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    std::fs::write(pending_path(), format!("{}\n{}\n{}\n", p.version, name, p.sha256))
}

fn initial_state() -> State {
    let current = current_version();
    let attempted = read(&updates_dir().join("attempted.txt"));
    if let Some(p) = read_pending() {
        if is_newer(&p.version, &current) && p.file.exists() {
            return State::Ready { version: p.version };
        }
    }
    if let Some(v) = attempted {
        if is_newer(&v, &current) {
            return State::InstallFailed { version: v };
        }
        // Installed fine: tidy up.
        cleanup();
    }
    State::Idle
}

/// Remove downloaded installers and markers (after a successful update).
fn cleanup() {
    let _ = std::fs::remove_dir_all(updates_dir());
}

/// A verified installer is waiting and auto-update applies to this copy.
pub fn has_pending() -> bool {
    is_installed() && read_pending().is_some_and(|p| is_newer(&p.version, &current_version()) && p.file.exists())
}

/// How the app should come back after installing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Relaunch {
    No,
    Normal,
    Minimized,
}

/// Run the pending installer silently. Returns true if it was started (the caller should exit
/// right away so files can be replaced). Re-verifies the checksum first.
pub fn apply_pending(relaunch: Relaunch) -> bool {
    if !is_installed() {
        return false;
    }
    let Some(p) = read_pending() else { return false };
    let _ = std::fs::remove_file(pending_path());
    if !is_newer(&p.version, &current_version()) {
        return false;
    }
    match urec_engine::sha256::digest_file(&p.file) {
        Ok(h) if h == p.sha256 => {}
        other => {
            crate::controller::log(&format!("update: installer checksum mismatch before install ({other:?})"));
            let _ = std::fs::remove_file(&p.file);
            return false;
        }
    }
    let _ = std::fs::write(updates_dir().join("attempted.txt"), &p.version);
    let mut args: Vec<String> =
        ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/update=1"].iter().map(|s| s.to_string()).collect();
    // Keep the install mode the user picked originally.
    let per_user = std::env::current_exe()
        .ok()
        .zip(std::env::var_os("LOCALAPPDATA"))
        .is_some_and(|(exe, local)| exe.starts_with(local));
    args.push(if per_user { "/CURRENTUSER" } else { "/ALLUSERS" }.into());
    match relaunch {
        Relaunch::No => {}
        Relaunch::Normal => args.push("/relaunch=1".into()),
        Relaunch::Minimized => args.push("/relaunch=min".into()),
    }
    let log = updates_dir().join("install.log");
    args.push(format!("/LOG={}", log.display()));
    crate::controller::log(&format!("update: installing {} ({})", p.version, args.join(" ")));
    match std::process::Command::new(&p.file).args(&args).spawn() {
        Ok(_) => true,
        Err(e) => {
            crate::controller::log(&format!("update: couldn't start the installer: {e}"));
            false
        }
    }
}

/// Called when uRec is quitting (after recordings are finished): install a waiting update if
/// automatic updates are on. Returns true if the installer was started.
pub fn apply_on_quit(relaunch: Relaunch) -> bool {
    let (settings, _) = crate::settings::Settings::load();
    settings.general.auto_update && has_pending() && apply_pending(relaunch)
}

// ------------------------------------------------------------------ checking + downloading

struct Release {
    version: String,
    page: String,
    setup: Option<(String, String)>, // (name, url)
    sums: Option<String>,            // url
    sums_sig: Option<String>,        // url
}

/// Public key (64-byte P-256 point as hex) that release checksums are signed with. Empty until
/// the project creates one (ci/new-update-key.ps1); then every update must carry a valid
/// signature, on top of the SHA-256 check.
const UPDATE_KEY: &str = include_str!("../assets/update-key.pub");

/// Check `SHA256SUMS.txt` against its signature. `verify` does the curve maths (Windows CNG).
pub fn check_signature(
    key_hex: &str,
    sums: &[u8],
    sig_hex: Option<&str>,
    verify: &dyn Fn(&[u8; 64], &[u8; 32], &[u8; 64]) -> bool,
) -> Result<(), String> {
    let key_hex = key_hex.trim();
    if key_hex.is_empty() {
        return Ok(()); // no signing key yet: checksum-only
    }
    let key: [u8; 64] = urec_engine::sha256::from_hex(key_hex)
        .and_then(|v| v.try_into().ok())
        .ok_or("this build's update key is malformed")?;
    let sig_hex = sig_hex.ok_or("the release isn't signed")?;
    let sig: [u8; 64] = urec_engine::sha256::from_hex(sig_hex)
        .and_then(|v| v.try_into().ok())
        .ok_or("the release signature is malformed")?;
    let hash = urec_engine::sha256::digest_bytes(sums);
    if verify(&key, &hash, &sig) {
        Ok(())
    } else {
        Err("the release signature doesn't match uRec's key".into())
    }
}

fn user_agent() -> String {
    format!("uRec/{} (+https://github.com/{REPO})", env!("CARGO_PKG_VERSION"))
}

fn allowed_download(url: &str) -> bool {
    url.starts_with(&format!("https://github.com/{REPO}/releases/download/"))
}

fn latest_release() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut headers =
        vec!["Accept: application/vnd.github+json".to_string(), "X-GitHub-Api-Version: 2022-11-28".into()];
    // CI only: avoids the shared-runner rate limit. Never set for normal users.
    if let Ok(t) = std::env::var("UREC_GITHUB_TOKEN") {
        if !t.is_empty() {
            headers.push(format!("Authorization: Bearer {t}"));
        }
    }
    let headers: Vec<&str> = headers.iter().map(|s| s.as_str()).collect();
    let body = urec_win::https_get(&url, &user_agent(), &headers, 2 << 20)?;
    parse_release(&body)
}

fn parse_release(body: &[u8]) -> Result<Release, String> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("unexpected reply from GitHub: {e}"))?;
    let tag = v["tag_name"].as_str().ok_or("unexpected reply from GitHub (no tag)")?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return Err("latest release is not final".into());
    }
    let version = tag.trim_start_matches('v').to_string();
    parse_version(&version).ok_or_else(|| format!("unrecognised release tag {tag}"))?;
    let page = v["html_url"].as_str().unwrap_or(RELEASES_PAGE).to_string();
    let mut setup = None;
    let mut sums = None;
    let mut sums_sig = None;
    for a in v["assets"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let (Some(name), Some(url)) = (a["name"].as_str(), a["browser_download_url"].as_str()) else { continue };
        if !allowed_download(url) {
            continue;
        }
        if name == format!("uRec-{version}-setup.exe") {
            setup = Some((name.to_string(), url.to_string()));
        } else if name == "SHA256SUMS.txt" {
            sums = Some(url.to_string());
        } else if name == "SHA256SUMS.txt.sig" {
            sums_sig = Some(url.to_string());
        }
    }
    Ok(Release { version, page, setup, sums, sums_sig })
}

/// Find `name` in a sha256sum-style listing ("<hex>  <name>" or "<hex> *<name>").
pub fn find_sum(listing: &str, name: &str) -> Option<String> {
    listing.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash.to_uppercase())
    })
}

fn set(state: &Mutex<State>, s: State, repaint: &(dyn Fn() + Send + Sync)) {
    if let Ok(mut g) = state.lock() {
        if *g != s {
            *g = s;
            drop(g);
            repaint();
        }
    }
}

/// One check (+ download if allowed). Returns the resulting state.
fn check_once(auto: bool, state: &Mutex<State>, repaint: &(dyn Fn() + Send + Sync)) -> State {
    let current = current_version();
    let _ = std::fs::create_dir_all(updates_dir());
    let _ = std::fs::write(updates_dir().join("last_check.txt"), now().to_string());
    let rel = match latest_release() {
        Ok(r) => r,
        Err(e) => return State::Failed(e),
    };
    if !is_newer(&rel.version, &current) {
        return State::UpToDate;
    }
    let attempted = read(&updates_dir().join("attempted.txt"));
    if attempted.as_deref() == Some(rel.version.as_str()) {
        return State::InstallFailed { version: rel.version };
    }
    if !auto || !is_installed() {
        return State::Available { version: rel.version, url: rel.page };
    }
    let (Some((name, url)), Some(sums_url)) = (rel.setup, rel.sums) else {
        // Releases without a checksum file are never installed automatically.
        return State::Available { version: rel.version, url: rel.page };
    };
    if let Some(p) = read_pending() {
        if p.version == rel.version && p.file.exists() {
            return State::Ready { version: rel.version };
        }
    }
    let sums_bytes = match urec_win::https_get(&sums_url, &user_agent(), &[], 64 * 1024) {
        Ok(b) => b,
        Err(e) => return State::Failed(format!("Couldn't download the checksums: {e}")),
    };
    // Signed releases: the checksum list itself must carry a valid signature.
    if !UPDATE_KEY.trim().is_empty() {
        let sig = match &rel.sums_sig {
            Some(u) => match urec_win::https_get(u, &user_agent(), &[], 4096) {
                Ok(b) => Some(String::from_utf8_lossy(&b).to_string()),
                Err(e) => return State::Failed(format!("Couldn't download the signature: {e}")),
            },
            None => None,
        };
        if let Err(e) = check_signature(UPDATE_KEY, &sums_bytes, sig.as_deref(), &urec_win::verify_p256) {
            crate::controller::log(&format!("update: refused {}: {e}", rel.version));
            return State::Failed(format!("Update refused: {e}."));
        }
    }
    let sums = String::from_utf8_lossy(&sums_bytes).to_string();
    let Some(expected) = find_sum(&sums, &name) else {
        return State::Failed(format!("{name} isn't listed in SHA256SUMS.txt"));
    };
    let dest = updates_dir().join(&name);
    let part = updates_dir().join(format!("{name}.part"));
    let version = rel.version.clone();
    set(state, State::Downloading { version: version.clone(), percent: Some(0) }, repaint);
    let mut last = 0u8;
    let res = urec_win::https_download(&url, &user_agent(), &part, &mut |done, total| {
        let pct = total.filter(|t| *t > 0).map(|t| (done * 100 / t).min(100) as u8);
        if let Some(p) = pct {
            if p >= last + 5 || p == 100 {
                last = p;
                set(state, State::Downloading { version: version.clone(), percent: Some(p) }, repaint);
            }
        }
    });
    if let Err(e) = res {
        let _ = std::fs::remove_file(&part);
        return State::Failed(format!("Download failed: {e}"));
    }
    // Test hook: damage the download to prove verification rejects it.
    if std::env::var("UREC_UPDATE_TEST_CORRUPT").is_ok() {
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&part) {
            use std::io::Write;
            let _ = f.write_all(b"x");
        }
    }
    match urec_engine::sha256::digest_file(&part) {
        Ok(h) if h == expected => {}
        Ok(h) => {
            let _ = std::fs::remove_file(&part);
            crate::controller::log(&format!("update: checksum mismatch for {name}: got {h}, expected {expected}"));
            return State::Failed("The download didn't match its published checksum and was deleted.".into());
        }
        Err(e) => return State::Failed(format!("Couldn't verify the download: {e}")),
    }
    let _ = std::fs::remove_file(&dest);
    if let Err(e) = std::fs::rename(&part, &dest) {
        return State::Failed(format!("Couldn't save the update: {e}"));
    }
    if let Err(e) = write_pending(&Pending { version: rel.version.clone(), file: dest, sha256: expected }) {
        return State::Failed(format!("Couldn't save the update: {e}"));
    }
    crate::controller::log(&format!("update: {} downloaded and verified", rel.version));
    State::Ready { version: rel.version }
}

fn worker(
    rx: Receiver<Msg>,
    state: Arc<Mutex<State>>,
    mut check: bool,
    mut auto: bool,
    repaint: Arc<dyn Fn() + Send + Sync>,
) {
    // Wait two minutes so the check never competes with startup or a game that is launching.
    let delay = std::env::var("UREC_UPDATE_DELAY").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
    let mut wait = Duration::from_secs(delay);
    loop {
        let manual = match rx.recv_timeout(wait) {
            Ok(Msg::Check) => true,
            Ok(Msg::Configure { check: c, auto: a }) => {
                check = c;
                auto = a;
                false
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        wait = Duration::from_secs(60 * 60);
        if !manual {
            let last = read(&updates_dir().join("last_check.txt")).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            if !check || now().saturating_sub(last) < DAY {
                continue;
            }
        }
        if matches!(state.lock().map(|s| s.clone()), Ok(State::Ready { .. }) | Ok(State::Downloading { .. })) && !manual
        {
            continue;
        }
        set(&state, State::Checking, &*repaint);
        let result = check_once(auto, &state, &*repaint);
        if let State::Failed(e) = &result {
            crate::controller::log(&format!("update: check failed: {e}"));
        }
        // An automatic check that fails stays quiet; only manual checks show the error.
        let shown = match (&result, manual) {
            (State::Failed(_), false) => State::Idle,
            _ => result,
        };
        set(&state, shown, &*repaint);
    }
}

/// Headless `uRec.exe --update-check`: check, download and verify, print the outcome.
/// Exit code 0 = verified update ready (or already up to date), 1 = anything else.
pub fn cli_check() -> i32 {
    let state = Mutex::new(State::Idle);
    let r = check_once(true, &state, &|| {});
    println!("update-check current={} result={r:?}", current_version());
    crate::controller::log(&format!("update: cli check -> {r:?}"));
    match r {
        State::Ready { .. } | State::UpToDate => 0,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v0.1.2", "0.1.1"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.1", "0.1.1"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("0.1.2-rc1", "0.1.1"));
        assert!(!is_newer("garbage", "0.1.1"));
        assert_eq!(parse_version("v10.20.30"), Some((10, 20, 30)));
        assert_eq!(parse_version("1.2"), None);
    }

    #[test]
    fn sums() {
        let h = "a".repeat(64);
        let listing = format!("{h}  uRec-0.1.2-setup.exe\n{}  uRec-0.1.2-portable.zip\n", "b".repeat(64));
        assert_eq!(find_sum(&listing, "uRec-0.1.2-setup.exe"), Some("A".repeat(64)));
        assert_eq!(find_sum(&format!("{h} *x.exe"), "x.exe"), Some("A".repeat(64)));
        assert_eq!(find_sum(&listing, "other.exe"), None);
        assert_eq!(find_sum("abc  x.exe", "x.exe"), None);
    }

    #[test]
    fn release_json() {
        let body = br#"{"tag_name":"v0.1.2","draft":false,"prerelease":false,"html_url":"https://github.com/u2alr/uRec/releases/tag/v0.1.2",
          "assets":[
            {"name":"uRec-0.1.2-setup.exe","browser_download_url":"https://github.com/u2alr/uRec/releases/download/v0.1.2/uRec-0.1.2-setup.exe"},
            {"name":"SHA256SUMS.txt","browser_download_url":"https://github.com/u2alr/uRec/releases/download/v0.1.2/SHA256SUMS.txt"},
            {"name":"uRec-0.1.2-setup.exe","browser_download_url":"https://evil.example/x.exe"}]}"#;
        let r = parse_release(body).unwrap();
        assert_eq!(r.version, "0.1.2");
        assert!(r.setup.unwrap().1.starts_with("https://github.com/u2alr/uRec/"));
        assert!(r.sums.is_some());
        let pre = br#"{"tag_name":"v0.2.0","prerelease":true,"assets":[]}"#;
        assert!(parse_release(pre).is_err());
    }

    #[test]
    fn signature_rules() {
        let key = "ab".repeat(64);
        let sig = "cd".repeat(64);
        let yes = |_: &[u8; 64], _: &[u8; 32], _: &[u8; 64]| true;
        let no = |_: &[u8; 64], _: &[u8; 32], _: &[u8; 64]| false;
        // No key compiled in: checksum-only, as before.
        assert!(check_signature("", b"sums", None, &no).is_ok());
        // With a key: a signature is required and must verify.
        assert!(check_signature(&key, b"sums", Some(&sig), &yes).is_ok());
        assert!(check_signature(&key, b"sums", None, &yes).unwrap_err().contains("isn't signed"));
        assert!(check_signature(&key, b"sums", Some(&sig), &no).unwrap_err().contains("doesn't match"));
        assert!(check_signature(&key, b"sums", Some("12"), &yes).unwrap_err().contains("malformed"));
        assert!(check_signature("abcd", b"sums", Some(&sig), &yes).is_err());
        // The hash passed to the verifier is SHA-256 of the checksum file.
        let seen = std::cell::Cell::new([0u8; 32]);
        let spy = |_: &[u8; 64], h: &[u8; 32], _: &[u8; 64]| {
            seen.set(*h);
            true
        };
        check_signature(&key, b"abc", Some(&sig), &spy).unwrap();
        assert_eq!(urec_engine::sha256::hex(&seen.get()), urec_engine::sha256::digest(b"abc"));
    }
}
