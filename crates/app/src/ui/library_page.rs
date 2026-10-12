//! Library: every recording with thumbnail, duration, resolution, frame rate, size and date.
//! Search, sort, favorites, play, show in folder, rename, copy path, delete one or many
//! (always to the Recycle Bin).

use super::home::open_file;
use super::theme;
use super::App;
use crate::controller::{Level, Status};
use crate::library::{format_duration, Item};
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Vec2};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    Largest,
    Longest,
    Name,
}

impl Sort {
    const ALL: [Sort; 5] = [Sort::Newest, Sort::Oldest, Sort::Largest, Sort::Longest, Sort::Name];
    fn label(&self) -> &'static str {
        match self {
            Sort::Newest => "Newest first",
            Sort::Oldest => "Oldest first",
            Sort::Largest => "Largest first",
            Sort::Longest => "Longest first",
            Sort::Name => "Name",
        }
    }
}

#[derive(Default)]
pub struct LibraryUi {
    pub search: String,
    pub sort: Sort,
    pub renaming: Option<(PathBuf, String)>,
    pub confirm_delete: Option<Pending>,
    pub favorites_only: bool,
    pub selecting: bool,
    pub selected: HashSet<PathBuf>,
}

/// Act on a file only if it's still there; otherwise say so and re-scan.
fn still_there(app: &mut App, item: &Item) -> bool {
    if item.path.exists() {
        return true;
    }
    app.push_notice(Level::Warning, format!("\"{}\" was moved or deleted outside uRec.", item.name));
    app.library.refresh(&app.s.output_dir());
    false
}

/// Outline or filled star, drawn (no font glyph needed).
fn star(ui: &mut egui::Ui, p: &theme::Palette, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
    let c = rect.center();
    let pts: Vec<egui::Pos2> = (0..10)
        .map(|i| {
            let r = if i % 2 == 0 { 6.8 } else { 2.9 };
            let a = std::f32::consts::PI * (i as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
            c + Vec2::new(a.cos() * r, a.sin() * r)
        })
        .collect();
    let color = if on {
        p.replay
    } else if resp.hovered() {
        p.text
    } else {
        p.faint
    };
    if on {
        // The star isn't convex: fill it as ten triangles around the centre.
        for i in 0..pts.len() {
            let tri = vec![c, pts[i], pts[(i + 1) % pts.len()]];
            ui.painter().add(egui::Shape::convex_polygon(tri, color, egui::Stroke::NONE));
        }
    }
    ui.painter().add(egui::Shape::closed_line(pts, egui::Stroke::new(1.3, color)));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(if on { "Remove from favorites" } else { "Add to favorites" })
}

/// Seconds east of UTC for the local time zone (from the current clock).
fn utc_offset() -> i64 {
    let (y, mo, d, h, mi, s) = urec_win::local_time();
    // days_from_civil
    let y = y as i64 - i64::from(mo <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = mo as i64;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let local = days * 86_400 + h as i64 * 3600 + mi as i64 * 60 + s as i64;
    let utc = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    ((local - utc) as f64 / 900.0).round() as i64 * 900
}

fn format_date(t: SystemTime, offset: i64) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let s = urec_engine::naming::Stamp::from_unix(secs, offset as i32);
    let today = urec_engine::naming::Stamp::from_unix(now, offset as i32);
    let yesterday = urec_engine::naming::Stamp::from_unix(now - 86_400, offset as i32);
    let time = format!("{}:{:02}", s.hour, s.minute);
    if (s.year, s.month, s.day) == (today.year, today.month, today.day) {
        format!("Today {time}")
    } else if (s.year, s.month, s.day) == (yesterday.year, yesterday.month, yesterday.day) {
        format!("Yesterday {time}")
    } else {
        const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!("{} {} {}, {time}", M[(s.month as usize).saturating_sub(1).min(11)], s.day, s.year)
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui, _st: &Status) {
    let p = app.p;
    let mut items = app.library.snapshot();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Library").font(theme::semibold(22.0)));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::quiet_button(ui, &p, "Open folder").clicked() {
                let _ = std::fs::create_dir_all(app.s.output_dir());
                urec_win::reveal_in_explorer(&app.s.output_dir());
            }
            if theme::quiet_button(ui, &p, "Refresh").clicked() {
                app.library.refresh(&app.s.output_dir());
            }
            let label = if app.lib.selecting { "Done" } else { "Select" };
            if theme::quiet_button(ui, &p, label).clicked() {
                app.lib.selecting = !app.lib.selecting;
                app.lib.selected.clear();
            }
            if app.library.scanning.load(std::sync::atomic::Ordering::Relaxed) {
                ui.spinner();
            }
        });
    });
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.lib.search)
                .hint_text("Search recordings")
                .desired_width(260.0)
                .margin(egui::Margin::symmetric(10, 8)),
        );
        egui::ComboBox::from_id_salt("lib_sort").selected_text(app.lib.sort.label()).width(140.0).show_ui(ui, |ui| {
            for s in Sort::ALL {
                ui.selectable_value(&mut app.lib.sort, s, s.label());
            }
        });
        ui.add_space(8.0);
        theme::switch(ui, &p, &mut app.lib.favorites_only);
        ui.label(RichText::new("Favorites only").color(p.muted));
    });
    ui.add_space(12.0);

    let q = app.lib.search.to_lowercase();
    if !q.is_empty() {
        items.retain(|i| i.name.to_lowercase().contains(&q) || i.app.to_lowercase().contains(&q));
    }
    if app.lib.favorites_only {
        items.retain(|i| app.library.is_favorite(&i.path));
    }
    match app.lib.sort {
        Sort::Newest => items.sort_by_key(|i| std::cmp::Reverse(i.modified)),
        Sort::Oldest => items.sort_by_key(|i| i.modified),
        Sort::Largest => items.sort_by_key(|i| std::cmp::Reverse(i.size)),
        Sort::Longest => items.sort_by(|a, b| {
            let d = |i: &Item| i.meta.as_ref().map(|m| m.duration).unwrap_or(0.0);
            d(b).total_cmp(&d(a))
        }),
        Sort::Name => items.sort_by_key(|i| i.name.to_lowercase()),
    }
    // Selections only ever refer to files that are still listed.
    app.lib.selected.retain(|p| items.iter().any(|i| &i.path == p));

    if items.is_empty() {
        ui.add_space(30.0);
        if app.lib.favorites_only && q.is_empty() {
            theme::note(ui, &p, "No favorites yet. Star a recording to keep it here.");
        } else if q.is_empty() {
            ui.label(RichText::new("No recordings yet").font(theme::semibold(16.0)));
            theme::note(
                ui,
                &p,
                &format!(
                    "Recordings and saved replays appear here. They're saved to {}.",
                    app.s.output_dir().display()
                ),
            );
        } else {
            theme::note(ui, &p, "Nothing matches your search.");
        }
        return;
    }

    if app.lib.selecting {
        selection_bar(app, ui, &items);
    } else {
        let total: u64 = items.iter().map(|i| i.size).sum();
        ui.label(
            RichText::new(format!(
                "{} recording{}, {}",
                items.len(),
                if items.len() == 1 { "" } else { "s" },
                urec_engine::video::format_bytes(total)
            ))
            .small()
            .color(p.muted),
        );
    }
    ui.add_space(6.0);
    let offset = utc_offset();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (i, item) in items.iter().enumerate() {
            row(app, ui, item, offset, i == 0);
        }
    });
    delete_dialog(app, ui.ctx());
}

/// "3 selected · Select all · Clear" and the delete button.
fn selection_bar(app: &mut App, ui: &mut egui::Ui, items: &[Item]) {
    let p = app.p;
    let n = app.lib.selected.len();
    let bytes: u64 = items.iter().filter(|i| app.lib.selected.contains(&i.path)).map(|i| i.size).sum();
    ui.horizontal(|ui| {
        let text = if n == 0 {
            "Click recordings to select them".to_string()
        } else {
            format!("{n} selected, {}", urec_engine::video::format_bytes(bytes))
        };
        ui.label(RichText::new(text).small().color(p.muted));
        ui.add_space(8.0);
        if n < items.len() && theme::link_button(ui, &p, "Select all").clicked() {
            app.lib.selected = items.iter().map(|i| i.path.clone()).collect();
        }
        if n > 0 && theme::link_button(ui, &p, "Clear").clicked() {
            app.lib.selected.clear();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let r = ui.add_enabled_ui(n > 0, |ui| theme::danger_button(ui, &p, "Delete")).inner;
            if r.clicked() {
                app.lib.confirm_delete = Some(Pending::Many(app.lib.selected.iter().cloned().collect()));
            }
            // Edit the selection as one video, oldest first.
            let r = ui
                .add_enabled_ui(n > 0, |ui| theme::quiet_button(ui, &p, "Edit together"))
                .inner
                .on_hover_text("Join the selected clips in the editor, oldest first");
            if r.clicked() {
                let mut picked: Vec<&Item> = items.iter().filter(|i| app.lib.selected.contains(&i.path)).collect();
                picked.sort_by_key(|i| i.modified);
                let paths = picked.into_iter().map(|i| i.path.clone()).collect();
                app.lib.selecting = false;
                app.lib.selected.clear();
                app.open_editor(paths);
            }
        });
    });
}

/// Square checkbox drawn to match the theme.
fn check_box(ui: &mut egui::Ui, p: &theme::Palette, rect: egui::Rect, on: bool) {
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, CornerRadius::same(4), p.text);
        let s = egui::Stroke::new(1.8, p.bg);
        let c = rect.center();
        painter.line_segment([c + Vec2::new(-4.0, 0.2), c + Vec2::new(-1.2, 3.0)], s);
        painter.line_segment([c + Vec2::new(-1.2, 3.0), c + Vec2::new(4.2, -2.8)], s);
    } else {
        painter.rect_stroke(rect, CornerRadius::same(4), egui::Stroke::new(1.3, p.faint), egui::StrokeKind::Inside);
    }
}

fn row(app: &mut App, ui: &mut egui::Ui, item: &Item, offset: i64, first: bool) {
    let p = app.p;
    let selecting = app.lib.selecting;
    let selected = app.lib.selected.contains(&item.path);
    // Paint the background after the content is laid out, so the row always wraps its content.
    let bg = ui.painter().add(egui::Shape::Noop);
    let full_w = ui.available_width();
    let inner = egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 8)).show(ui, |ui| {
        ui.set_width(full_w - 16.0);
        ui.horizontal_top(|ui| {
            if selecting {
                let (r, _) = ui.allocate_exact_size(Vec2::new(26.0, 76.0), Sense::hover());
                check_box(ui, &p, egui::Rect::from_center_size(r.center(), Vec2::splat(18.0)), selected);
            }
            thumbnail(app, ui, item, selecting, first);
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                details(app, ui, item, offset, selecting, first);
            });
        });
    });
    let rect = inner.response.rect;
    // The whole row is clickable only while selecting. Outside select mode it must only sense
    // hover: it's added after the row's buttons, so a clickable row would sit on top of them
    // and take every click (Play, Edit, Show in folder and the thumbnail did nothing in 0.3/0.4).
    let sense = if selecting { Sense::click() } else { Sense::hover() };
    let resp = ui.interact(rect, ui.id().with(("row", &item.path)), sense);
    if selecting && resp.clicked() {
        if selected {
            app.lib.selected.remove(&item.path);
        } else {
            app.lib.selected.insert(item.path.clone());
        }
    }
    let fill = if selected {
        p.raised
    } else if resp.hovered() || resp.contains_pointer() {
        p.panel
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().set(bg, egui::Shape::rect_filled(rect, CornerRadius::same(8), fill));
    ui.add_space(4.0);
}

fn thumbnail(app: &mut App, ui: &mut egui::Ui, item: &Item, selecting: bool, first: bool) {
    let p = app.p;
    let (thumb, thumb_resp) = ui.allocate_exact_size(Vec2::new(135.0, 76.0), Sense::click());
    if let Some(tex) = item.thumb.as_ref().and_then(|t| app.thumb(ui.ctx(), t)) {
        ui.painter().image(
            tex,
            thumb,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        ui.painter().rect_filled(thumb, CornerRadius::same(4), p.raised);
    }
    if let Some(m) = &item.meta {
        let badge = format_duration(m.duration);
        let pos = thumb.right_bottom() - Vec2::new(6.0, 6.0);
        let galley = ui.painter().layout_no_wrap(badge, egui::FontId::proportional(11.0), Color32::WHITE);
        let bg = egui::Rect::from_min_max(pos - galley.size() - Vec2::new(8.0, 4.0), pos);
        ui.painter().rect_filled(bg, CornerRadius::same(3), Color32::from_black_alpha(170));
        ui.painter().galley(bg.min + Vec2::new(4.0, 2.0), galley, Color32::WHITE);
    }
    if selecting {
        return;
    }
    if first {
        hitmap::place(ui, "thumbnail", thumb);
    }
    if thumb_resp.clicked() {
        hitmap::clicked(first, "thumbnail");
        if still_there(app, item) {
            open_file(&item.path);
        }
    }
    if thumb_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

fn details(app: &mut App, ui: &mut egui::Ui, item: &Item, offset: i64, selecting: bool, first: bool) {
    let p = app.p;
    let renaming = app.lib.renaming.as_ref().is_some_and(|(p2, _)| p2 == &item.path);
    if renaming {
        let mut commit = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            if let Some((_, name)) = app.lib.renaming.as_mut() {
                let w = (ui.available_width() - 130.0).clamp(120.0, 320.0);
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(w));
                r.request_focus();
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    commit = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            }
            if theme::quiet_button(ui, &p, "Save").clicked() {
                commit = true;
            }
            if theme::link_button(ui, &p, "Cancel").clicked() {
                cancel = true;
            }
        });
        if commit {
            if let Some((_, name)) = app.lib.renaming.take() {
                if let Err(e) = app.library.rename(item, &name) {
                    app.push_notice(Level::Error, e);
                }
            }
        } else if cancel {
            app.lib.renaming = None;
        }
    } else {
        ui.horizontal(|ui| {
            if !selecting {
                let fav = app.library.is_favorite(&item.path);
                if star(ui, &p, fav).clicked() {
                    app.library.set_favorite(&item.path, !fav);
                }
            }
            ui.add(egui::Label::new(RichText::new(&item.name).color(p.text)).truncate()).on_hover_text(&item.name);
        });
    }
    let mut meta = Vec::new();
    if let Some(m) = &item.meta {
        meta.push(format!("{} × {}", m.width, m.height));
        if m.fps > 0.0 {
            meta.push(format!("{:.0} FPS", m.fps));
        }
    }
    meta.push(urec_engine::video::format_bytes(item.size));
    meta.push(format_date(item.modified, offset));
    if !item.app.is_empty() {
        meta.push(item.app.clone());
    }
    ui.add(egui::Label::new(RichText::new(meta.join(" · ")).small().color(p.muted)).truncate());
    if let Some(problem) = &item.problem {
        ui.label(RichText::new(problem).small().color(p.warn));
    }
    if selecting {
        return;
    }
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        let r = theme::link_button(ui, &p, "Play");
        if first {
            hitmap::place(ui, "play", r.rect);
        }
        if r.clicked() {
            hitmap::clicked(first, "play");
            if still_there(app, item) {
                open_file(&item.path);
            }
        }
        ui.add_space(6.0);
        let r = theme::link_button(ui, &p, "Edit").on_hover_text("Cut, join and crop in uRec's editor");
        if first {
            hitmap::place(ui, "edit", r.rect);
        }
        if r.clicked() {
            hitmap::clicked(first, "edit");
            if still_there(app, item) {
                app.open_editor(vec![item.path.clone()]);
            }
        }
        ui.add_space(6.0);
        let r = theme::link_button(ui, &p, "Show in folder");
        if first {
            hitmap::place(ui, "reveal", r.rect);
        }
        if r.clicked() {
            hitmap::clicked(first, "reveal");
            if still_there(app, item) {
                urec_win::reveal_in_explorer(&item.path);
            }
        }
        ui.add_space(6.0);
        let more = egui::Button::new(RichText::new("More").color(p.muted).underline()).frame(false);
        egui::containers::menu::MenuButton::from_button(more).ui(ui, |ui| {
            ui.set_min_width(150.0);
            if ui.button("Rename").clicked() {
                app.lib.renaming = Some((item.path.clone(), item.name.clone()));
                ui.close();
            }
            if ui.button("Copy path").clicked() {
                ui.ctx().copy_text(item.path.to_string_lossy().into_owned());
                app.push_notice(Level::Info, "File path copied.".into());
                ui.close();
            }
            ui.separator();
            if ui.button(RichText::new("Delete").color(p.rec)).clicked() {
                app.lib.confirm_delete = Some(Pending::One(item.clone()));
                ui.close();
            }
        });
    });
}

/// What the delete dialog is about.
pub enum Pending {
    One(Item),
    Many(Vec<PathBuf>),
}

fn delete_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(pending) = app.lib.confirm_delete.as_ref() else { return };
    let (title, body) = match pending {
        Pending::One(item) => (
            "Delete this recording?".to_string(),
            format!("\"{}\" will be moved to the Recycle Bin. You can restore it from there.", item.name),
        ),
        Pending::Many(paths) => (
            format!("Delete {} recording{}?", paths.len(), if paths.len() == 1 { "" } else { "s" }),
            "They will be moved to the Recycle Bin. You can restore them from there.".to_string(),
        ),
    };
    let p = app.p;
    let mut close = false;
    let mut confirm = false;
    let resp = egui::Modal::new(egui::Id::new("confirm_delete")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.label(RichText::new(title).font(theme::semibold(16.0)));
        ui.add_space(4.0);
        theme::note(ui, &p, &body);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if theme::danger_button(ui, &p, "Move to Recycle Bin").clicked() {
                confirm = true;
                close = true;
            }
            if theme::quiet_button(ui, &p, "Keep").clicked() {
                close = true;
            }
        });
    });
    if confirm {
        match app.lib.confirm_delete.take() {
            Some(Pending::One(item)) => {
                if let Err(e) = app.library.delete(&item) {
                    app.push_notice(Level::Error, e);
                }
            }
            Some(Pending::Many(paths)) => {
                let (ok, failed) = app.library.delete_many(&paths);
                if failed.is_empty() {
                    app.push_notice(Level::Info, format!("{ok} moved to the Recycle Bin."));
                } else {
                    app.push_notice(
                        Level::Error,
                        format!(
                            "{ok} moved to the Recycle Bin. {} couldn't be (open in another app?): {}",
                            failed.len(),
                            failed.join(", ")
                        ),
                    );
                }
                app.lib.selected.clear();
                app.lib.selecting = false;
            }
            None => {}
        }
    }
    if close || resp.should_close() {
        app.lib.confirm_delete = None;
    }
}

/// CI only: with `UREC_HITMAP=<file>`, writes where the first row's controls are on screen (in
/// pixels) and appends each click that reaches them to `<file>.clicks`, so a test can click them
/// with the real mouse and see the click arrive. Does nothing otherwise.
mod hitmap {
    use eframe::egui;
    use std::collections::BTreeMap;
    use std::sync::{Mutex, OnceLock};

    fn target() -> Option<&'static str> {
        static T: OnceLock<Option<String>> = OnceLock::new();
        T.get_or_init(|| std::env::var("UREC_HITMAP").ok().filter(|s| !s.is_empty())).as_deref()
    }

    pub fn place(ui: &egui::Ui, name: &str, r: egui::Rect) {
        let Some(path) = target() else { return };
        static LAST: Mutex<BTreeMap<String, (i32, i32)>> = Mutex::new(BTreeMap::new());
        let (inner, ppp) = ui.ctx().input(|i| (i.viewport().inner_rect, i.pixels_per_point));
        let Some(inner) = inner else { return };
        let c = (inner.min + r.center().to_vec2()) * ppp;
        let pos = (c.x.round() as i32, c.y.round() as i32);
        let mut last = LAST.lock().unwrap();
        if last.get(name) != Some(&pos) {
            last.insert(name.to_string(), pos);
            let text: String = last.iter().map(|(k, (x, y))| format!("{k} {x} {y}\n")).collect();
            let _ = std::fs::write(path, text);
        }
    }

    pub fn clicked(first: bool, name: &str) {
        if let (true, Some(path)) = (first, target()) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{path}.clicks")) {
                let _ = writeln!(f, "{name}");
            }
        }
    }
}
