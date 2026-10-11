//! First-run setup: what uRec found, a quality preset, where files go, and a real 5-second
//! test recording. Every step can be skipped; everything here is also in Settings.

use super::settings_page::{caps, recommended};
use super::{theme, App, Page};
use crate::controller::{format_secs, preset_id, Command, Status, TestCheck};
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, Vec2};
use urec_engine::encoder::Backend;
use urec_engine::replay;
use urec_engine::video::{self, Codec, Preset, Resolution};

pub const STEPS: [&str; 4] = ["What uRec found", "Quality", "Where files go", "Test"];

/// ✓ / ✕ / – drawn as shapes (not font glyphs), so they look the same everywhere.
fn status_icon(ui: &mut egui::Ui, p: &theme::Palette, ok: Option<bool>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
    let c = rect.center();
    let painter = ui.painter();
    match ok {
        Some(true) => {
            painter.circle_filled(c, 8.0, p.ok);
            let s = Stroke::new(1.8, Color32::WHITE);
            painter.line_segment([c + Vec2::new(-3.6, 0.2), c + Vec2::new(-1.0, 2.8)], s);
            painter.line_segment([c + Vec2::new(-1.0, 2.8), c + Vec2::new(3.8, -2.6)], s);
        }
        Some(false) => {
            painter.circle_filled(c, 8.0, p.rec);
            let s = Stroke::new(1.8, Color32::WHITE);
            painter.line_segment([c + Vec2::new(-2.8, -2.8), c + Vec2::new(2.8, 2.8)], s);
            painter.line_segment([c + Vec2::new(-2.8, 2.8), c + Vec2::new(2.8, -2.8)], s);
        }
        None => {
            painter.circle_stroke(c, 7.5, Stroke::new(1.2, p.faint));
            painter.line_segment([c + Vec2::new(-3.0, 0.0), c + Vec2::new(3.0, 0.0)], Stroke::new(1.5, p.faint));
        }
    }
}

fn line(ui: &mut egui::Ui, p: &theme::Palette, ok: Option<bool>, title: &str, detail: &str) {
    ui.add_space(5.0);
    ui.horizontal_top(|ui| {
        status_icon(ui, p, ok);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).color(p.text));
            if !detail.is_empty() {
                ui.add(egui::Label::new(RichText::new(detail).small().color(p.muted)).wrap());
            }
        });
    });
}

pub fn finish(app: &mut App) {
    app.s.setup_done = true;
    app.commit();
    app.page = Page::Home;
}

pub fn show(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    let step = app.setup_step.min(STEPS.len() - 1);
    // Header: title, step, skip.
    ui.horizontal(|ui| {
        ui.label(RichText::new("Set up uRec").font(theme::semibold(22.0)).color(p.text));
        ui.add_space(10.0);
        ui.label(RichText::new(format!("Step {} of {} · {}", step + 1, STEPS.len(), STEPS[step])).color(p.muted));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::link_button(ui, &p, "Skip setup")
                .on_hover_text("You can change everything later in Settings.")
                .clicked()
            {
                finish(app);
            }
        });
    });
    // Progress strip.
    ui.add_space(10.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 3.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(1), p.hairline);
    let mut done = rect;
    done.set_width(rect.width() * (step + 1) as f32 / STEPS.len() as f32);
    ui.painter().rect_filled(done, CornerRadius::same(1), p.text);
    ui.add_space(16.0);

    // Explicit rectangles: the footer is pinned to the bottom, the steps scroll above it.
    let rest = ui.available_rect_before_wrap();
    let footer_h = 58.0;
    let body = egui::Rect::from_min_max(rest.min, egui::pos2(rest.max.x, rest.max.y - footer_h));
    let footer = egui::Rect::from_min_max(egui::pos2(rest.min.x, rest.max.y - footer_h), rest.max);
    ui.scope_builder(egui::UiBuilder::new().max_rect(body).id_salt("setup_body"), |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width((ui.available_width() - 16.0).min(720.0));
            match step {
                0 => found(app, ui, st),
                1 => quality(app, ui, st),
                2 => storage(app, ui, st),
                _ => test(app, ui, st),
            }
        });
    });
    // Footer: back / next.
    let mut fui = ui.new_child(egui::UiBuilder::new().max_rect(footer).layout(Layout::top_down(Align::Min)));
    let ui = &mut fui;
    ui.add_space(8.0);
    theme::hairline(ui, &p);
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if step > 0 && theme::quiet_button(ui, &p, "Back").clicked() {
            app.setup_step -= 1;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let testing = st.test.as_ref().is_some_and(|t| t.running);
            if step + 1 < STEPS.len() {
                if theme::primary_button(ui, &p, "Next").clicked() {
                    app.setup_step += 1;
                }
            } else {
                let r = ui.add_enabled_ui(!testing, |ui| theme::primary_button(ui, &p, "Start using uRec")).inner;
                if r.clicked() {
                    finish(app);
                }
            }
        });
    });
}

fn found(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    app.refresh_devices(false);
    theme::note(
        ui,
        &p,
        "uRec checked this PC. Nothing is assumed from the graphics card's brand: each encoder was actually tried.",
    );
    theme::group_title(ui, &p, "Screen capture");
    let build = urec_win::windows_build().map(|b| format!("Windows build {b}")).unwrap_or_default();
    line(
        ui,
        &p,
        Some(urec_win::supports_wgc()),
        "Windows Graphics Capture",
        if urec_win::supports_wgc() { "Preferred: captures games and windows with little overhead." } else { &build },
    );
    let dda = app.monitors.iter().any(|m| m.dda_capable());
    line(
        ui,
        &p,
        Some(dda),
        "Desktop Duplication",
        if dda { "Fallback for whole displays." } else { "Not available here." },
    );
    line(ui, &p, Some(true), "GDI", "Last resort. Works everywhere, uses more CPU.");
    let mons: Vec<String> = app
        .monitors
        .iter()
        .map(|m| {
            format!(
                "{}×{}{}{}",
                m.bounds.width,
                m.bounds.height,
                m.refresh_hz.map(|h| format!(" at {h} Hz")).unwrap_or_default(),
                if m.primary { " (main)" } else { "" }
            )
        })
        .collect();
    if !mons.is_empty() {
        ui.add_space(4.0);
        theme::note(ui, &p, &format!("Displays: {}", mons.join(", ")));
    }

    theme::group_title(ui, &p, "Video encoders");
    if st.probing || !st.ready {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(14.0).color(p.muted));
            ui.label(RichText::new("Testing which encoders work on this PC…").color(p.muted));
        });
    } else {
        for backend in Backend::PREFERENCE {
            let Some(r) = st.probe.iter().find(|r| r.encoder.backend == backend && r.encoder.codec == Codec::H264)
            else {
                continue;
            };
            let others: Vec<String> = st
                .probe
                .iter()
                .filter(|o| o.encoder.backend == backend && o.encoder.codec != Codec::H264 && o.ok)
                .map(|o| o.encoder.codec.to_string())
                .collect();
            let detail = if r.ok {
                let mut d = if backend.is_hardware() {
                    "Graphics card: low CPU use.".to_string()
                } else {
                    "Runs on the CPU.".to_string()
                };
                if !others.is_empty() {
                    d += &format!(" Also {}.", others.join(", "));
                }
                d
            } else {
                r.reason.clone()
            };
            line(ui, &p, Some(r.ok), &format!("{} (H.264)", backend.label()), &detail);
        }
        if st.best_encoder(Codec::H264).is_none() {
            ui.add_space(6.0);
            ui.label(
                RichText::new("No working encoder was found, so recording isn't possible on this PC yet.").color(p.rec),
            );
        }
    }

    theme::group_title(ui, &p, "Audio");
    let def_out = app.speakers.iter().find(|d| d.is_default).or(app.speakers.first());
    line(
        ui,
        &p,
        Some(def_out.is_some()),
        "Game and desktop sound",
        &def_out.map(|d| d.name.clone()).unwrap_or_else(|| "No sound output device found.".into()),
    );
    let def_mic = app.mics.iter().find(|d| d.is_default).or(app.mics.first());
    line(
        ui,
        &p,
        def_mic.map(|_| true),
        "Microphone",
        &match def_mic {
            Some(d) => format!("{} (recorded only if you turn it on)", d.name),
            None => "No microphone found.".into(),
        },
    );
}

fn quality(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    let caps = caps(app, st);
    let rec = recommended(&caps);
    let enc = st.best_encoder(Codec::H264);
    theme::note(
        ui,
        &p,
        &format!(
            "Recommended for this PC: {}. Encoder: {}.",
            rec.label(),
            enc.map(|e| e.backend.label().to_string()).unwrap_or_else(|| "none found".into())
        ),
    );
    ui.add_space(10.0);
    let current = app.s.video.preset.clone();
    for preset in [Preset::LowEnd, Preset::Balanced, Preset::High, Preset::VeryHigh] {
        let v = video::apply_preset(preset, &caps);
        let (w, h) = video::output_size(v.resolution, caps.source_width, caps.source_height);
        let fps = match v.frame_rate {
            video::FrameRate::Fixed(f) => f.to_string(),
            video::FrameRate::Source => "display".into(),
        };
        let per_min = video::bytes_per_second(v.bitrate_kbps, app.s.audio.bitrate_kbps) * 60;
        let selected = current == preset_id(preset);
        let size = Vec2::new(ui.available_width(), 58.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let painter = ui.painter();
        painter.rect(
            rect,
            CornerRadius::same(8),
            if selected {
                p.panel
            } else if resp.hovered() {
                p.panel.gamma_multiply(0.6)
            } else {
                p.bg
            },
            Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { p.text } else { p.hairline }),
            egui::StrokeKind::Inside,
        );
        let title = format!("{}{}", preset.label(), if preset == rec { "  ·  Recommended" } else { "" });
        painter.text(
            rect.left_top() + Vec2::new(16.0, 11.0),
            egui::Align2::LEFT_TOP,
            title,
            theme::semibold(14.5),
            p.text,
        );
        let res = if v.resolution == Resolution::Native { format!("{w}×{h} (native)") } else { format!("{w}×{h}") };
        painter.text(
            rect.left_top() + Vec2::new(16.0, 33.0),
            egui::Align2::LEFT_TOP,
            format!(
                "{res} · {fps} FPS · {:.0} Mbit/s · about {} per minute",
                v.bitrate_kbps as f32 / 1000.0,
                video::format_bytes(per_min)
            ),
            egui::FontId::proportional(12.5),
            p.muted,
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            app.s.set_video(&v);
            app.s.video.preset = preset_id(preset).into();
            app.commit();
        }
        ui.add_space(8.0);
    }
    theme::note(
        ui,
        &p,
        "Sizes are estimates: busy, detailed scenes use more space, still screens less. Resolution, frame rate, codec and bitrate can be fine-tuned later in Settings › Video.",
    );
}

fn storage(app: &mut App, ui: &mut egui::Ui, _st: &Status) {
    let p = app.p;
    let out = app.s.output_dir();
    let free = urec_win::disk_space(&out).map(|(f, _)| format!("{} free", video::format_bytes(f))).unwrap_or_default();
    theme::row(ui, &p, "Recordings and clips", &format!("{}  ·  {free}", out.display()), |ui| {
        if theme::quiet_button(ui, &p, "Change").clicked() {
            if let Some(d) = urec_win::pick_folder("Folder for recordings", Some(&out)) {
                let test = d.join(".urec-write-test");
                match std::fs::write(&test, b"ok") {
                    Ok(()) => {
                        let _ = std::fs::remove_file(test);
                        app.s.storage.output_dir = d.to_string_lossy().into();
                        app.library.refresh(&d);
                        app.commit();
                    }
                    Err(e) => app.push_notice(
                        crate::controller::Level::Error,
                        format!("uRec can't save to {}: {e}", d.display()),
                    ),
                }
            }
        }
    });
    let mut on = app.s.replay.enabled;
    theme::row(
        ui,
        &p,
        "Instant Replay",
        "Keeps the last moments in a buffer so you can save them after they happen. Off by default.",
        |ui| {
            theme::switch(ui, &p, &mut on);
        },
    );
    if on != app.s.replay.enabled {
        app.s.replay.enabled = on;
        app.commit();
    }
    let secs = app.s.replay.seconds;
    let bps = video::bytes_per_second(app.s.video.bitrate_kbps, app.s.audio.bitrate_kbps);
    let ring = replay::ring_bytes(secs, 2, bps);
    ui.add_space(8.0);
    theme::note(
        ui,
        &p,
        &format!(
            "The Instant Replay buffer lives in {} and uses at most about {} for {} at these settings. It never grows beyond that and is cleared when Instant Replay starts.",
            app.s.replay_dir().display(),
            video::format_bytes(ring),
            format_secs(secs)
        ),
    );
    ui.add_space(4.0);
    theme::note(ui, &p, "uRec never deletes your recordings. Uninstalling keeps them too.");
}

fn test(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    theme::note(
        ui,
        &p,
        "Record 5 seconds of your screen with these settings to check the whole chain: capture, encoding, sound, saving and playback.",
    );
    ui.add_space(12.0);
    let report = st.test.clone();
    let running = report.as_ref().is_some_and(|t| t.running);
    // CI: start the test by itself once uRec is ready (UREC_SETUP_AUTOTEST=1).
    if report.is_none() && st.ready && !st.probing && std::env::var_os("UREC_SETUP_AUTOTEST").is_some() {
        app.ctl.send(Command::TestRecording);
    }
    ui.horizontal(|ui| {
        let label = if report.as_ref().is_some_and(|t| !t.running) { "Test again" } else { "Record test" };
        let r = ui.add_enabled_ui(!running && st.ready && !st.probing, |ui| theme::primary_button(ui, &p, label)).inner;
        if r.clicked() {
            app.ctl.send(Command::TestRecording);
        }
        if running {
            ui.add_space(8.0);
            ui.add(egui::Spinner::new().size(16.0).color(p.muted));
            ui.label(RichText::new(report.as_ref().map(|t| t.stage.clone()).unwrap_or_default()).color(p.muted));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
    });
    let Some(t) = report else { return };
    ui.add_space(10.0);
    for TestCheck { name, ok, detail } in &t.checks {
        line(ui, &p, *ok, name, detail);
    }
    if t.running {
        return;
    }
    ui.add_space(12.0);
    if t.passed() {
        ui.label(RichText::new("Everything works.").font(theme::semibold(15.0)).color(p.ok));
    } else {
        ui.label(RichText::new("Something didn't work.").font(theme::semibold(15.0)).color(p.rec));
        theme::note(
            ui,
            &p,
            "Try the Low resource usage preset, or pick another encoder or capture method in Settings. You can also finish setup and change this later.",
        );
    }
    if let Some(f) = &t.file {
        ui.add_space(6.0);
        if theme::quiet_button(ui, &p, "Play test clip").clicked() {
            urec_win::open_file(f);
        }
    }
}
