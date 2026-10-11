//! Visual system: palette, fonts, egui style and the few custom widgets the app needs.
//!
//! The interface stays neutral graphite; colour appears only as state — the "tally light":
//! red while recording, amber while Instant Replay is buffering.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Layout, Margin, Response,
    RichText, Sense, Stroke, TextStyle, Ui, Vec2,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub raised: Color32,
    pub hairline: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub rec: Color32,
    pub replay: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub dark: bool,
}

pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x20, 0x21, 0x24),
    panel: Color32::from_rgb(0x2A, 0x2B, 0x2E),
    raised: Color32::from_rgb(0x33, 0x34, 0x38),
    hairline: Color32::from_rgb(0x3A, 0x3B, 0x3F),
    text: Color32::from_rgb(0xEC, 0xEA, 0xE6),
    muted: Color32::from_rgb(0x9C, 0x9A, 0x95),
    faint: Color32::from_rgb(0x6A, 0x69, 0x66),
    rec: Color32::from_rgb(0xE5, 0x48, 0x4D),
    replay: Color32::from_rgb(0xD9, 0xA4, 0x41),
    ok: Color32::from_rgb(0x6F, 0xB5, 0x8A),
    warn: Color32::from_rgb(0xD9, 0xA4, 0x41),
    dark: true,
};

pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xF5, 0xF6, 0xF7),
    panel: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    raised: Color32::from_rgb(0xEB, 0xEC, 0xEE),
    hairline: Color32::from_rgb(0xDF, 0xE1, 0xE4),
    text: Color32::from_rgb(0x1C, 0x1D, 0x1F),
    muted: Color32::from_rgb(0x66, 0x69, 0x6E),
    faint: Color32::from_rgb(0xA3, 0xA6, 0xAB),
    rec: Color32::from_rgb(0xD7, 0x34, 0x3A),
    replay: Color32::from_rgb(0xB7, 0x80, 0x1F),
    ok: Color32::from_rgb(0x2E, 0x8B, 0x57),
    warn: Color32::from_rgb(0xB7, 0x80, 0x1F),
    dark: false,
};

/// Load Segoe UI Variable (Windows 11) or Segoe UI (Windows 10) from the system; keep egui's
/// bundled fonts as fallback for symbols and other scripts.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir =
        std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
    let candidates = [("segoe-var", "SegUIVar.ttf"), ("segoe", "segoeui.ttf")];
    let mut loaded = false;
    for (name, file) in candidates {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
            fonts.families.entry(FontFamily::Proportional).or_default().insert(0, name.to_owned());
            loaded = true;
            break;
        }
    }
    if let Ok(bytes) = std::fs::read(dir.join("seguisb.ttf")) {
        fonts.font_data.insert("segoe-semibold".to_owned(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .insert(FontFamily::Name("semibold".into()), vec!["segoe-semibold".to_owned(), "segoe-var".to_owned()]);
    } else {
        let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
        fonts.families.insert(FontFamily::Name("semibold".into()), fallback);
    }
    if !loaded {
        // Non-Windows dev builds: egui defaults are fine.
    }
    ctx.set_fonts(fonts);
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn apply(ctx: &egui::Context, p: &Palette, scale: f32) {
    ctx.set_pixels_per_point(ctx.native_pixels_per_point().unwrap_or(1.0) * scale);
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Heading, semibold(20.0)),
        (TextStyle::Monospace, FontId::proportional(14.0)),
    ]
    .into();
    style.spacing.item_spacing = Vec2::new(10.0, 8.0);
    style.spacing.button_padding = Vec2::new(14.0, 7.0);
    style.spacing.interact_size = Vec2::new(40.0, 32.0);
    style.spacing.combo_width = 220.0;
    style.spacing.slider_width = 160.0;
    style.spacing.window_margin = Margin::same(16);
    style.spacing.menu_margin = Margin::same(6);
    style.interaction.selectable_labels = false;

    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.panel;
    v.extreme_bg_color = p.panel;
    v.faint_bg_color = p.panel;
    v.code_bg_color = p.raised;
    v.window_stroke = Stroke::new(1.0, p.hairline);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 90 } else { 30 }),
    };
    v.popup_shadow = v.window_shadow;
    v.selection.bg_fill =
        if p.dark { Color32::from_rgb(0x4A, 0x4C, 0x52) } else { Color32::from_rgb(0xD5, 0xD8, 0xDD) };
    v.selection.stroke = Stroke::new(1.0, p.text);
    v.hyperlink_color = p.text;
    v.override_text_color = Some(p.text);
    let r = CornerRadius::same(6);
    for (w, fill, stroke) in [
        (&mut v.widgets.noninteractive, p.bg, p.hairline),
        (&mut v.widgets.inactive, p.raised, p.hairline),
        (
            &mut v.widgets.hovered,
            if p.dark { Color32::from_rgb(0x3D, 0x3E, 0x43) } else { Color32::from_rgb(0xE2, 0xE4, 0xE7) },
            p.faint,
        ),
        (
            &mut v.widgets.active,
            if p.dark { Color32::from_rgb(0x45, 0x46, 0x4B) } else { Color32::from_rgb(0xD8, 0xDA, 0xDE) },
            p.muted,
        ),
        (&mut v.widgets.open, p.raised, p.faint),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, stroke);
        w.corner_radius = r;
        w.fg_stroke = Stroke::new(1.0, p.text);
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.hairline);
    style.visuals = v;
    style.animation_time = 0.12;
    // Same style for egui's dark and light slots: the app picks the palette itself.
    ctx.all_styles_mut(|s| *s = style.clone());
}

// ------------------------------------------------------------------ widgets

/// Toggle switch. Returns the response; flips `on` when clicked.
pub fn switch(ui: &mut Ui, p: &Palette, on: &mut bool) -> Response {
    let size = Vec2::new(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let enabled = ui.is_enabled();
    let track = if *on { lerp(p.raised, p.text, t) } else { p.raised };
    let painter = ui.painter();
    let alpha = |c: Color32| if enabled { c } else { c.gamma_multiply(0.4) };
    painter.rect(
        rect,
        CornerRadius::same(11),
        alpha(track),
        Stroke::new(1.0, alpha(p.hairline)),
        egui::StrokeKind::Inside,
    );
    let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
    let knob = if *on { p.bg } else { p.muted };
    painter.circle_filled(egui::pos2(x, rect.center().y), 7.0, alpha(knob));
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// A settings row: label (+ optional help text) on the left, control on the right.
pub fn row(ui: &mut Ui, p: &Palette, label: &str, help: &str, control: impl FnOnce(&mut Ui)) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let avail = ui.available_width();
        let left = (avail * 0.52).clamp(200.0, 380.0);
        ui.allocate_ui_with_layout(Vec2::new(left, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(left);
            ui.label(RichText::new(label).color(p.text));
            if !help.is_empty() {
                ui.label(RichText::new(help).small().color(p.muted));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
    ui.add_space(6.0);
    hairline(ui, p);
}

pub fn hairline(ui: &mut Ui, p: &Palette) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, p.hairline));
}

/// Section title for a settings page: plain sentence case, no eyebrow.
pub fn page_title(ui: &mut Ui, p: &Palette, title: &str, intro: &str) {
    ui.label(RichText::new(title).font(semibold(22.0)).color(p.text));
    if !intro.is_empty() {
        ui.add_space(2.0);
        ui.label(RichText::new(intro).color(p.muted));
    }
    ui.add_space(14.0);
    hairline(ui, p);
}

pub fn group_title(ui: &mut Ui, p: &Palette, title: &str) {
    ui.add_space(18.0);
    ui.label(RichText::new(title).font(semibold(15.0)).color(p.text));
    ui.add_space(4.0);
    hairline(ui, p);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    Primary,
    Quiet,
    Danger,
}

/// Hand-drawn button so hover, pressed and disabled states always show, whatever the fill.
fn button(ui: &mut Ui, p: &Palette, text: &str, kind: ButtonKind) -> Response {
    let enabled = ui.is_enabled();
    let font = if kind == ButtonKind::Quiet { FontId::proportional(14.0) } else { semibold(14.0) };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    let size = Vec2::new(galley.size().x + 30.0, if kind == ButtonKind::Quiet { 32.0 } else { 34.0 });
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hovered = enabled && resp.hovered();
    let pressed = enabled && resp.is_pointer_button_down_on();
    let (fill, stroke, text_color) = match (kind, enabled) {
        (ButtonKind::Primary, true) => {
            let t = if pressed {
                0.24
            } else if hovered {
                0.12
            } else {
                0.0
            };
            (lerp(p.text, p.bg, t), Stroke::NONE, p.bg)
        }
        (ButtonKind::Primary | ButtonKind::Danger, false) => (p.raised, Stroke::NONE, p.faint),
        (ButtonKind::Danger, true) => {
            let t = if pressed {
                0.24
            } else if hovered {
                0.12
            } else {
                0.0
            };
            (lerp(p.rec, Color32::BLACK, t), Stroke::NONE, Color32::WHITE)
        }
        (ButtonKind::Quiet, true) => {
            let fill = if pressed {
                lerp(p.raised, p.hairline, 0.6)
            } else if hovered {
                p.raised
            } else {
                Color32::TRANSPARENT
            };
            (fill, Stroke::new(1.0, if hovered { p.faint } else { p.hairline }), p.text)
        }
        (ButtonKind::Quiet, false) => (Color32::TRANSPARENT, Stroke::new(1.0, p.hairline), p.faint),
    };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(6), fill, stroke, egui::StrokeKind::Inside);
    painter.galley(rect.center() - galley.size() / 2.0, galley, text_color);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Solid button in the text colour (inverted), for the single main action on a screen.
pub fn primary_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    button(ui, p, text, ButtonKind::Primary)
}

/// Solid red button for destructive actions (always behind a confirmation).
pub fn danger_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    button(ui, p, text, ButtonKind::Danger)
}

pub fn quiet_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    button(ui, p, text, ButtonKind::Quiet)
}

pub fn link_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    let r = ui.add(egui::Label::new(RichText::new(text).color(p.muted).underline()).sense(Sense::click()));
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    r
}

/// Horizontal level meter (0..1 linear amplitude shown on a dB-like curve).
pub fn meter(ui: &mut Ui, p: &Palette, level: f32, width: f32, active: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 6.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), p.raised);
    if active && level > 0.0005 {
        let db = 20.0 * level.max(1e-5).log10();
        let t = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        let mut fill = rect;
        fill.set_width(rect.width() * t);
        let color = if db > -3.0 { p.rec } else { p.muted };
        painter.rect_filled(fill, CornerRadius::same(3), color);
    }
}

/// Label that wraps, in the muted colour.
pub fn note(ui: &mut Ui, p: &Palette, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(p.muted)).wrap());
}
