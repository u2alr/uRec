//! The clip editor: preview with sound, a timeline of pieces (split, delete, trim, reorder),
//! crop with aspect presets, mute/volume, and export to a new file. Originals are never changed.
//!
//! Keys: Space play/pause · S split at the playhead · Delete remove the selected piece ·
//! ← → one frame (Shift: one second) · Home/End · Ctrl+Z / Ctrl+Y undo/redo ·
//! Ctrl+wheel zoom the timeline, wheel scrolls it.

use super::theme::{self, Palette};
use super::{App, Page};
use crate::controller::{format_secs, Level, Status};
use crate::editor::{self, Export, Frame, Opening, Player, Stills, Strips, PEAKS_PER_SEC, THUMB_H, THUMB_W};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2,
};
use std::collections::HashMap;
use std::path::PathBuf;
use urec_engine::edit::{Crop, EditState, ExportMode, Project};
use urec_engine::encoder::{Backend, Encoder};
use urec_engine::video::{self, Codec, RateControl, VideoSettings};

const PREVIEW_MAX: (u32, u32) = (960, 540);
const RULER_H: f32 = 18.0;
const STRIP_H: f32 = 56.0;
const WAVE_H: f32 = 26.0;
const EDGE_GRAB: f32 = 7.0;

#[derive(Clone, Copy, PartialEq)]
enum Aspect {
    Free,
    Ratio(f32),
}

const ASPECTS: [(&str, Aspect); 5] = [
    ("Free", Aspect::Free),
    ("16:9", Aspect::Ratio(16.0 / 9.0)),
    ("9:16", Aspect::Ratio(9.0 / 16.0)),
    ("1:1", Aspect::Ratio(1.0)),
    ("4:5", Aspect::Ratio(0.8)),
];

enum Drag {
    Playhead,
    Edge { piece: usize, start: bool, before: EditState, origin: f64, grab_x: f32 },
    Move { piece: usize, before_x: f32, target: usize },
    Crop { handle: CropHandle, start: Crop, grab: Pos2 },
}

#[derive(Clone, Copy, PartialEq)]
enum CropHandle {
    Body,
    Corner(bool, bool), // (right, bottom)
}

struct ExportDialog {
    name: String,
    mode: ExportMode,
    codec: Codec,
}

pub struct EditorUi {
    names: Vec<String>,
    opening: Option<Opening>,
    project: Option<Project>,
    player: Option<Player>,
    stills: Option<Stills>,
    strips: Option<Strips>,
    playhead: f64,
    selected: Option<usize>,
    zoom: f32,
    scroll: f32,
    drag: Option<Drag>,
    crop_mode: bool,
    aspect: Aspect,
    crop_edit: Option<Crop>,
    tex: Option<(egui::TextureHandle, u64)>,
    thumbs: HashMap<(usize, usize), egui::TextureHandle>,
    dialog: Option<ExportDialog>,
    export: Option<Export>,
    confirm_close: bool,
    error: Option<String>,
    still_for: Option<(f64, u64)>,
    volume_before: Option<EditState>,
}

impl EditorUi {
    pub fn open(app: &App, paths: Vec<PathBuf>) -> Option<EditorUi> {
        let ffmpeg = urec_engine::process::locate_ffmpeg()?;
        let names =
            paths.iter().map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()).collect();
        let ctx = app.repaint_handle();
        Some(EditorUi {
            names,
            opening: Some(Opening::start(ffmpeg.clone(), paths, ctx.clone())),
            project: None,
            player: None,
            stills: Some(Stills::start(ffmpeg, ctx)),
            strips: None,
            playhead: 0.0,
            selected: None,
            zoom: 0.0,
            scroll: 0.0,
            drag: None,
            crop_mode: false,
            aspect: Aspect::Free,
            crop_edit: None,
            tex: None,
            thumbs: HashMap::new(),
            dialog: None,
            export: None,
            confirm_close: false,
            error: None,
            still_for: None,
            volume_before: None,
        })
    }
}

/// Open the editor on these files.
pub fn open(app: &mut App, paths: Vec<PathBuf>) {
    match EditorUi::open(app, paths) {
        Some(ed) => {
            app.editor = Some(ed);
            app.page = Page::Editor;
        }
        None => app.push_notice(Level::Error, "FFmpeg is missing, so clips can't be edited. Reinstall uRec.".into()),
    }
}

fn clock(t: f64) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor() as u64;
    let s = t - m as f64 * 60.0;
    format!("{m}:{s:04.1}")
}

pub fn show(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let Some(mut ed) = app.editor.take() else {
        app.page = Page::Library;
        return;
    };
    let close = frame(app, &mut ed, ui, st);
    if close {
        app.page = Page::Library;
        app.library.refresh(&app.s.output_dir());
    } else {
        app.editor = Some(ed);
    }
}

/// Returns true when the editor should close.
fn frame(app: &mut App, ed: &mut EditorUi, ui: &mut egui::Ui, st: &Status) -> bool {
    let p = app.p;
    let ffmpeg = st.ffmpeg.clone().or_else(urec_engine::process::locate_ffmpeg);
    // Finish opening.
    if let Some(o) = &ed.opening {
        let done = o.result.lock().unwrap().take();
        match done {
            Some(Ok(sources)) => {
                if let Some(ff) = &ffmpeg {
                    ed.strips = Some(Strips::start(ff.clone(), sources.clone(), app.repaint_handle()));
                }
                ed.project = Some(Project::new(sources));
                ed.opening = None;
                ed.still_for = None;
            }
            Some(Err(e)) => {
                ed.error = Some(e);
                ed.opening = None;
            }
            None => {}
        }
    }

    // Header.
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Edit").font(theme::semibold(22.0)).color(p.text));
        ui.add_space(10.0);
        let title = if ed.names.len() == 1 { ed.names[0].clone() } else { format!("{} clips", ed.names.len()) };
        ui.add(egui::Label::new(RichText::new(title).color(p.muted)).truncate());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let can_export = ed.project.is_some() && ed.export.is_none();
            if ui.add_enabled_ui(can_export, |ui| theme::primary_button(ui, &p, "Export")).inner.clicked() {
                ed.player = None;
                let name = format!("{} edit", ed.names.first().cloned().unwrap_or_else(|| "Clip".into()));
                let lossless = ed.project.as_ref().is_some_and(|pr| pr.lossless_possible().is_ok());
                ed.dialog = Some(ExportDialog {
                    name,
                    mode: if lossless { ExportMode::Lossless } else { ExportMode::Exact },
                    codec: Codec::H264,
                });
            }
            if theme::quiet_button(ui, &p, "Close").clicked() {
                if ed.project.as_ref().is_some_and(|pr| pr.can_undo()) {
                    ed.confirm_close = true;
                } else {
                    close = true;
                }
            }
            if let Some(pr) = ed.project.as_mut() {
                ui.add_space(8.0);
                if ui.add_enabled_ui(pr.can_redo(), |ui| theme::quiet_button(ui, &p, "Redo")).inner.clicked() {
                    pr.redo();
                    ed.still_for = None;
                }
                if ui.add_enabled_ui(pr.can_undo(), |ui| theme::quiet_button(ui, &p, "Undo")).inner.clicked() {
                    pr.undo();
                    ed.still_for = None;
                }
            }
        });
    });
    ui.add_space(12.0);

    if let Some(e) = &ed.error {
        theme::note(ui, &p, e);
        return close;
    }
    if ed.project.is_none() {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(16.0).color(p.muted));
            ui.label(RichText::new("Opening…").color(p.muted));
        });
        return close;
    }

    keyboard(ed, ui);
    let Some(ff) = ffmpeg else { return close };

    // Layout: preview on top, controls, timeline at the bottom.
    let avail = ui.available_rect_before_wrap();
    let timeline_h = RULER_H + STRIP_H + WAVE_H + 22.0;
    let controls_h = 46.0;
    let preview_rect =
        Rect::from_min_max(avail.min, Pos2::new(avail.max.x, avail.max.y - timeline_h - controls_h - 12.0));
    let controls_rect =
        Rect::from_min_size(Pos2::new(avail.min.x, preview_rect.max.y + 6.0), Vec2::new(avail.width(), controls_h));
    let timeline_rect = Rect::from_min_max(Pos2::new(avail.min.x, avail.max.y - timeline_h), avail.max);

    preview(app, ed, ui, preview_rect, &ff);
    controls(app, ed, ui, controls_rect, &ff);
    timeline(app, ed, ui, timeline_rect);
    ui.allocate_rect(avail, Sense::hover());

    export_dialog(app, ed, ui.ctx(), st, &ff);
    if ed.confirm_close {
        let mut keep = false;
        let r = egui::Modal::new(egui::Id::new("editor_close")).show(ui.ctx(), |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new("Close without exporting?").font(theme::semibold(16.0)));
            ui.add_space(4.0);
            theme::note(ui, &p, "Your edits will be lost. The original clips are unchanged either way.");
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if theme::danger_button(ui, &p, "Discard edits").clicked() {
                    close = true;
                }
                if theme::quiet_button(ui, &p, "Keep editing").clicked() {
                    keep = true;
                }
            });
        });
        if keep || r.should_close() {
            ed.confirm_close = false;
        }
    }
    close
}

fn keyboard(ed: &mut EditorUi, ui: &egui::Ui) {
    if ui.ctx().egui_wants_keyboard_input() || ed.dialog.is_some() {
        return;
    }
    let Some(pr) = ed.project.as_mut() else { return };
    let fps = pr.fps();
    let (space, s_key, del, left, right, home, end, undo, redo, shift) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Space),
            i.key_pressed(egui::Key::S) && !i.modifiers.ctrl,
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            i.key_pressed(egui::Key::ArrowLeft),
            i.key_pressed(egui::Key::ArrowRight),
            i.key_pressed(egui::Key::Home),
            i.key_pressed(egui::Key::End),
            i.modifiers.command && i.key_pressed(egui::Key::Z) && !i.modifiers.shift,
            i.modifiers.command && (i.key_pressed(egui::Key::Y) || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
            i.modifiers.shift,
        )
    });
    let dur = pr.duration();
    if space {
        toggle_play(ed);
        return;
    }
    let step = if shift { 1.0 } else { 1.0 / fps };
    let mut moved = None;
    if left {
        moved = Some(ed.playhead - step);
    }
    if right {
        moved = Some(ed.playhead + step);
    }
    if home {
        moved = Some(0.0);
    }
    if end {
        moved = Some(dur - 1.0 / fps);
    }
    if let Some(t) = moved {
        ed.player = None;
        ed.playhead = t.clamp(0.0, (dur - 0.001).max(0.0));
    }
    if s_key && pr.split_at(ed.playhead) {
        ed.selected = pr.locate(ed.playhead).map(|(i, _)| i);
    }
    if del {
        if let Some(i) = ed.selected {
            if pr.delete(i) {
                ed.selected = None;
                ed.playhead = ed.playhead.min(pr.duration() - 0.001).max(0.0);
                ed.still_for = None;
            }
        }
    }
    if undo && pr.undo() {
        ed.still_for = None;
    }
    if redo && pr.redo() {
        ed.still_for = None;
    }
}

/// Play/pause is applied in `preview`, which has what it needs to start the player.
fn toggle_play(ed: &mut EditorUi) {
    ed.drag = None;
    PLAY_REQUEST.with(|r| r.set(true));
}

thread_local! {
    static PLAY_REQUEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn preview(app: &mut App, ed: &mut EditorUi, ui: &mut egui::Ui, area: Rect, ffmpeg: &std::path::Path) {
    let p = app.p;
    let Some(pr) = ed.project.as_ref() else { return };
    let (fw, fh) = pr.frame_size();
    let (pw, ph) = editor::fit(fw, fh, PREVIEW_MAX.0, PREVIEW_MAX.1);

    // Play / pause requests.
    if PLAY_REQUEST.with(|r| r.replace(false)) {
        if ed.player.take().is_none() {
            let from = if ed.playhead >= pr.duration() - 0.05 { 0.0 } else { ed.playhead };
            ed.player = Some(Player::start(ffmpeg, pr, from, pw, ph, app.repaint_handle()));
        }
    }
    // Follow the player; stop at the end.
    let mut shown: Option<Frame> = None;
    if let Some(pl) = &ed.player {
        ed.playhead = pl.position().min(pr.duration());
        shown = pl.frame.lock().unwrap().clone();
        if pl.ended.load(std::sync::atomic::Ordering::Relaxed) {
            ed.player = None;
        }
    }
    if ed.player.is_none() {
        // Still frame for the playhead (re-requested only when it changes).
        let dragging = matches!(ed.drag, Some(Drag::Playhead) | Some(Drag::Edge { .. }));
        let key = ((ed.playhead * 1000.0).round() / 1000.0, pr.state.pieces.len() as u64);
        if ed.still_for != Some(key) {
            if let Some(s) = &ed.stills {
                s.request(pr, ed.playhead, pw, ph, dragging);
            }
            ed.still_for = Some(key);
        }
        shown = ed.stills.as_ref().and_then(|s| s.frame.lock().unwrap().clone());
    }

    let painter = ui.painter_at(area);
    painter.rect_filled(area, CornerRadius::same(8), Color32::from_rgb(12, 12, 13));
    // Texture for the frame.
    if let Some(f) = &shown {
        if ed.tex.as_ref().map(|(_, g)| *g) != Some(f.generation) {
            let img = egui::ColorImage::from_rgba_unmultiplied([f.w, f.h], &f.rgba);
            match &mut ed.tex {
                Some((t, g)) => {
                    t.set(img, egui::TextureOptions::LINEAR);
                    *g = f.generation;
                }
                None => {
                    ed.tex =
                        Some((ui.ctx().load_texture("editor-frame", img, egui::TextureOptions::LINEAR), f.generation))
                }
            }
        }
    }
    // Picture rect: the whole frame while cropping, otherwise just the cropped part.
    let crop = ed.crop_edit.or(pr.state.crop).unwrap_or(Crop::FULL);
    let (show_w, show_h, uv) = if ed.crop_mode {
        (fw as f32, fh as f32, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)))
    } else {
        let c = crop.clamped();
        (fw as f32 * c.w, fh as f32 * c.h, Rect::from_min_max(Pos2::new(c.x, c.y), Pos2::new(c.x + c.w, c.y + c.h)))
    };
    let inner = area.shrink(10.0);
    let k = (inner.width() / show_w).min(inner.height() / show_h);
    let pic = Rect::from_center_size(inner.center(), Vec2::new(show_w * k, show_h * k));
    if let Some((t, _)) = &ed.tex {
        painter.image(t.id(), pic, uv, Color32::WHITE);
    }
    if ed.crop_mode {
        crop_overlay(app, ed, ui, pic);
    }
    let _ = p;
}

fn crop_overlay(app: &mut App, ed: &mut EditorUi, ui: &mut egui::Ui, pic: Rect) {
    let Some(pr) = ed.project.as_mut() else { return };
    let (fw, fh) = pr.frame_size();
    let c = ed.crop_edit.or(pr.state.crop).unwrap_or(Crop::FULL).clamped();
    let r = Rect::from_min_size(
        Pos2::new(pic.min.x + c.x * pic.width(), pic.min.y + c.y * pic.height()),
        Vec2::new(c.w * pic.width(), c.h * pic.height()),
    );
    let painter = ui.painter_at(pic);
    let shade = Color32::from_black_alpha(150);
    painter.rect_filled(Rect::from_min_max(pic.min, Pos2::new(pic.max.x, r.min.y)), CornerRadius::ZERO, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(pic.min.x, r.max.y), pic.max), CornerRadius::ZERO, shade);
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(pic.min.x, r.min.y), Pos2::new(r.min.x, r.max.y)),
        CornerRadius::ZERO,
        shade,
    );
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(r.max.x, r.min.y), Pos2::new(pic.max.x, r.max.y)),
        CornerRadius::ZERO,
        shade,
    );
    painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Middle);
    // Thirds.
    for i in 1..3 {
        let x = r.min.x + r.width() * i as f32 / 3.0;
        let y = r.min.y + r.height() * i as f32 / 3.0;
        painter.line_segment(
            [Pos2::new(x, r.min.y), Pos2::new(x, r.max.y)],
            Stroke::new(1.0, Color32::from_white_alpha(60)),
        );
        painter.line_segment(
            [Pos2::new(r.min.x, y), Pos2::new(r.max.x, y)],
            Stroke::new(1.0, Color32::from_white_alpha(60)),
        );
    }
    let corners = [(false, false), (true, false), (false, true), (true, true)];
    for (right, bottom) in corners {
        let pt = Pos2::new(if right { r.max.x } else { r.min.x }, if bottom { r.max.y } else { r.min.y });
        painter.circle_filled(pt, 5.0, Color32::WHITE);
    }
    let resp = ui.interact(pic, ui.id().with("crop"), Sense::drag());
    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let handle = corners
                .into_iter()
                .find(|(right, bottom)| {
                    let pt = Pos2::new(if *right { r.max.x } else { r.min.x }, if *bottom { r.max.y } else { r.min.y });
                    pt.distance(pos) < 14.0
                })
                .map(|(rr, bb)| CropHandle::Corner(rr, bb))
                .unwrap_or(CropHandle::Body);
            ed.drag = Some(Drag::Crop { handle, start: c, grab: pos });
        }
    }
    if let (Some(Drag::Crop { handle, start, grab }), Some(pos)) = (&ed.drag, resp.interact_pointer_pos()) {
        let dx = (pos.x - grab.x) / pic.width();
        let dy = (pos.y - grab.y) / pic.height();
        let mut n = *start;
        match handle {
            CropHandle::Body => {
                n.x = start.x + dx;
                n.y = start.y + dy;
            }
            CropHandle::Corner(right, bottom) => {
                let (mut x0, mut y0, mut x1, mut y1) = (start.x, start.y, start.x + start.w, start.y + start.h);
                if *right {
                    x1 += dx
                } else {
                    x0 += dx
                }
                if *bottom {
                    y1 += dy
                } else {
                    y0 += dy
                }
                let (mut w, mut h) = ((x1 - x0).max(0.05), (y1 - y0).max(0.05));
                if let Aspect::Ratio(ratio) = ed.aspect {
                    // Keep the chosen shape: width follows height in pixels.
                    let frame = fw as f32 / fh as f32;
                    w = (h * ratio / frame).min(1.0);
                    h = w * frame / ratio;
                }
                n = Crop { x: if *right { x0 } else { x1 - w }, y: if *bottom { y0 } else { y1 - h }, w, h };
            }
        }
        ed.crop_edit = Some(n.clamped());
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    if resp.drag_stopped() {
        if let Some(c) = ed.crop_edit.take() {
            pr.set_crop(Some(c));
        }
        ed.drag = None;
    }
    let _ = app;
}

fn icon_button(
    ui: &mut egui::Ui,
    p: &Palette,
    draw: impl Fn(&egui::Painter, Rect, Color32),
    tip: &str,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        p.hairline
    } else if resp.hovered() {
        p.raised
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
    draw(ui.painter(), rect.shrink(10.0), p.text);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tip)
}

fn controls(app: &mut App, ed: &mut EditorUi, ui: &mut egui::Ui, area: Rect, _ffmpeg: &std::path::Path) {
    let p = app.p;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(Layout::left_to_right(Align::Center)));
    let ui = &mut child;
    let playing = ed.player.is_some();
    let play = icon_button(
        ui,
        &p,
        |pt, r, c| {
            if playing {
                let w = r.width() * 0.32;
                pt.rect_filled(Rect::from_min_size(r.min, Vec2::new(w, r.height())), CornerRadius::same(1), c);
                pt.rect_filled(
                    Rect::from_min_size(Pos2::new(r.max.x - w, r.min.y), Vec2::new(w, r.height())),
                    CornerRadius::same(1),
                    c,
                );
            } else {
                pt.add(egui::Shape::convex_polygon(
                    vec![
                        r.left_top() + Vec2::new(1.0, 0.0),
                        r.left_bottom() + Vec2::new(1.0, 0.0),
                        Pos2::new(r.max.x + 1.0, r.center().y),
                    ],
                    c,
                    Stroke::NONE,
                ));
            }
        },
        if playing { "Pause (Space)" } else { "Play (Space)" },
    );
    if play.clicked() {
        toggle_play(ed);
    }
    let Some(pr) = ed.project.as_mut() else { return };
    ui.add_space(6.0);
    ui.label(
        RichText::new(format!("{} / {}", clock(ed.playhead), clock(pr.duration())))
            .font(FontId::monospace(13.0))
            .color(p.text),
    );
    ui.add_space(14.0);
    let split = theme::quiet_button(ui, &p, "Split").on_hover_text("Cut at the playhead (S)");
    if split.clicked() && pr.split_at(ed.playhead) {
        ed.selected = pr.locate(ed.playhead).map(|(i, _)| i);
    }
    let can_delete = ed.selected.is_some() && pr.pieces().len() > 1;
    if ui
        .add_enabled_ui(can_delete, |ui| theme::quiet_button(ui, &p, "Delete piece"))
        .inner
        .on_hover_text("Remove the selected piece (Delete)")
        .on_disabled_hover_text("Select a piece on the timeline first. The last piece can't be removed.")
        .clicked()
    {
        if let Some(i) = ed.selected {
            if pr.delete(i) {
                ed.selected = None;
                ed.playhead = ed.playhead.min(pr.duration() - 0.001).max(0.0);
                ed.still_for = None;
            }
        }
    }
    ui.add_space(14.0);
    let crop_label = if ed.crop_mode {
        "Done cropping"
    } else if pr.state.crop.is_some() {
        "Crop (on)"
    } else {
        "Crop"
    };
    if theme::quiet_button(ui, &p, crop_label).clicked() {
        ed.crop_mode = !ed.crop_mode;
    }
    if ed.crop_mode {
        let (fw, fh) = pr.frame_size();
        for (label, a) in ASPECTS {
            let on = ed.aspect == a;
            let r = ui.selectable_label(on, label);
            if r.clicked() {
                ed.aspect = a;
                match a {
                    Aspect::Ratio(ratio) => pr.set_crop(Some(Crop::centered(ratio, fw, fh))),
                    Aspect::Free => {}
                }
            }
        }
        if pr.state.crop.is_some() && theme::link_button(ui, &p, "Reset").clicked() {
            pr.set_crop(None);
            ed.aspect = Aspect::Free;
        }
    }
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let mut vol = pr.state.volume;
        let r = ui.add_enabled(!pr.state.mute, egui::Slider::new(&mut vol, 0.0..=2.0).show_value(false));
        if r.drag_started() {
            ed.volume_before = Some(pr.state.clone());
        }
        if r.changed() {
            pr.set_volume(vol, false);
        }
        if r.drag_stopped() {
            if let Some(b) = ed.volume_before.take() {
                pr.commit_drag(b);
            }
        }
        ui.label(RichText::new(format!("{:.0}%", pr.state.volume * 100.0)).small().color(p.muted));
        let mut mute = pr.state.mute;
        if ui.checkbox(&mut mute, "Mute").changed() {
            pr.set_mute(mute);
            ed.player = None;
        }
    });
}

fn timeline(app: &mut App, ed: &mut EditorUi, ui: &mut egui::Ui, area: Rect) {
    let p = app.p;
    let Some(pr) = ed.project.as_ref() else { return };
    let dur = pr.duration().max(0.1);
    let width = area.width();
    if ed.zoom <= 0.0 {
        ed.zoom = (width - 2.0) / dur as f32; // fit everything at first
    }
    let min_zoom = ((width - 2.0) / dur as f32).min(40.0);
    let resp = ui.interact(area, ui.id().with("timeline"), Sense::click_and_drag());
    // Wheel: scroll; Ctrl+wheel: zoom around the pointer.
    if resp.hovered() {
        let (scroll, ctrl, pointer) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.command, i.pointer.hover_pos()));
        if ctrl && scroll.y != 0.0 {
            let px = pointer.map(|q| q.x - area.min.x).unwrap_or(width / 2.0);
            let t_at = (px + ed.scroll) / ed.zoom;
            ed.zoom = (ed.zoom * (1.0 + scroll.y * 0.003)).clamp(min_zoom, 400.0);
            ed.scroll = t_at * ed.zoom - px;
        } else if scroll.y != 0.0 || scroll.x != 0.0 {
            ed.scroll -= scroll.y + scroll.x;
        }
    }
    let content = dur as f32 * ed.zoom;
    ed.scroll = ed.scroll.clamp(0.0, (content - width + 2.0).max(0.0));
    // Keep the playhead in view while playing.
    if ed.player.is_some() {
        let x = ed.playhead as f32 * ed.zoom - ed.scroll;
        if x > width * 0.9 || x < 0.0 {
            ed.scroll = (ed.playhead as f32 * ed.zoom - width * 0.2).max(0.0);
        }
    }
    let x_of = |t: f64, scroll: f32, zoom: f32| area.min.x + t as f32 * zoom - scroll;
    let t_of = |x: f32, scroll: f32, zoom: f32| ((x - area.min.x + scroll) / zoom) as f64;

    let painter = ui.painter_at(area);
    painter.rect_filled(area, CornerRadius::same(8), p.panel);
    // Ruler.
    let ruler = Rect::from_min_size(area.min, Vec2::new(width, RULER_H));
    let step = [0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0]
        .into_iter()
        .find(|s| *s as f32 * ed.zoom >= 70.0)
        .unwrap_or(600.0);
    let mut t = (t_of(area.min.x, ed.scroll, ed.zoom) / step).floor() * step;
    while x_of(t, ed.scroll, ed.zoom) < area.max.x {
        let x = x_of(t, ed.scroll, ed.zoom);
        if t >= 0.0 && t <= dur {
            painter
                .line_segment([Pos2::new(x, ruler.max.y - 5.0), Pos2::new(x, ruler.max.y)], Stroke::new(1.0, p.faint));
            painter.text(
                Pos2::new(x + 3.0, ruler.min.y + 2.0),
                Align2::LEFT_TOP,
                format_secs(t as u32),
                FontId::proportional(10.5),
                p.faint,
            );
        }
        t += step;
    }

    // Pieces.
    let lane = Rect::from_min_max(
        Pos2::new(area.min.x, ruler.max.y + 2.0),
        Pos2::new(area.max.x, ruler.max.y + 2.0 + STRIP_H + WAVE_H),
    );
    let starts = pr.starts();
    let media = ed.strips.as_ref().map(|s| s.media.clone());
    let media = media.as_ref().map(|m| m.lock().unwrap());
    let mut hover_edge = None;
    let pointer = ui.input(|i| i.pointer.hover_pos());
    for (i, piece) in pr.pieces().iter().enumerate() {
        let x0 = x_of(starts[i], ed.scroll, ed.zoom);
        let x1 = x_of(starts[i] + piece.len(), ed.scroll, ed.zoom);
        if x1 < area.min.x || x0 > area.max.x {
            continue;
        }
        let block = Rect::from_min_max(Pos2::new(x0 + 1.0, lane.min.y), Pos2::new(x1 - 1.0, lane.max.y));
        let clip = block.intersect(area);
        let bp = ui.painter_at(clip);
        bp.rect_filled(block, CornerRadius::same(5), p.raised);
        // Thumbnails tiled across the strip.
        let strip = Rect::from_min_size(block.min, Vec2::new(block.width(), STRIP_H));
        let tile_w = STRIP_H * THUMB_W as f32 / THUMB_H as f32;
        if let Some(m) = media.as_ref().and_then(|m| m.get(&piece.source)) {
            let mut x = strip.min.x;
            while x < strip.max.x {
                let mid = t_of(x + tile_w / 2.0, ed.scroll, ed.zoom) - starts[i] + piece.start;
                if let Some((k, (_, px))) =
                    m.thumbs.iter().enumerate().min_by(|a, b| (a.1 .0 - mid).abs().total_cmp(&(b.1 .0 - mid).abs()))
                {
                    let key = (piece.source, k);
                    let tex = ed.thumbs.entry(key).or_insert_with(|| {
                        let img = egui::ColorImage::from_rgba_unmultiplied([THUMB_W as usize, THUMB_H as usize], px);
                        ui.ctx().load_texture(format!("thumb-{}-{k}", piece.source), img, egui::TextureOptions::LINEAR)
                    });
                    let r = Rect::from_min_size(Pos2::new(x, strip.min.y), Vec2::new(tile_w, STRIP_H));
                    bp.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
                x += tile_w;
            }
            // Waveform.
            let wave = Rect::from_min_max(Pos2::new(block.min.x, strip.max.y), block.max);
            bp.rect_filled(wave, CornerRadius::ZERO, p.bg);
            if !m.peaks.is_empty() {
                let mut x = wave.min.x.max(area.min.x);
                while x < wave.max.x.min(area.max.x) {
                    let st = t_of(x, ed.scroll, ed.zoom) - starts[i] + piece.start;
                    let idx = (st * PEAKS_PER_SEC as f64) as usize;
                    let v = m.peaks.get(idx).copied().unwrap_or(0.0).min(1.0);
                    let hh = (v * (WAVE_H - 4.0)).max(1.0);
                    let cy = wave.center().y;
                    bp.line_segment(
                        [Pos2::new(x, cy - hh / 2.0), Pos2::new(x, cy + hh / 2.0)],
                        Stroke::new(1.0, p.muted),
                    );
                    x += 2.0;
                }
            }
        }
        let selected = ed.selected == Some(i);
        bp.rect_stroke(
            block,
            CornerRadius::same(5),
            Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { p.text } else { p.hairline }),
            egui::StrokeKind::Inside,
        );
        if pr.pieces().len() > 1 || pr.sources.len() > 1 {
            let name =
                pr.sources[piece.source].path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let label = format!("{}  {}", format_secs(piece.len().round() as u32), name);
            let g = bp.layout_no_wrap(label, FontId::proportional(10.5), Color32::WHITE);
            let lr = Rect::from_min_size(block.min + Vec2::new(4.0, 4.0), g.size() + Vec2::new(8.0, 2.0));
            if lr.width() < block.width() - 8.0 {
                bp.rect_filled(lr, CornerRadius::same(3), Color32::from_black_alpha(150));
                bp.galley(lr.min + Vec2::new(4.0, 1.0), g, Color32::WHITE);
            }
        }
        if let Some(q) = pointer {
            if lane.contains(q) {
                if (q.x - block.min.x).abs() < EDGE_GRAB {
                    hover_edge = Some((i, true));
                } else if (q.x - block.max.x).abs() < EDGE_GRAB {
                    hover_edge = Some((i, false));
                }
            }
        }
    }
    drop(media);

    // Interaction.
    if hover_edge.is_some() || matches!(ed.drag, Some(Drag::Edge { .. })) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let pr = ed.project.as_mut().expect("project");
    if resp.drag_started() {
        if let Some(q) = resp.interact_pointer_pos() {
            ed.player = None;
            ed.drag = if q.y < lane.min.y {
                Some(Drag::Playhead)
            } else if let Some((piece, start)) = hover_edge {
                let pc = pr.pieces()[piece];
                Some(Drag::Edge {
                    piece,
                    start,
                    before: pr.state.clone(),
                    origin: if start { pc.start } else { pc.end },
                    grab_x: q.x,
                })
            } else if let Some((i, _)) = pr.locate(t_of(q.x, ed.scroll, ed.zoom)) {
                ed.selected = Some(i);
                Some(Drag::Move { piece: i, before_x: q.x, target: i })
            } else {
                None
            };
        }
    }
    let starts = pr.starts();
    if let (Some(drag), Some(q)) = (&mut ed.drag, resp.interact_pointer_pos()) {
        match drag {
            Drag::Playhead => ed.playhead = t_of(q.x, ed.scroll, ed.zoom).clamp(0.0, dur - 0.001),
            Drag::Edge { piece, start, origin, grab_x, .. } => {
                let v = *origin + ((q.x - *grab_x) / ed.zoom) as f64;
                pr.set_edge(*piece, *start, v, false);
                // Show the frame at the edge being moved.
                let st = pr.starts();
                let pc = pr.pieces()[*piece];
                ed.playhead = if *start { st[*piece] } else { (st[*piece] + pc.len() - 0.001).max(0.0) };
            }
            Drag::Move { piece, before_x, target } => {
                if (q.x - *before_x).abs() > 8.0 {
                    let t = t_of(q.x, ed.scroll, ed.zoom);
                    let mut to = pr.pieces().len() - 1;
                    for (k, s) in starts.iter().enumerate() {
                        if t < s + pr.pieces()[k].len() / 2.0 {
                            to = k;
                            break;
                        }
                    }
                    *target = to;
                    let x = x_of(starts.get(to).copied().unwrap_or(0.0), ed.scroll, ed.zoom);
                    ui.painter().line_segment(
                        [Pos2::new(x, lane.min.y - 4.0), Pos2::new(x, lane.max.y + 4.0)],
                        Stroke::new(3.0, p.replay),
                    );
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    let _ = piece;
                }
            }
            Drag::Crop { .. } => {}
        }
    }
    if resp.drag_stopped() {
        match ed.drag.take() {
            Some(Drag::Edge { before, .. }) => {
                pr.commit_drag(before);
                ed.still_for = None;
            }
            Some(Drag::Move { piece, target, before_x }) => {
                let moved = resp.interact_pointer_pos().is_some_and(|q| (q.x - before_x).abs() > 8.0);
                if moved && pr.move_piece(piece, target) {
                    ed.selected = Some(target);
                    ed.still_for = None;
                }
            }
            other => ed.drag = other.filter(|d| matches!(d, Drag::Crop { .. })),
        }
    }
    if resp.clicked() {
        if let Some(q) = resp.interact_pointer_pos() {
            ed.player = None;
            let t = t_of(q.x, ed.scroll, ed.zoom).clamp(0.0, dur - 0.001);
            ed.playhead = t;
            if q.y >= lane.min.y {
                ed.selected = pr.locate(t).map(|(i, _)| i);
            }
        }
    }
    // Playhead.
    let x = x_of(ed.playhead, ed.scroll, ed.zoom);
    if x >= area.min.x && x <= area.max.x {
        painter.line_segment([Pos2::new(x, area.min.y), Pos2::new(x, lane.max.y + 4.0)], Stroke::new(2.0, p.rec));
        painter.add(egui::Shape::convex_polygon(
            vec![Pos2::new(x - 6.0, area.min.y), Pos2::new(x + 6.0, area.min.y), Pos2::new(x, area.min.y + 8.0)],
            p.rec,
            Stroke::NONE,
        ));
    }
    // Hint line.
    let hint = if ed.zoom <= min_zoom + 0.01 {
        "Space play · S split · Delete remove piece · drag edges to trim, pieces to reorder · Ctrl+wheel zoom"
    } else {
        "Wheel scrolls · Ctrl+wheel zooms"
    };
    painter.text(
        Pos2::new(area.min.x + 6.0, area.max.y - 4.0),
        Align2::LEFT_BOTTOM,
        hint,
        FontId::proportional(10.5),
        p.faint,
    );
}

fn export_dialog(app: &mut App, ed: &mut EditorUi, ctx: &egui::Context, st: &Status, ffmpeg: &std::path::Path) {
    let p = app.p;
    let Some(pr) = ed.project.as_ref() else { return };
    // Running or finished export.
    if let Some(ex) = &ed.export {
        let res = ex.result.lock().unwrap().clone();
        let (done, total) = *ex.progress.lock().unwrap();
        let mut close = false;
        egui::Modal::new(egui::Id::new("editor_export")).show(ctx, |ui| {
            ui.set_width(400.0);
            match &res {
                None => {
                    ui.label(RichText::new("Exporting…").font(theme::semibold(16.0)));
                    ui.add_space(8.0);
                    let frac = if total > 0.0 { (done / total) as f32 } else { 0.0 };
                    ui.add(egui::ProgressBar::new(frac).desired_width(380.0).show_percentage());
                    ui.add_space(8.0);
                    if theme::quiet_button(ui, &p, "Cancel").clicked() {
                        ex.cancel();
                    }
                }
                Some(Ok(path)) => {
                    ui.label(RichText::new("Saved").font(theme::semibold(16.0)));
                    ui.add_space(4.0);
                    theme::note(ui, &p, &path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if theme::primary_button(ui, &p, "Play").clicked() {
                            super::home::open_file(path);
                        }
                        if theme::quiet_button(ui, &p, "Show in folder").clicked() {
                            urec_win::reveal_in_explorer(path);
                        }
                        if theme::quiet_button(ui, &p, "Keep editing").clicked() {
                            close = true;
                        }
                    });
                }
                Some(Err(e)) => {
                    ui.label(RichText::new("Export didn't finish").font(theme::semibold(16.0)));
                    ui.add_space(4.0);
                    theme::note(ui, &p, e);
                    ui.add_space(12.0);
                    if theme::quiet_button(ui, &p, "Back").clicked() {
                        close = true;
                    }
                }
            }
        });
        if res.is_none() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if close {
            if let Some(Ok(_)) = &res {
                app.library.refresh(&app.s.output_dir());
            }
            ed.export = None;
        }
        return;
    }
    let Some(dlg) = ed.dialog.as_mut() else { return };
    let mut cancel = false;
    let mut go = false;
    let lossless = pr.lossless_possible();
    let hevc_ok = st.best_encoder(Codec::Hevc).is_some();
    egui::Modal::new(egui::Id::new("editor_export_setup")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.label(RichText::new("Export").font(theme::semibold(16.0)));
        ui.add_space(10.0);
        ui.label(RichText::new("Name").color(p.muted));
        ui.add(egui::TextEdit::singleline(&mut dlg.name).desired_width(420.0));
        ui.add_space(10.0);
        let (ow, oh) = pr.output_size();
        theme::note(
            ui,
            &p,
            &format!("{} · {}×{} · saved next to the original, which stays as it is.", clock(pr.duration()), ow, oh),
        );
        ui.add_space(10.0);
        ui.radio_value(&mut dlg.mode, ExportMode::Exact, "Exact cuts (re-encodes; uses the graphics card when it can)");
        let r = ui.add_enabled_ui(lossless.is_ok(), |ui| {
            ui.radio_value(
                &mut dlg.mode,
                ExportMode::Lossless,
                "Lossless and fast (each piece may start up to 2 s early)",
            )
        });
        if let Err(why) = lossless {
            r.response.on_disabled_hover_text(format!("Not possible here: {why}."));
        }
        if dlg.mode == ExportMode::Exact {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.radio_value(&mut dlg.codec, Codec::H264, "H.264 (plays everywhere)");
                ui.add_enabled_ui(hevc_ok, |ui| ui.radio_value(&mut dlg.codec, Codec::Hevc, "HEVC (smaller)"));
            });
        }
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let ok = !urec_engine::naming::sanitize(&dlg.name).is_empty();
            if ui.add_enabled_ui(ok, |ui| theme::primary_button(ui, &p, "Export")).inner.clicked() {
                go = true;
            }
            if theme::quiet_button(ui, &p, "Cancel").clicked() {
                cancel = true;
            }
        });
    });
    if cancel {
        ed.dialog = None;
    }
    if go {
        let dlg = ed.dialog.take().expect("dialog");
        let dir = pr.sources[0].path.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| app.s.output_dir());
        let out = dir.join(format!("{}.mp4", urec_engine::naming::sanitize(&dlg.name)));
        let (ow, oh) = pr.output_size();
        let fps = pr.fps().round().max(1.0) as u32;
        let enc = st.best_encoder(dlg.codec).unwrap_or_else(|| Encoder::new(Backend::Software, dlg.codec));
        let v = VideoSettings {
            codec: dlg.codec,
            rate_control: RateControl::Quality,
            quality: 8,
            bitrate_kbps: video::recommended_bitrate_kbps(ow, oh, fps, dlg.codec, 1.5),
            ..VideoSettings::default()
        };
        let args = enc.args(&v, fps);
        crate::controller::log(&format!(
            "editor: exporting {} piece(s), {:.1} s, {:?} via {}",
            pr.pieces().len(),
            pr.duration(),
            dlg.mode,
            enc
        ));
        ed.export = Some(Export::start(ffmpeg.to_path_buf(), pr.clone(), dlg.mode, args, out, app.repaint_handle()));
    }
}
