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
    ("CapsLock", 0x14),
    // Side mouse buttons: push-to-talk only (Windows can't register them as global hotkeys).
    ("Mouse4", 0x05),
    ("Mouse5", 0x06),
];

/// Mouse buttons can be watched (push-to-talk) but not registered as hotkeys.
pub fn is_mouse_button(vk: u32) -> bool {
    matches!(vk, 0x05 | 0x06)
}

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

/// Is exactly this combination held? `down(vk)` says whether a virtual key is pressed.
/// Modifiers that aren't part of the hotkey must be up, so Ctrl+F9 doesn't fire Alt+F9.
pub fn combo_down(key: Hotkey, down: impl Fn(u32) -> bool) -> bool {
    const VK_SHIFT: u32 = 0x10;
    const VK_CONTROL: u32 = 0x11;
    const VK_MENU: u32 = 0x12;
    const VK_LWIN: u32 = 0x5B;
    const VK_RWIN: u32 = 0x5C;
    let want = |flag: u32| key.mods & flag != 0;
    down(key.vk)
        && want(MOD_ALT) == down(VK_MENU)
        && want(MOD_CONTROL) == down(VK_CONTROL)
        && want(MOD_SHIFT) == down(VK_SHIFT)
        && want(MOD_WIN) == (down(VK_LWIN) || down(VK_RWIN))
}

/// Pairs the key watcher's sightings with hotkeys Windows delivered, so each physical press
/// counts once: a press the watcher sees is reported only if Windows didn't deliver a hotkey
/// for it within `WAIT_MS`. Presses Windows did deliver are never delayed or dropped, however
/// fast they come.
#[derive(Clone, Copy, Debug, Default)]
pub struct PressMatcher {
    pending: Option<u64>,
}

impl PressMatcher {
    pub const WAIT_MS: u64 = 150;

    /// Call on every poll. `now` and `last_delivered` are milliseconds on the same clock
    /// (`last_delivered` = 0: never). `edge` is true when the watcher just saw the combination
    /// go down. Returns true when the watcher should report a press.
    pub fn poll(&mut self, now: u64, edge: bool, last_delivered: u64) -> bool {
        if edge && self.pending.is_none() {
            self.pending = Some(now);
        }
        let Some(p) = self.pending else { return false };
        if last_delivered != 0 && last_delivered + Self::WAIT_MS >= p {
            self.pending = None;
            return false;
        }
        if now >= p + Self::WAIT_MS {
            self.pending = None;
            return true;
        }
        false
    }
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
        // Push-to-talk keys: a plain key or a side mouse button.
        assert_eq!(Hotkey::parse("V"), Some(Hotkey { mods: 0, vk: 0x56 }));
        assert_eq!(Hotkey::parse("Mouse4").unwrap().to_text(), "Mouse4");
        assert!(is_mouse_button(Hotkey::parse("Mouse5").unwrap().vk));
        assert!(!is_mouse_button(Hotkey::parse("CapsLock").unwrap().vk));
    }

    #[test]
    fn finds_conflicts() {
        let c = conflicts(&[("record", "Alt+F9"), ("replay", "Alt+F10"), ("mute", "alt+f9")]);
        assert_eq!(c, vec![("record", "mute")]);
    }
}

#[cfg(test)]
mod combo_tests {
    use super::*;

    #[test]
    fn needs_exactly_the_modifiers() {
        let alt_f9 = Hotkey { mods: MOD_ALT, vk: 0x78 };
        assert!(combo_down(alt_f9, |vk| vk == 0x78 || vk == 0x12));
        assert!(!combo_down(alt_f9, |vk| vk == 0x78));
        assert!(!combo_down(alt_f9, |vk| vk == 0x78 || vk == 0x12 || vk == 0x11));
        assert!(!combo_down(alt_f9, |vk| vk == 0x12));
        let plain = Hotkey { mods: 0, vk: 0x78 };
        assert!(combo_down(plain, |vk| vk == 0x78));
        assert!(!combo_down(plain, |vk| vk == 0x78 || vk == 0x10));
    }
}

#[cfg(test)]
mod matcher_tests {
    use super::PressMatcher;

    #[test]
    fn a_delivered_press_is_not_reported_again() {
        let mut m = PressMatcher::default();
        // Windows delivered at 1000; the watcher sees the key at 1010.
        assert!(!m.poll(1010, true, 1000));
        assert!(!m.poll(1200, false, 1000));
        // The watcher saw it first; Windows delivers 30 ms later.
        let mut m = PressMatcher::default();
        assert!(!m.poll(2000, true, 1000));
        assert!(!m.poll(2030, false, 2030));
        assert!(!m.poll(2200, false, 2030));
    }

    #[test]
    fn a_blocked_press_is_reported_once_after_the_wait() {
        let mut m = PressMatcher::default();
        assert!(!m.poll(5000, true, 0));
        assert!(!m.poll(5100, false, 0));
        assert!(m.poll(5150, false, 0));
        assert!(!m.poll(5200, false, 0));
        // Next press, still blocked (an old delivery long ago doesn't count).
        assert!(!m.poll(9000, true, 1000));
        assert!(m.poll(9160, false, 1000));
    }
}
