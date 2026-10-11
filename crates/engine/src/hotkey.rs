//! Hotkey text format ("Alt+F10") <-> Win32 modifier flags and virtual-key codes.

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub mods: u32,
    pub vk: u32,
}

const NAMED: &[(&str, u32)] = &[
    ("Space", 0x20),
    ("PgUp", 0x21),
    ("PgDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("PrtSc", 0x2C),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("Pause", 0x13),
    ("ScrollLock", 0x91),
    ("`", 0xC0),
    ("-", 0xBD),
    ("=", 0xBB),
    ("[", 0xDB),
    ("]", 0xDD),
    (";", 0xBA),
    ("'", 0xDE),
    (",", 0xBC),
    (".", 0xBE),
    ("/", 0xBF),
    ("\\", 0xDC),
];

impl Hotkey {
    pub fn parse(text: &str) -> Option<Hotkey> {
        let parts: Vec<&str> = text.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
        let (key, mods_txt) = parts.split_last()?;
        let mut mods = 0;
        for m in mods_txt {
            mods |= match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => MOD_CONTROL,
                "alt" => MOD_ALT,
                "shift" => MOD_SHIFT,
                "win" => MOD_WIN,
                _ => return None,
            };
        }
        Some(Hotkey { mods, vk: key_code(key)? })
    }

    pub fn to_text(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.mods & MOD_CONTROL != 0 {
            parts.push("Ctrl".into());
        }
        if self.mods & MOD_ALT != 0 {
            parts.push("Alt".into());
        }
        if self.mods & MOD_SHIFT != 0 {
            parts.push("Shift".into());
        }
        if self.mods & MOD_WIN != 0 {
            parts.push("Win".into());
        }
        parts.push(key_name(self.vk));
        parts.join("+")
    }
}

pub fn key_code(name: &str) -> Option<u32> {
    let up = name.to_ascii_uppercase();
    if let Some(n) = up.strip_prefix('F').and_then(|r| r.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    if let Some(n) = up.strip_prefix("NUM").and_then(|r| r.parse::<u32>().ok()) {
        if n <= 9 {
            return Some(0x60 + n);
        }
    }
    if up.len() == 1 {
        let c = up.as_bytes()[0];
        if c.is_ascii_alphanumeric() {
            return Some(c as u32);
        }
    }
    NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| *v)
}

pub fn key_name(vk: u32) -> String {
    match vk {
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).unwrap().to_string(),
        _ => NAMED.iter().find(|(_, v)| *v == vk).map(|(n, _)| n.to_string()).unwrap_or(format!("Key{vk:02X}")),
    }
}

/// Find hotkeys that are assigned to more than one action.
pub fn conflicts<'a>(bindings: &[(&'a str, &str)]) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::new();
    for (i, (a, ka)) in bindings.iter().enumerate() {
        for (b, kb) in &bindings[i + 1..] {
            if let (Some(x), Some(y)) = (Hotkey::parse(ka), Hotkey::parse(kb)) {
                if x == y {
                    out.push((*a, *b));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for t in ["Alt+F10", "Ctrl+Shift+S", "F9", "Alt+Num5", "Ctrl+Alt+PrtSc", "Alt+`", "Win+G"] {
            let h = Hotkey::parse(t).unwrap_or_else(|| panic!("{t}"));
            assert_eq!(h.to_text(), t);
        }
        assert_eq!(Hotkey::parse("Alt+F10"), Some(Hotkey { mods: MOD_ALT, vk: 0x79 }));
        assert_eq!(Hotkey::parse("Hyper+F1"), None);
        assert_eq!(Hotkey::parse("Alt+"), None);
        assert_eq!(Hotkey::parse("shift+alt+r").unwrap().to_text(), "Alt+Shift+R");
    }

    #[test]
    fn finds_conflicts() {
        let c = conflicts(&[("record", "Alt+F9"), ("replay", "Alt+F10"), ("mute", "alt+f9")]);
        assert_eq!(c, vec![("record", "mute")]);
    }
}
