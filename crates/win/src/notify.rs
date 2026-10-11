//! Windows notifications for Instant Replay results when the overlay is turned off.
//! This part is platform-neutral (and unit-tested): the toast XML and the `urec:` links its
//! buttons use. Showing the toast is in `toast.rs`.

use std::path::{Path, PathBuf};

/// The `urec:` link scheme, registered per user. It only opens or reveals an existing video
/// file inside the recordings folder; anything else is refused by the app.
pub const SCHEME: &str = "urec";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipAction {
    Open,
    Reveal,
}

impl ClipAction {
    fn verb(self) -> &'static str {
        match self {
            ClipAction::Open => "open",
            ClipAction::Reveal => "reveal",
        }
    }
}

fn pct_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn pct_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `urec://open?path=...`
pub fn clip_uri(action: ClipAction, path: &Path) -> String {
    format!("{SCHEME}://{}?path={}", action.verb(), pct_encode(&path.to_string_lossy()))
}

pub fn parse_clip_uri(uri: &str) -> Option<(ClipAction, PathBuf)> {
    let rest = uri.strip_prefix(&format!("{SCHEME}://"))?;
    let (verb, query) = rest.split_once('?')?;
    let action = match verb.trim_end_matches('/') {
        "open" => ClipAction::Open,
        "reveal" => ClipAction::Reveal,
        _ => return None,
    };
    let enc = query.strip_prefix("path=")?;
    if enc.contains('&') {
        return None;
    }
    let p = pct_decode(enc)?;
    if p.is_empty() || p.contains('\0') {
        return None;
    }
    Some((action, PathBuf::from(p)))
}

/// May this link be acted on? Only existing .mp4/.mkv files inside `recordings_dir`.
pub fn clip_allowed(path: &Path, recordings_dir: &Path) -> Result<(), &'static str> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext != "mp4" && ext != "mkv" {
        return Err("not a video");
    }
    if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("not a plain path");
    }
    let lower = |p: &Path| p.to_string_lossy().to_lowercase().replace('/', "\\");
    let mut root = lower(recordings_dir);
    if !root.ends_with('\\') {
        root.push('\\');
    }
    if !lower(path).starts_with(&root) {
        return Err("outside the recordings folder");
    }
    if !path.is_file() {
        return Err("missing");
    }
    Ok(())
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// Toast XML for a save result. Silent: uRec plays its own sound if that's turned on.
pub fn toast_xml(title: &str, body: &str, clip: Option<&Path>) -> String {
    let mut x = String::new();
    match clip {
        Some(p) => {
            x += &format!(
                "<toast activationType=\"protocol\" launch=\"{}\">",
                xml_escape(&clip_uri(ClipAction::Open, p))
            )
        }
        None => x += "<toast>",
    }
    x += &format!(
        "<visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual>",
        xml_escape(title),
        xml_escape(body)
    );
    if let Some(p) = clip {
        x += &format!(
            "<actions><action content=\"Open clip\" activationType=\"protocol\" arguments=\"{}\"/>\
             <action content=\"Show in folder\" activationType=\"protocol\" arguments=\"{}\"/></actions>",
            xml_escape(&clip_uri(ClipAction::Open, p)),
            xml_escape(&clip_uri(ClipAction::Reveal, p))
        );
    }
    x += "<audio silent=\"true\"/></toast>";
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_round_trip() {
        let p = Path::new(r"C:\Users\Ana María\Videos\uRec\VALORANT\VALORANT 2026-10-12 01.02.03 Replay (2).mp4");
        let u = clip_uri(ClipAction::Reveal, p);
        assert!(u.starts_with("urec://reveal?path=C%3A%5CUsers%5CAna%20Mar%C3%ADa"));
        assert!(!u.contains(' ') && !u.contains('"') && !u.contains('&'));
        assert_eq!(parse_clip_uri(&u), Some((ClipAction::Reveal, p.to_path_buf())));
        assert_eq!(parse_clip_uri(&clip_uri(ClipAction::Open, p)).unwrap().0, ClipAction::Open);
        // Windows passes the link with a trailing slash sometimes.
        assert!(parse_clip_uri("urec://open/?path=C%3A%5Cx.mp4").is_some());
    }

    #[test]
    fn rejects_malformed_links() {
        for bad in [
            "urec://delete?path=C%3A%5Cx.mp4",
            "urec://open?path=",
            "urec://open?path=C%3A%5Cx.mp4&run=calc",
            "urec://open?path=%ZZ",
            "https://open?path=x",
            "urec://open",
        ] {
            assert_eq!(parse_clip_uri(bad), None, "{bad}");
        }
    }

    #[test]
    fn only_videos_inside_the_recordings_folder() {
        let d = std::env::temp_dir().join(format!("urec-notify-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let clip = d.join("a.mp4");
        std::fs::write(&clip, b"x").unwrap();
        std::fs::write(d.join("run.bat"), b"x").unwrap();
        assert_eq!(clip_allowed(&clip, &d), Ok(()));
        assert_eq!(clip_allowed(&d.join("run.bat"), &d), Err("not a video"));
        assert_eq!(clip_allowed(&d.join("gone.mp4"), &d), Err("missing"));
        assert_eq!(clip_allowed(&d.join("..").join("a.mp4"), &d), Err("not a plain path"));
        assert_eq!(clip_allowed(&clip, &d.join("other")), Err("outside the recordings folder"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn xml_is_escaped_and_has_actions() {
        let x = toast_xml(
            "Instant Replay Saved",
            "0:30 · Tom & Jerry <1>.mp4",
            Some(Path::new("C:/v/Tom & Jerry <1>.mp4")),
        );
        assert!(x.contains("Tom &amp; Jerry &lt;1&gt;.mp4"));
        assert!(x.contains("content=\"Open clip\"") && x.contains("content=\"Show in folder\""));
        assert!(x.contains("activationType=\"protocol\""));
        let f = toast_xml("Instant Replay Failed", "Disk full", None);
        assert!(!f.contains("<actions>") && f.starts_with("<toast>"));
    }
}
