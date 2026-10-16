//! Output file names: safe characters, timestamps, never overwriting.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Remove characters Windows doesn't allow in file names and tidy whitespace.
pub fn sanitize(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c }).collect();
    let mut out = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    out = out.trim_matches(|c| c == '.' || c == ' ').to_string();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
        "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    // Windows treats "CON.anything" as the device too, so only the part before the first dot counts.
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    if out.is_empty() || RESERVED.contains(&stem.as_str()) {
        out = format!("Recording {out}").trim().to_string();
    }
    out.chars().take(80).collect::<String>().trim().to_string()
}

/// Local calendar time parts (year, month, day, hour, minute, second).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Stamp {
    /// From seconds since the Unix epoch plus a UTC offset in seconds.
    pub fn from_unix(secs: i64, utc_offset: i32) -> Stamp {
        let t = secs + utc_offset as i64;
        let days = t.div_euclid(86_400);
        let rem = t.rem_euclid(86_400);
        // Howard Hinnant's civil_from_days
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        Stamp {
            year: (y + i64::from(m <= 2)) as i32,
            month: m as u32,
            day: d as u32,
            hour: (rem / 3600) as u32,
            minute: (rem % 3600 / 60) as u32,
            second: (rem % 60) as u32,
        }
    }

    pub fn now(utc_offset: i32) -> Stamp {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        Stamp::from_unix(secs, utc_offset)
    }

    /// "2026-10-11 15.02.11" — sorts correctly and is valid on every file system.
    pub fn file_part(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}.{:02}.{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

/// `dir/<prefix> <stamp><suffix>.<ext>`, adding " (2)", " (3)"... if the name is taken.
pub fn unique_path(dir: &Path, prefix: &str, stamp: &Stamp, suffix: &str, ext: &str) -> PathBuf {
    let base = format!("{} {}{}", sanitize(prefix), stamp.file_part(), suffix);
    let mut candidate = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes() {
        assert_eq!(sanitize("VALORANT  "), "VALORANT");
        assert_eq!(sanitize("a<b>c:d\"e/f\\g|h?i*j"), "a b c d e f g h i j");
        assert_eq!(sanitize("con"), "Recording con");
        // Windows reserves "CON.txt" and the like too.
        assert_eq!(sanitize("con.txt"), "Recording con.txt");
        assert_eq!(sanitize("Lpt1.backup"), "Recording Lpt1.backup");
        assert_eq!(sanitize("console"), "console");
        assert_eq!(sanitize("..."), "Recording");
        assert_eq!(sanitize("Notes\u{7}.txt."), "Notes .txt");
    }

    #[test]
    fn stamps() {
        // 2026-10-11 07:51:03 UTC = 1791705063
        let s = Stamp::from_unix(1_791_705_063, 8 * 3600);
        assert_eq!(s.file_part(), "2026-10-11 15.51.03");
        let leap = Stamp::from_unix(951_782_400, 0); // 2000-02-29
        assert_eq!((leap.year, leap.month, leap.day), (2000, 2, 29));
    }

    #[test]
    fn unique_names() {
        let dir = std::env::temp_dir().join(format!("urec-naming-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Stamp::from_unix(0, 0);
        let a = unique_path(&dir, "Desktop", &s, "", "mp4");
        std::fs::write(&a, b"x").unwrap();
        let b = unique_path(&dir, "Desktop", &s, "", "mp4");
        assert!(b.to_string_lossy().ends_with("(2).mp4"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
