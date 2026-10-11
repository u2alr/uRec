//! Home: the record control, what will be recorded, Instant Replay, audio and recent clips.

use super::theme;
use super::{App, Page, Section};
use crate::controller::{format_secs, Command, ReplayState, Status};
use crate::library::format_duration;
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, Vec2};
use urec_engine::encoder::Backend;
use urec_engine::hotkey::Hotkey;
use urec_engine::video;

fn hotkey_text(s: &str) -> Option<String> {
    Hotkey::parse(s).map(|h| h.to_text().replace('+', " + "))
}

pub fn show(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    app.refresh_devices(false);
    let p = app.p;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.set_max_width(ui.available_width().min(760.0));
        hero(app, ui, st);
        ui.add_space(26.0);
        replay_row(app, ui, st);
        theme::hairline(ui, &p);
        audio_row(app, ui, st);
        theme::hairline(ui, &p);
        ui.add_space(22.0);
        recent(app, ui);
    });
}

/// The large record button and status beside it.
fn hero(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    let recording = st.recording.is_some();
    let busy = st.starting || st.finishing || !st.ready;
    ui.horizontal(|ui| {
        let size = 92.0;
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
        let hovered = resp.hovered() && !busy;
        let painter = ui.painter();
        let c = rect.center();
        painter.circle(
            c,
            size / 2.0 - 1.0,
            if hovered { p.panel } else { p.bg },
            Stroke::new(2.0, if recording { p.rec } else { p.hairline }),
        );
        if recording {
            let sq = egui::Rect::from_center_size(c, Vec2::splat(26.0));
            painter.rect_filled(sq, CornerRadius::same(5), p.rec);
        } else {
            let r = if hovered { 24.0 } else { 22.0 };
            painter.circle_filled(c, r, if busy { p.faint } else { p.rec });
        }
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() && !busy {
            app.ctl.send(Command::ToggleRecording);
        }
        resp.on_hover_text(if recording { "Stop recording" } else { "Start recording" });

        ui.add_space(18.0);
        ui.vertical(|ui| {
            ui.add_space(8.0);
            let title = if !st.ready {
                "Getting ready".to_string()
            } else if st.finishing {
                "Saving recording".to_string()
            } else if st.starting {
                "Starting".to_string()
            } else if let Some(r) = &st.recording {
                let s = r.started.elapsed().as_secs();
                format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
            } else {
                "Ready to record".to_string()
            };
            let size = if recording { 38.0 } else { 26.0 };
            ui.label(RichText::new(title).font(theme::semibold(size)).color(p.text));
            ui.label(RichText::new(describe_capture(app, st)).color(p.muted));
            let key = hotkey_text(&app.s.hotkeys.record);
            let hint = match (recording, key) {
                (true, Some(k)) => format!("Press {k} to stop."),
                (false, Some(k)) => format!("Press {k} to start, even while a game is open."),
                (true, None) => "Click the button to stop.".into(),
                (false, None) => "Click the button to start.".into(),
            };
            if recording {
                let s = &st.stats;
                let health = if st.lagging {
                    format!("Falling behind: {} frames dropped", s.drop_frames)
                } else if s.drop_frames > 0 {
                    format!("{} frames dropped", s.drop_frames)
                } else {
                    format!("Running smoothly at {:.0} FPS", s.fps.max(0.0))
                };
                ui.label(RichText::new(health).small().color(if st.lagging { p.warn } else { p.faint }));
            }
            ui.label(RichText::new(hint).small().color(p.faint));
        });
    });
    ui.add_space(18.0);
    source_picker(app, ui, st);
    if !st.ready || st.probing {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Checking which encoders work on this PC").color(p.muted));
        });
    }
    welcome(app, ui, st);
}

/// "Display 1 at 1920 × 1080, 60 FPS, using AMD AMF (H.264)"
fn describe_capture(app: &App, st: &Status) -> String {
    if let Some(c) = &st.capture {
        return format!(
            "{} × {} at {} FPS, {} with {}, captured with {}",
            c.width,
            c.height,
            c.fps,
            codec_short(c.encoder.codec),
            encoder_short(c.encoder.backend),
            method_short(c.method)
        );
    }
    let src = source_label(app);
    let (sw, sh) = match app.s.capture.kind.as_str() {
        "window" => (0, 0),
        _ => app
            .monitors
            .iter()
            .find(|m| m.device == app.s.capture.monitor || app.s.capture.monitor.is_empty() && m.primary)
            .map(|m| (m.bounds.width, m.bounds.height))
            .unwrap_or((0, 0)),
    };
    let size = if sw > 0 {
        let (w, h) = video::output_size(app.s.resolution(), sw, sh);
        format!("{w} × {h}")
    } else {
        app.s.resolution().label()
    };
    let fps = if app.s.video.fps == 0 { "display refresh rate".into() } else { format!("{} FPS", app.s.video.fps) };
    let enc = if app.s.video.encoder == "auto" {
        st.best_encoder(app.s.codec())
            .map(|e| encoder_short(e.backend).to_string())
            .unwrap_or_else(|| "the best available encoder".into())
    } else {
        Backend::from_id(&app.s.video.encoder).map(|b| encoder_short(b).to_string()).unwrap_or_default()
    };
    format!("{src} at {size}, {fps}, {} with {enc}", codec_short(app.s.codec()))
}

pub fn codec_short(c: urec_engine::video::Codec) -> &'static str {
    match c {
        urec_engine::video::Codec::H264 => "H.264",
        urec_engine::video::Codec::Hevc => "HEVC",
        urec_engine::video::Codec::Av1 => "AV1",
    }
}

pub fn encoder_short(b: Backend) -> &'static str {
    match b {
        Backend::Nvenc => "NVIDIA NVENC",
        Backend::Amf => "AMD AMF",
        Backend::Qsv => "Intel Quick Sync",
        Backend::MediaFoundationHw => "the Windows hardware encoder",
        Backend::Software => "x264 on the CPU",
        Backend::MediaFoundation => "the Windows encoder on the CPU",
    }
}

fn method_short(m: urec_engine::pipeline::CaptureMethod) -> &'static str {
    match m {
        urec_engine::pipeline::CaptureMethod::Wgc => "Windows Graphics Capture",
        urec_engine::pipeline::CaptureMethod::Dda => "Desktop Duplication",
        urec_engine::pipeline::CaptureMethod::Gdi => "GDI",
        urec_engine::pipeline::CaptureMethod::Test => "a test pattern",
    }
}

pub fn source_label(app: &App) -> String {
    let c = &app.s.capture;
    match c.kind.as_str() {
        "window" => {
            let t = if c.window_title.is_empty() { &c.window_exe } else { &c.window_title };
            format!("The window \"{}\"", truncate(t, 40))
        }
        "region" => format!("A {} × {} area", c.region[2], c.region[3]),
        _ => app
            .monitors
            .iter()
            .find(|m| m.device == c.monitor)
            .or_else(|| app.monitors.iter().find(|m| m.primary))
            .map(|m| m.name.split(',').next().unwrap_or("Display").trim().to_string())
            .unwrap_or_else(|| "Your main display".into()),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    } else {
        s.to_string()
    }
}

fn source_picker(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    let locked = st.recording.is_some() || st.finishing;
    ui.add_enabled_ui(!locked, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Record").color(p.muted));
            let current = source_label(app);
            let mut changed = false;
            egui::ComboBox::from_id_salt("source").selected_text(current).width(320.0).show_ui(ui, |ui| {
                ui.label(RichText::new("Displays").small().color(p.muted));
                for m in app.monitors.clone() {
                    let sel = app.s.capture.kind == "monitor"
                        && (app.s.capture.monitor == m.device || app.s.capture.monitor.is_empty() && m.primary);
                    if ui.selectable_label(sel, &m.name).clicked() {
                        app.s.capture.kind = "monitor".into();
                        app.s.capture.monitor = m.device.clone();
                        changed = true;
                    }
                }
                ui.separator();
                ui.label(RichText::new("Windows").small().color(p.muted));
                if app.windows.is_empty() {
                    ui.label(RichText::new("No windows found").color(p.faint));
                }
                for w in app.windows.clone().into_iter().take(20) {
                    let sel = app.s.capture.kind == "window" && app.s.capture.window_title == w.title;
                    if ui.selectable_label(sel, truncate(&w.title, 48)).on_hover_text(&w.exe).clicked() {
                        app.s.capture.kind = "window".into();
                        app.s.capture.window_title = w.title.clone();
                        app.s.capture.window_exe = w.exe.clone();
                        changed = true;
                    }
                }
            });
            if changed {
                app.commit();
            }
            if theme::link_button(ui, &p, "More capture options").clicked() {
                app.page = Page::Settings;
                app.section = Section::Recording;
            }
        });
    });
}

/// Shown until the first recording exists: what uRec picked for this PC.
fn welcome(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    if !st.ready || st.probing || !app.library.snapshot().is_empty() || st.busy() {
        return;
    }
    let p = app.p;
    ui.add_space(18.0);
    let inner = ui.available_width() - 36.0;
    egui::Frame::new()
        .fill(p.panel)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(18, 14))
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.label(RichText::new("Set up for this PC").font(theme::semibold(15.0)));
            let enc = st
                .best_encoder(app.s.codec())
                .map(|e| {
                    if e.backend.is_hardware() {
                        format!("your graphics card's encoder ({})", e.backend.label())
                    } else {
                        "your processor (no hardware encoder was found)".into()
                    }
                })
                .unwrap_or_else(|| "no working encoder".into());
            theme::note(
                ui,
                &p,
                &format!(
                    "Videos are encoded with {enc}. Quality is set to {}. Recordings are saved to {}.",
                    preset_name(&app.s.video.preset),
                    app.s.storage.output_dir
                ),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::quiet_button(ui, &p, "Change quality").clicked() {
                    app.page = Page::Settings;
                    app.section = Section::Video;
                }
                if theme::quiet_button(ui, &p, "Change folder").clicked() {
                    app.page = Page::Settings;
                    app.section = Section::Storage;
                }
            });
        });
}

pub fn preset_name(id: &str) -> &'static str {
    match id {
        "low" => "Low-end PC",
        "balanced" => "Balanced",
        "high" => "High quality",
        "very_high" => "Very high quality",
        _ => "Custom",
    }
}

fn replay_row(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let mut on = app.s.replay.enabled;
        if theme::switch(ui, &p, &mut on).changed() {
            app.s.replay.enabled = on;
            let _ = app.s.save();
            app.ctl.send(Command::SetReplay(on));
        }
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.label("Instant Replay");
            let save_key =
                hotkey_text(&app.s.hotkeys.save_replay).map(|k| format!(" Press {k} to save.")).unwrap_or_default();
            let status = match &st.replay {
                ReplayState::On => format!("Keeping the last {}.{save_key}", format_secs(app.s.replay.seconds)),
                ReplayState::Starting => "Starting".into(),
                ReplayState::Error(e) => format!("Not running: {}", truncate(e, 90)),
                ReplayState::Off => {
                    format!("Off. Turn it on to keep the last {} ready to save.", format_secs(app.s.replay.seconds))
                }
            };
            let color = if matches!(st.replay, ReplayState::Error(_)) { p.warn } else { p.muted };
            ui.add(egui::Label::new(RichText::new(status).small().color(color)).wrap());
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled_ui(st.replay == ReplayState::On, |ui| {
                if theme::primary_button(ui, &p, "Save replay").clicked() {
                    app.ctl.send(Command::SaveReplay);
                }
            });
            let mut secs = app.s.replay.seconds;
            egui::ComboBox::from_id_salt("replay_len").selected_text(format_secs(secs)).width(96.0).show_ui(ui, |ui| {
                for c in crate::settings::REPLAY_CHOICES {
                    ui.selectable_value(&mut secs, c, format_secs(c));
                }
            });
            if secs != app.s.replay.seconds {
                app.s.replay.seconds = secs;
                app.custom_replay = secs.to_string();
                app.commit();
            }
        });
    });
    ui.add_space(10.0);
}

fn audio_row(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    ui.add_space(10.0);
    let live = st.capture.is_some();
    let source = |ui: &mut egui::Ui, name: &str, enabled: bool, state: &Option<urec_win::SourceState>, level: f32| {
        ui.vertical(|ui| {
            ui.label(name);
            let (text, active) = match (enabled, state) {
                (false, _) => ("Off".to_string(), false),
                (true, Some(urec_win::SourceState::Active { device })) => (truncate(device, 34), true),
                (true, Some(urec_win::SourceState::Lost { device })) => {
                    (format!("{} disconnected", truncate(device, 24)), false)
                }
                (true, Some(urec_win::SourceState::Failed { reason })) => {
                    (crate::controller::explain_audio_failure(reason), false)
                }
                (true, _) if live => ("Not recording".into(), false),
                (true, _) => ("Recorded when capture runs".into(), false),
            };
            theme::meter(ui, &p, level, 180.0, active);
            ui.add(egui::Label::new(RichText::new(text).small().color(p.muted)).truncate());
        });
    };
    ui.horizontal(|ui| {
        ui.set_min_height(56.0);
        let w = (ui.available_width() - 120.0) / 2.0;
        ui.allocate_ui(Vec2::new(w, 56.0), |ui| {
            source(ui, "Game and desktop sound", app.s.audio.system, &st.audio.0, st.levels.0)
        });
        ui.allocate_ui(Vec2::new(w, 56.0), |ui| {
            let label = if st.mic_muted { "Microphone (muted)" } else { "Microphone" };
            source(ui, label, app.s.audio.mic, &st.audio.1, if st.mic_muted { 0.0 } else { st.levels.1 })
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled_ui(app.s.audio.mic, |ui| {
                let text = if st.mic_muted { "Unmute mic" } else { "Mute mic" };
                if theme::quiet_button(ui, &p, text).clicked() {
                    app.ctl.send(Command::ToggleMicMute);
                }
            });
        });
    });
    ui.add_space(10.0);
}

fn recent(app: &mut App, ui: &mut egui::Ui) {
    let p = app.p;
    let items = app.library.snapshot();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Recent").font(theme::semibold(15.0)));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !items.is_empty() && theme::link_button(ui, &p, "Open library").clicked() {
                app.page = Page::Library;
                app.library.refresh(&app.s.output_dir());
            }
        });
    });
    ui.add_space(6.0);
    if items.is_empty() {
        theme::note(ui, &p, "Your recordings will show up here.");
        return;
    }
    for item in items.iter().take(4) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 62.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(8), p.panel);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let thumb_rect = egui::Rect::from_min_size(rect.left_top() + Vec2::new(6.0, 6.0), Vec2::new(89.0, 50.0));
        match item.thumb.as_ref().and_then(|t| app.thumb(ui.ctx(), t)) {
            Some(tex) => {
                ui.painter().image(
                    tex,
                    thumb_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                ui.painter().rect_filled(thumb_rect, CornerRadius::same(4), p.raised);
            }
        }
        let tx = thumb_rect.right() + 14.0;
        ui.painter().text(
            egui::pos2(tx, rect.top() + 20.0),
            egui::Align2::LEFT_CENTER,
            truncate(&item.name, 60),
            egui::FontId::proportional(14.0),
            p.text,
        );
        let mut meta = vec![urec_engine::video::format_bytes(item.size)];
        if let Some(m) = &item.meta {
            meta.insert(0, format_duration(m.duration));
            meta.push(format!("{}p", m.height));
        }
        if !item.app.is_empty() {
            meta.push(item.app.clone());
        }
        ui.painter().text(
            egui::pos2(tx, rect.top() + 42.0),
            egui::Align2::LEFT_CENTER,
            meta.join(", "),
            egui::FontId::proportional(12.0),
            p.muted,
        );
        if resp.clicked() {
            open_file(&item.path);
        }
        resp.on_hover_text("Play");
    }
}

pub fn open_file(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}
