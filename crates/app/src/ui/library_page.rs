//! Library: every recording with thumbnail, duration, resolution, frame rate, size and date.
//! Search, sort, play, show in folder, rename, copy path, delete (to the Recycle Bin).

use super::home::open_file;
use super::theme;
use super::App;
use crate::controller::{Level, Status};
use crate::library::{format_duration, Item};
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Vec2};
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
    pub confirm_delete: Option<Item>,
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
            if app.library.scanning.load(std::sync::atomic::Ordering::Relaxed) {
                ui.spinner();
            }
        });
    });
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.lib.search)
                .hint_text("Search recordings")
                .desired_width(280.0)
                .margin(egui::Margin::symmetric(10, 8)),
        );
        egui::ComboBox::from_id_salt("lib_sort").selected_text(app.lib.sort.label()).width(150.0).show_ui(ui, |ui| {
            for s in Sort::ALL {
                ui.selectable_value(&mut app.lib.sort, s, s.label());
            }
        });
    });
    ui.add_space(12.0);

    let q = app.lib.search.to_lowercase();
    if !q.is_empty() {
        items.retain(|i| i.name.to_lowercase().contains(&q) || i.app.to_lowercase().contains(&q));
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

    if items.is_empty() {
        ui.add_space(30.0);
        if q.is_empty() {
            ui.label(RichText::new("No recordings yet").font(theme::semibold(16.0)));
            theme::note(
                ui,
                &p,
                &format!("Recordings and saved replays appear here. They're saved to {}.", app.s.storage.output_dir),
            );
        } else {
            theme::note(ui, &p, "Nothing matches your search.");
        }
        return;
    }

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
    ui.add_space(6.0);
    let offset = utc_offset();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 92.0, items.len(), |ui, range| {
        for item in &items[range] {
            row(app, ui, item, offset);
        }
    });
    delete_dialog(app, ui.ctx());
}

fn row(app: &mut App, ui: &mut egui::Ui, item: &Item, offset: i64) {
    let p = app.p;
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 88.0), Sense::hover());
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(8), p.panel);
    }
    let thumb = egui::Rect::from_min_size(rect.left_top() + Vec2::new(6.0, 6.0), Vec2::new(135.0, 76.0));
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
    let thumb_resp = ui.interact(thumb, ui.id().with(("thumb", &item.path)), Sense::click());
    if thumb_resp.clicked() {
        open_file(&item.path);
    }
    if thumb_resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let text_rect = egui::Rect::from_min_max(
        egui::pos2(thumb.right() + 16.0, rect.top() + 8.0),
        rect.right_bottom() - Vec2::new(8.0, 8.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(Layout::top_down(Align::Min)));
    let renaming = app.lib.renaming.as_ref().is_some_and(|(p2, _)| p2 == &item.path);
    if renaming {
        let mut commit = false;
        let mut cancel = false;
        child.horizontal(|ui| {
            if let Some((_, name)) = app.lib.renaming.as_mut() {
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(300.0));
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
        child.add(egui::Label::new(RichText::new(&item.name).color(p.text)).truncate());
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
    child.label(RichText::new(meta.join(", ")).small().color(p.muted));
    child.add_space(4.0);
    child.horizontal(|ui| {
        if theme::link_button(ui, &p, "Play").clicked() {
            open_file(&item.path);
        }
        if theme::link_button(ui, &p, "Show in folder").clicked() {
            urec_win::reveal_in_explorer(&item.path);
        }
        if theme::link_button(ui, &p, "Rename").clicked() {
            app.lib.renaming = Some((item.path.clone(), item.name.clone()));
        }
        if theme::link_button(ui, &p, "Copy path").clicked() {
            ui.ctx().copy_text(item.path.to_string_lossy().into_owned());
            app.push_notice(Level::Info, "File path copied.".into());
        }
        if theme::link_button(ui, &p, "Delete").clicked() {
            app.lib.confirm_delete = Some(item.clone());
        }
    });
}

fn delete_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(item) = app.lib.confirm_delete.clone() else { return };
    let p = app.p;
    let mut close = false;
    let resp = egui::Modal::new(egui::Id::new("confirm_delete")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.label(RichText::new("Delete this recording?").font(theme::semibold(16.0)));
        ui.add_space(4.0);
        theme::note(
            ui,
            &p,
            &format!("\"{}\" will be moved to the Recycle Bin. You can restore it from there.", item.name),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let b = egui::Button::new(RichText::new("Move to Recycle Bin").color(Color32::WHITE).strong()).fill(p.rec);
            if ui.add(b).clicked() {
                if let Err(e) = app.library.delete(&item) {
                    app.push_notice(Level::Error, e);
                }
                close = true;
            }
            if theme::quiet_button(ui, &p, "Keep it").clicked() {
                close = true;
            }
        });
    });
    if close || resp.should_close() {
        app.lib.confirm_delete = None;
    }
}
