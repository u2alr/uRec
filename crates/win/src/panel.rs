//! The overlay panel: a compact card opened with a hotkey while playing, with Save replay,
//! Start/Stop recording, microphone mute and the replay length.
//!
//! Platform-neutral so it can be unit-tested: `PanelModel` is what the controller reports,
//! `layout` places everything for one display scale, `hit` finds the button under a point,
//! and `render` draws premultiplied BGRA pixels. The Windows side (`panel_win.rs`) only owns
//! the window, input and focus.

use crate::pill::TextMask;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum ReplayView {
    Off,
    Starting,
    On,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RecordView {
    Idle,
    Starting,
    Recording { seconds: u64 },
    Finishing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Success,
    Error,
}

/// Everything the panel shows. Built by the controller from its real state; nothing here is
/// guessed by the panel.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelModel {
    pub replay: ReplayView,
    pub replay_seconds: u32,
    /// A replay save is running.
    pub saving: bool,
    pub recording: RecordView,
    /// None when the microphone isn't part of the capture.
    pub mic_muted: Option<bool>,
    /// The latest message (shown for a few seconds).
    pub message: Option<(String, Tone)>,
    pub key_save: String,
    pub key_record: String,
    pub key_mute: String,
    pub key_panel: String,
}

impl Default for PanelModel {
    fn default() -> Self {
        PanelModel {
            replay: ReplayView::Off,
            replay_seconds: 60,
            saving: false,
            recording: RecordView::Idle,
            mic_muted: None,
            message: None,
            key_save: String::new(),
            key_record: String::new(),
            key_mute: String::new(),
            key_panel: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    SaveReplay,
    ToggleReplay,
    ToggleRecording,
    ToggleMic,
    ReplaySeconds(u32),
    OpenSettings,
    Close,
}

/// Replay lengths offered in the panel (the full list is in Settings).
pub const REPLAY_CHOICES: [u32; 4] = [30, 60, 120, 300];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Font {
    Title,
    Body,
    Small,
}

impl Font {
    pub fn px(self, k: f32) -> i32 {
        let base = match self {
            Font::Title => 13.5,
            Font::Body => 12.5,
            Font::Small => 11.5,
        };
        (base * k).round() as i32
    }
    pub fn semibold(self) -> bool {
        !matches!(self, Font::Small)
    }
}

/// Renders one line of text: (text, pixel height, semibold).
type MakeMask = Box<dyn FnMut(&str, i32, bool) -> TextMask>;

/// Text masks, made by the platform and cached by text, font and size.
pub struct Texts {
    cache: HashMap<(String, Font, i32), TextMask>,
    make: MakeMask,
}

impl Texts {
    pub fn new(make: impl FnMut(&str, i32, bool) -> TextMask + 'static) -> Texts {
        Texts { cache: HashMap::new(), make: Box::new(make) }
    }

    pub fn get(&mut self, text: &str, font: Font, k: f32) -> &TextMask {
        let px = font.px(k);
        let key = (text.to_string(), font, px);
        if !self.cache.contains_key(&key) {
            // Keep the cache small: the elapsed-time text changes every second.
            if self.cache.len() > 400 {
                self.cache.clear();
            }
            let m = (self.make)(text, px, font.semibold());
            self.cache.insert(key.clone(), m);
        }
        &self.cache[&key]
    }

    fn width(&mut self, text: &str, font: Font, k: f32) -> f32 {
        self.get(text, font, k).w as f32
    }

    /// `text`, shortened with "…" to fit `max` pixels.
    fn fit(&mut self, text: &str, font: Font, k: f32, max: f32) -> String {
        if self.width(text, font, k) <= max {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut n = chars.len();
        while n > 1 {
            n -= 1;
            let s: String = chars[..n].iter().collect::<String>().trim_end().to_string() + "…";
            if self.width(&s, font, k) <= max {
                return s;
            }
        }
        "…".into()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Primary,
    Secondary,
    Chip { selected: bool },
    Quiet,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    pub rect: RectF,
    pub action: PanelAction,
    pub label: String,
    /// Hotkey shown next to the label (may be empty).
    pub hint: String,
    pub style: Style,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Op {
    Text { x: f32, y: f32, text: String, font: Font, color: (u8, u8, u8) },
    Dot { cx: f32, cy: f32, r: f32, color: (u8, u8, u8) },
    Divider { y: f32 },
}

/// A button before placement: action, label, hotkey hint, style, enabled.
type RowButton = (PanelAction, String, String, Style, bool);

/// A placed panel for one scale. `w`/`h` include the shadow margin around the card.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub w: usize,
    pub h: usize,
    pub k: f32,
    pub card: RectF,
    pub buttons: Vec<Button>,
    ops: Vec<Op>,
}

const TEXT: (u8, u8, u8) = (236, 236, 239);
const MUTED: (u8, u8, u8) = (161, 161, 170);
const DARK: (u8, u8, u8) = (24, 24, 27);
const RED: (u8, u8, u8) = (229, 72, 77);
const AMBER: (u8, u8, u8) = (217, 164, 65);
const GREEN: (u8, u8, u8) = (74, 222, 128);
const GREY: (u8, u8, u8) = (113, 113, 122);

pub fn format_secs(s: u32) -> String {
    if s < 60 {
        format!("{s}s")
    } else if s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{}m {}s", s / 60, s % 60)
    }
}

fn format_long(s: u32) -> String {
    match s {
        s if s < 60 => format!("{s} seconds"),
        60 => "1 minute".into(),
        s if s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{}:{:02} minutes", s / 60, s % 60),
    }
}

pub fn format_clock(s: u64) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Shadow margin around the card.
fn shadow(k: f32) -> f32 {
    (14.0 * k).round()
}

/// Lay the panel out for display scale `k` (1.0 at 1080p).
pub fn layout(m: &PanelModel, k: f32, texts: &mut Texts) -> Layout {
    let s = shadow(k);
    let pad = (16.0 * k).round();
    let card_w = (380.0 * k).round();
    let row_h = (56.0 * k).round();
    let header_h = (40.0 * k).round();
    let btn_h = (28.0 * k).round();
    let gap = (8.0 * k).round();
    let mut ops = Vec::new();
    let mut buttons: Vec<Button> = Vec::new();
    let left = s + pad;
    let right = s + card_w - pad;
    let mut y = s;

    // Header.
    let cy = y + header_h / 2.0;
    text_left(&mut ops, texts, "uRec", Font::Title, TEXT, left, cy, k);
    let close_hint =
        if m.key_panel.is_empty() { "Esc to close".to_string() } else { format!("Esc or {} to close", m.key_panel) };
    let hint_w = texts.width(&close_hint, Font::Small, k);
    text_left(&mut ops, texts, &close_hint, Font::Small, MUTED, right - hint_w, cy, k);
    y += header_h;
    ops.push(Op::Divider { y });

    // Rows: dot, title, subtitle and right-aligned buttons.
    let row = |ops: &mut Vec<Op>,
               buttons: &mut Vec<Button>,
               texts: &mut Texts,
               y: f32,
               dot: Option<(u8, u8, u8)>,
               title: &str,
               subtitle: &str,
               btns: Vec<RowButton>| {
        // Buttons first (right to left) so the texts know how much room is left.
        let mut x = right;
        let by = y + (row_h - btn_h) / 2.0;
        let mut placed = Vec::new();
        for (action, label, hint, style, enabled) in btns.into_iter().rev() {
            let w = button_width(texts, &label, &hint, style, k);
            x -= w;
            placed.push(Button { rect: RectF { x, y: by, w, h: btn_h }, action, label, hint, style, enabled });
            x -= (6.0 * k).round();
        }
        placed.reverse();
        let text_x = if dot.is_some() { left + (18.0 * k).round() } else { left };
        if let Some(c) = dot {
            ops.push(Op::Dot { cx: left + 5.0 * k, cy: y + row_h / 2.0 - 7.0 * k, r: 4.0 * k, color: c });
        }
        let room = x - text_x - gap;
        let t = texts.fit(title, Font::Body, k, room);
        text_left(ops, texts, &t, Font::Body, TEXT, text_x, y + row_h / 2.0 - 8.0 * k, k);
        let st = texts.fit(subtitle, Font::Small, k, room);
        text_left(ops, texts, &st, Font::Small, MUTED, text_x, y + row_h / 2.0 + 9.0 * k, k);
        buttons.extend(placed);
    };

    // Instant Replay.
    let (dot, sub) = match m.replay {
        ReplayView::On if m.saving => (AMBER, "Saving…".to_string()),
        ReplayView::On => (AMBER, format!("On · keeps the last {}", format_long(m.replay_seconds))),
        ReplayView::Starting => (AMBER, "Starting…".into()),
        ReplayView::Off => (GREY, "Off".into()),
        ReplayView::Error => (RED, "Couldn't start. See uRec for details.".into()),
    };
    let on = matches!(m.replay, ReplayView::On);
    row(
        &mut ops,
        &mut buttons,
        texts,
        y,
        Some(dot),
        "Instant Replay",
        &sub,
        vec![
            (
                PanelAction::ToggleReplay,
                if matches!(m.replay, ReplayView::Off | ReplayView::Error) { "Turn on" } else { "Turn off" }.into(),
                String::new(),
                Style::Secondary,
                !matches!(m.replay, ReplayView::Starting),
            ),
            (PanelAction::SaveReplay, "Save replay".into(), m.key_save.clone(), Style::Primary, on),
        ],
    );
    y += row_h;
    ops.push(Op::Divider { y });

    // Recording.
    let (dot, sub, label, enabled) = match &m.recording {
        RecordView::Idle => (GREY, "Not recording".to_string(), "Start", true),
        RecordView::Starting => (RED, "Starting…".into(), "Start", false),
        RecordView::Recording { seconds } => (RED, format!("Recording · {}", format_clock(*seconds)), "Stop", true),
        RecordView::Finishing => (GREY, "Finishing…".into(), "Stop", false),
    };
    row(
        &mut ops,
        &mut buttons,
        texts,
        y,
        Some(dot),
        "Recording",
        &sub,
        vec![(PanelAction::ToggleRecording, label.into(), m.key_record.clone(), Style::Secondary, enabled)],
    );
    y += row_h;
    ops.push(Op::Divider { y });

    // Microphone, only when it is being recorded.
    if let Some(muted) = m.mic_muted {
        row(
            &mut ops,
            &mut buttons,
            texts,
            y,
            Some(if muted { GREY } else { GREEN }),
            "Microphone",
            if muted { "Muted" } else { "Live" },
            vec![(
                PanelAction::ToggleMic,
                if muted { "Unmute" } else { "Mute" }.into(),
                m.key_mute.clone(),
                Style::Secondary,
                true,
            )],
        );
        y += row_h;
        ops.push(Op::Divider { y });
    }

    // Replay length.
    let chips = REPLAY_CHOICES
        .iter()
        .map(|&c| {
            (
                PanelAction::ReplaySeconds(c),
                format_secs(c),
                String::new(),
                Style::Chip { selected: c == m.replay_seconds },
                !matches!(m.replay, ReplayView::Starting),
            )
        })
        .collect();
    let sub = if on { "Changing it restarts the buffer" } else { "Used when Instant Replay is on" };
    row(&mut ops, &mut buttons, texts, y, None, "Replay length", sub, chips);
    y += row_h;
    ops.push(Op::Divider { y });

    // Footer: the latest message, and a way to the full settings.
    let foot_h = (44.0 * k).round();
    let settings_btn_w = button_width(texts, "Settings", "", Style::Quiet, k);
    let bx = right - settings_btn_w;
    let fy = y + (foot_h - btn_h) / 2.0;
    if let Some((msg, tone)) = &m.message {
        let color = match tone {
            Tone::Info => MUTED,
            Tone::Success => GREEN,
            Tone::Error => (248, 113, 113),
        };
        let t = texts.fit(msg, Font::Small, k, bx - left - gap);
        text_left(&mut ops, texts, &t, Font::Small, color, left, y + foot_h / 2.0, k);
    }
    buttons.push(Button {
        rect: RectF { x: bx, y: fy, w: settings_btn_w, h: btn_h },
        action: PanelAction::OpenSettings,
        label: "Settings".into(),
        hint: String::new(),
        style: Style::Quiet,
        enabled: true,
    });
    y += foot_h;

    let card = RectF { x: s, y: s, w: card_w, h: y - s };
    Layout { w: (card_w + 2.0 * s) as usize, h: (y + s) as usize, k, card, buttons, ops }
}

fn button_width(texts: &mut Texts, label: &str, hint: &str, style: Style, k: f32) -> f32 {
    let pad = match style {
        Style::Chip { .. } => 10.0,
        _ => 12.0,
    } * k;
    let mut w = texts.width(label, Font::Body, k) + 2.0 * pad;
    if !hint.is_empty() {
        w += (8.0 * k).round() + texts.width(hint, Font::Small, k);
    }
    w.round()
}

/// Text with its vertical centre at `cy`.
#[allow(clippy::too_many_arguments)]
fn text_left(
    ops: &mut Vec<Op>,
    texts: &mut Texts,
    text: &str,
    font: Font,
    color: (u8, u8, u8),
    x: f32,
    cy: f32,
    k: f32,
) {
    let h = texts.get(text, font, k).h as f32;
    ops.push(Op::Text { x: x.round(), y: (cy - h / 2.0).round(), text: text.to_string(), font, color });
}

/// The button under (x, y), enabled or not.
pub fn hit(l: &Layout, x: f32, y: f32) -> Option<usize> {
    l.buttons.iter().position(|b| b.rect.contains(x, y))
}

/// Next enabled button for keyboard focus, wrapping around. `dir` is +1 or -1.
pub fn next_focus(l: &Layout, from: Option<usize>, dir: i32) -> Option<usize> {
    let n = l.buttons.len() as i32;
    if n == 0 {
        return None;
    }
    let mut i = match from {
        Some(f) => f as i32,
        None if dir > 0 => -1,
        None => n,
    };
    for _ in 0..n {
        i = (i + dir).rem_euclid(n);
        if l.buttons[i as usize].enabled {
            return Some(i as usize);
        }
    }
    None
}

/// Signed distance to a rounded rectangle (negative inside).
fn sd_round_rect(px: f32, py: f32, r: &RectF, rad: f32) -> f32 {
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let (hx, hy) = (r.w / 2.0 - rad, r.h / 2.0 - rad);
    let qx = (px - cx).abs() - hx;
    let qy = (py - cy).abs() - hy;
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - rad
}

/// Paint straight RGB at alpha `a` over the premultiplied pixel at `i`.
fn over(px: &mut [u8], i: usize, rgb: (u8, u8, u8), a: f32) {
    if a <= 0.0 {
        return;
    }
    let a = a.min(1.0);
    let inv = 1.0 - a;
    px[i] = (rgb.2 as f32 * a + px[i] as f32 * inv).round() as u8;
    px[i + 1] = (rgb.1 as f32 * a + px[i + 1] as f32 * inv).round() as u8;
    px[i + 2] = (rgb.0 as f32 * a + px[i + 2] as f32 * inv).round() as u8;
    px[i + 3] = (255.0 * a + px[i + 3] as f32 * inv).round().min(255.0) as u8;
}

fn fill_round_rect(px: &mut [u8], w: usize, h: usize, r: &RectF, rad: f32, rgb: (u8, u8, u8), a: f32) {
    let x0 = (r.x.floor().max(0.0)) as usize;
    let y0 = (r.y.floor().max(0.0)) as usize;
    let x1 = ((r.x + r.w).ceil() as usize).min(w);
    let y1 = ((r.y + r.h).ceil() as usize).min(h);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = sd_round_rect(x as f32 + 0.5, y as f32 + 0.5, r, rad);
            let cov = (0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                over(px, (y * w + x) * 4, rgb, cov * a);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stroke_round_rect(px: &mut [u8], w: usize, h: usize, r: &RectF, rad: f32, width: f32, rgb: (u8, u8, u8), a: f32) {
    let x0 = ((r.x - width).floor().max(0.0)) as usize;
    let y0 = ((r.y - width).floor().max(0.0)) as usize;
    let x1 = ((r.x + r.w + width).ceil() as usize).min(w);
    let y1 = ((r.y + r.h + width).ceil() as usize).min(h);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = sd_round_rect(x as f32 + 0.5, y as f32 + 0.5, r, rad);
            // A band of `width` just inside the edge.
            let cov = ((0.5 - d).clamp(0.0, 1.0) - (0.5 - (d + width)).clamp(0.0, 1.0)).max(0.0);
            if cov > 0.0 {
                over(px, (y * w + x) * 4, rgb, cov * a);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_text(px: &mut [u8], w: usize, h: usize, mask: &TextMask, x: f32, y: f32, rgb: (u8, u8, u8), a: f32) {
    let (x0, y0) = (x as i64, y as i64);
    for my in 0..mask.h {
        let yy = y0 + my as i64;
        if yy < 0 || yy >= h as i64 {
            continue;
        }
        for mx in 0..mask.w {
            let xx = x0 + mx as i64;
            if xx < 0 || xx >= w as i64 {
                continue;
            }
            let c = mask.cov[my * mask.w + mx];
            if c == 0 {
                continue;
            }
            // A slight gamma lifts GDI's thin greyscale edges so light text reads crisply.
            let cov = (c as f32 / 255.0).powf(0.8);
            over(px, (yy as usize * w + xx as usize) * 4, rgb, cov * a);
        }
    }
}

/// Interaction state that changes how buttons look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Interaction {
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    /// Keyboard focus; drawn only once the keyboard has been used.
    pub focus: Option<usize>,
    pub show_focus: bool,
}

/// Draw the panel: premultiplied BGRA, top-down, `l.w` × `l.h`.
pub fn render(l: &Layout, ui: Interaction, texts: &mut Texts) -> Vec<u8> {
    let (w, h, k) = (l.w, l.h, l.k);
    let mut px = vec![0u8; w * h * 4];
    let rad = (12.0 * k).round();
    let s = shadow(k);
    // Soft shadow: keeps the card readable on bright scenes.
    for y in 0..h {
        for x in 0..w {
            let d = sd_round_rect(x as f32 + 0.5, y as f32 + 0.5, &l.card, rad);
            if d > 0.0 && d < s {
                let t = 1.0 - d / s;
                over(&mut px, (y * w + x) * 4, (0, 0, 0), 0.32 * t * t);
            }
        }
    }
    fill_round_rect(&mut px, w, h, &l.card, rad, DARK, 0.94);
    stroke_round_rect(&mut px, w, h, &l.card, rad, k.max(1.0), (255, 255, 255), 0.09);
    for op in &l.ops {
        match op {
            Op::Divider { y } => {
                let r = RectF { x: l.card.x + 1.0, y: *y, w: l.card.w - 2.0, h: k.max(1.0).round() };
                fill_round_rect(&mut px, w, h, &r, 0.0, (255, 255, 255), 0.06);
            }
            Op::Dot { cx, cy, r, color } => {
                let rect = RectF { x: cx - r, y: cy - r, w: r * 2.0, h: r * 2.0 };
                fill_round_rect(&mut px, w, h, &rect, *r, *color, 1.0);
            }
            Op::Text { x, y, text, font, color } => {
                let mask = texts.get(text, *font, k).clone();
                blit_text(&mut px, w, h, &mask, *x, *y, *color, 1.0);
            }
        }
    }
    for (i, b) in l.buttons.iter().enumerate() {
        let hovered = ui.hover == Some(i) && b.enabled;
        let pressed = ui.pressed == Some(i) && hovered;
        let brad = (7.0 * k).round();
        let (fill, fill_a, fg, hint_fg) = match b.style {
            Style::Primary => {
                let a = if pressed {
                    0.78
                } else if hovered {
                    0.88
                } else {
                    1.0
                };
                (TEXT, a, DARK, (82, 82, 91))
            }
            Style::Secondary => (
                (255, 255, 255),
                if pressed {
                    0.2
                } else if hovered {
                    0.14
                } else {
                    0.08
                },
                TEXT,
                MUTED,
            ),
            Style::Chip { selected: true } => (TEXT, 1.0, DARK, DARK),
            Style::Chip { selected: false } => (
                (255, 255, 255),
                if pressed {
                    0.16
                } else if hovered {
                    0.11
                } else {
                    0.05
                },
                TEXT,
                MUTED,
            ),
            Style::Quiet => (
                (255, 255, 255),
                if pressed {
                    0.14
                } else if hovered {
                    0.08
                } else {
                    0.0
                },
                MUTED,
                MUTED,
            ),
        };
        let content_a = if b.enabled { 1.0 } else { 0.38 };
        if fill_a > 0.0 {
            fill_round_rect(&mut px, w, h, &b.rect, brad, fill, fill_a * if b.enabled { 1.0 } else { 0.5 });
        }
        if ui.show_focus && ui.focus == Some(i) {
            let ring =
                RectF { x: b.rect.x - 2.0 * k, y: b.rect.y - 2.0 * k, w: b.rect.w + 4.0 * k, h: b.rect.h + 4.0 * k };
            stroke_round_rect(&mut px, w, h, &ring, brad + 2.0 * k, 1.5 * k, (255, 255, 255), 0.6);
        }
        let pad = match b.style {
            Style::Chip { .. } => 10.0,
            _ => 12.0,
        } * k;
        let label = texts.get(&b.label, Font::Body, k).clone();
        let ly = (b.rect.y + (b.rect.h - label.h as f32) / 2.0).round();
        blit_text(&mut px, w, h, &label, (b.rect.x + pad).round(), ly, fg, content_a);
        if !b.hint.is_empty() {
            let hint = texts.get(&b.hint, Font::Small, k).clone();
            let hx = (b.rect.x + pad + label.w as f32 + (8.0 * k).round()).round();
            let hy = (b.rect.y + (b.rect.h - hint.h as f32) / 2.0).round();
            blit_text(&mut px, w, h, &hint, hx, hy, hint_fg, content_a);
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake text: 7 px per character at scale 1, 14 px high.
    fn texts() -> Texts {
        Texts::new(|t, px, _| {
            let w = t.chars().count() * (px as usize * 7 / 13).max(1);
            TextMask { w, h: px as usize + 2, cov: vec![255; w * (px as usize + 2)] }
        })
    }

    fn model() -> PanelModel {
        PanelModel {
            replay: ReplayView::On,
            replay_seconds: 60,
            key_save: "Alt+F10".into(),
            key_record: "Alt+F9".into(),
            key_mute: "Ctrl+Alt+M".into(),
            key_panel: "Alt+F8".into(),
            ..Default::default()
        }
    }

    fn find(l: &Layout, a: PanelAction) -> &Button {
        l.buttons.iter().find(|b| b.action == a).expect("button")
    }

    #[test]
    fn every_button_is_inside_the_card_and_none_overlap() {
        for k in [1.0, 1.5, 2.0] {
            for mic in [None, Some(false)] {
                let mut m = model();
                m.mic_muted = mic;
                m.message =
                    Some(("A very long message that should be shortened to fit the footer".into(), Tone::Error));
                let mut t = texts();
                let l = layout(&m, k, &mut t);
                for (i, a) in l.buttons.iter().enumerate() {
                    assert!(a.rect.x >= l.card.x && a.rect.x + a.rect.w <= l.card.x + l.card.w, "{a:?} at k={k}");
                    assert!(a.rect.y >= l.card.y && a.rect.y + a.rect.h <= l.card.y + l.card.h);
                    for b in &l.buttons[i + 1..] {
                        let apart = a.rect.x + a.rect.w <= b.rect.x
                            || b.rect.x + b.rect.w <= a.rect.x
                            || a.rect.y + a.rect.h <= b.rect.y
                            || b.rect.y + b.rect.h <= a.rect.y;
                        assert!(apart, "{a:?} overlaps {b:?}");
                    }
                }
                let px = render(&l, Interaction::default(), &mut t);
                assert_eq!(px.len(), l.w * l.h * 4);
            }
        }
    }

    #[test]
    fn buttons_follow_the_real_state() {
        let mut t = texts();
        let mut m = model();
        m.replay = ReplayView::Off;
        let l = layout(&m, 1.0, &mut t);
        assert!(!find(&l, PanelAction::SaveReplay).enabled, "nothing to save while replay is off");
        assert_eq!(find(&l, PanelAction::ToggleReplay).label, "Turn on");
        assert!(!l.buttons.iter().any(|b| b.action == PanelAction::ToggleMic), "no mic row without a mic");

        m.replay = ReplayView::On;
        m.recording = RecordView::Recording { seconds: 75 };
        m.mic_muted = Some(true);
        let l = layout(&m, 1.0, &mut t);
        assert!(find(&l, PanelAction::SaveReplay).enabled);
        assert_eq!(find(&l, PanelAction::ToggleRecording).label, "Stop");
        assert_eq!(find(&l, PanelAction::ToggleMic).label, "Unmute");
        assert!(l.ops.iter().any(|o| matches!(o, Op::Text { text, .. } if text == "Recording · 1:15")));

        m.recording = RecordView::Finishing;
        let l = layout(&m, 1.0, &mut t);
        assert!(!find(&l, PanelAction::ToggleRecording).enabled, "no second press while finishing");
        let chip = find(&l, PanelAction::ReplaySeconds(60));
        assert_eq!(chip.style, Style::Chip { selected: true });
    }

    #[test]
    fn hit_testing_and_keyboard_focus() {
        let mut t = texts();
        let mut m = model();
        m.replay = ReplayView::Off;
        let l = layout(&m, 1.0, &mut t);
        let b = find(&l, PanelAction::ToggleRecording);
        let i = hit(&l, b.rect.x + 2.0, b.rect.y + 2.0).unwrap();
        assert_eq!(l.buttons[i].action, PanelAction::ToggleRecording);
        assert_eq!(hit(&l, 0.0, 0.0), None);
        // Focus skips the disabled Save button.
        let save = l.buttons.iter().position(|b| b.action == PanelAction::SaveReplay).unwrap();
        let mut f = None;
        for _ in 0..l.buttons.len() * 2 {
            f = next_focus(&l, f, 1);
            assert_ne!(f, Some(save));
        }
        assert_eq!(next_focus(&l, None, -1), Some(l.buttons.len() - 1));
    }

    #[test]
    fn long_text_is_shortened() {
        let mut t = texts();
        let s = t.fit("Instant Replay couldn't be saved: the drive is full", Font::Small, 1.0, 100.0);
        assert!(s.ends_with('…'));
        assert!(t.width(&s, Font::Small, 1.0) <= 100.0);
        assert_eq!(format_clock(3725), "1:02:05");
        assert_eq!(format_secs(120), "2m");
    }
}
