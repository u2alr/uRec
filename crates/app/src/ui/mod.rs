//! Application window.

mod home;
mod library_page;
mod settings_page;
pub mod theme;

use crate::controller::{Command, Controller, Level, ReplayState, Status};
use crate::library::{Library, THUMB_H, THUMB_W};
use crate::settings::Settings;
use eframe::egui::{self, CornerRadius, RichText, Sense, Stroke, Vec2};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use theme::Palette;
use urec_win::{AudioDevice, MonitorInfo, WindowInfo};

pub const WINDOW_TITLE: &str = "uRec";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Home,
    Library,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    General,
    Recording,
    Video,
    Audio,
    Replay,
    Hotkeys,
    Storage,
    Appearance,
    Advanced,
}

impl Section {
    pub const ALL: [Section; 9] = [
        Section::General,
        Section::Recording,
        Section::Video,
        Section::Audio,
        Section::Replay,
        Section::Hotkeys,
        Section::Storage,
        Section::Appearance,
        Section::Advanced,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Recording => "Recording",
            Section::Video => "Video",
            Section::Audio => "Audio",
            Section::Replay => "Instant Replay",
            Section::Hotkeys => "Hotkeys",
            Section::Storage => "Storage",
            Section::Appearance => "Appearance",
            Section::Advanced => "Advanced",
        }
    }
}

pub struct App {
    pub ctl: Controller,
    pub s: Settings,
    pub p: Palette,
    pub page: Page,
    pub section: Section,
    pub library: Library,
    pub lib: library_page::LibraryUi,
    thumbs: HashMap<PathBuf, egui::TextureHandle>,
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<WindowInfo>,
    pub mics: Vec<AudioDevice>,
    pub speakers: Vec<AudioDevice>,
    devices_at: Instant,
    /// Which hotkey field is waiting for a key press.
    pub listening: Option<usize>,
    pub custom_replay: String,
    told_about_tray: bool,
    applied_theme: Option<(bool, u32)>,
    pub startup_message: Option<String>,
    exit_requested: bool,
    first_frame_logged: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        ctl: Controller,
        s: Settings,
        startup_message: Option<String>,
    ) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        ctl.set_repaint(move || ctx.request_repaint());
        let ctx2 = cc.egui_ctx.clone();
        let library = Library::new(
            urec_engine::process::locate_ffmpeg(),
            ctl.capturing.clone(),
            Arc::new(move || ctx2.request_repaint()),
        );
        library.refresh(&s.output_dir());
        let custom_replay = s.replay.seconds.to_string();
        let mut app = App {
            ctl,
            s,
            p: theme::DARK,
            page: Page::Home,
            section: Section::General,
            library,
            lib: library_page::LibraryUi::default(),
            thumbs: HashMap::new(),
            monitors: Vec::new(),
            windows: Vec::new(),
            mics: Vec::new(),
            speakers: Vec::new(),
            devices_at: Instant::now() - Duration::from_secs(3600),
            listening: None,
            custom_replay,
            told_about_tray: false,
            applied_theme: None,
            startup_message,
            exit_requested: false,
            first_frame_logged: false,
        };
        app.refresh_devices(true);
        app
    }

    /// Open a page by name ("home", "library", "settings:video").
    pub fn open_page(&mut self, spec: &str) {
        let (page, section) = spec.split_once(':').unwrap_or((spec, ""));
        self.page = match page {
            "library" => Page::Library,
            "settings" => Page::Settings,
            _ => Page::Home,
        };
        if let Some(s) =
            Section::ALL.into_iter().find(|s| s.label().to_lowercase().replace(' ', "") == section.replace(' ', ""))
        {
            self.section = s;
        } else if section == "replay" {
            self.section = Section::Replay;
        }
    }

    /// Re-read monitors/windows/audio devices (at most every few seconds).
    pub fn refresh_devices(&mut self, force: bool) {
        if !force && self.devices_at.elapsed() < Duration::from_secs(4) {
            return;
        }
        self.devices_at = Instant::now();
        self.monitors = urec_win::monitors();
        self.windows = urec_win::windows();
        self.mics = urec_win::audio_inputs();
        self.speakers = urec_win::audio_outputs();
    }

    /// Persist and hand new settings to the controller.
    pub fn commit(&mut self) {
        self.s.sanitize();
        if let Err(e) = self.s.save() {
            self.push_notice(Level::Error, format!("Settings couldn't be saved: {e}"));
        }
        self.ctl.send(Command::Apply(Box::new(self.s.clone())));
    }

    pub fn push_notice(&self, level: Level, text: String) {
        if let Ok(mut s) = self.ctl.status.lock() {
            s.notices.push_front(crate::controller::Notice { level, text, at: Instant::now(), file: None });
            s.notices.truncate(6);
        }
    }

    pub fn thumb(&mut self, ctx: &egui::Context, path: &PathBuf) -> Option<egui::TextureId> {
        if let Some(t) = self.thumbs.get(path) {
            return Some(t.id());
        }
        let bytes = std::fs::read(path).ok()?;
        if bytes.len() != (THUMB_W * THUMB_H * 4) as usize {
            return None;
        }
        let img = egui::ColorImage::from_rgba_unmultiplied([THUMB_W as usize, THUMB_H as usize], &bytes);
        let tex = ctx.load_texture(path.to_string_lossy(), img, egui::TextureOptions::LINEAR);
        let id = tex.id();
        self.thumbs.insert(path.clone(), tex);
        Some(id)
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let system_dark = ctx.input(|i| i.raw.system_theme).map(|t| t == egui::Theme::Dark).unwrap_or(true);
        let forced = std::env::var("UREC_THEME").ok();
        let dark = match forced.as_deref().unwrap_or(self.s.appearance.theme.as_str()) {
            "light" => false,
            "dark" => true,
            _ => system_dark,
        };
        let key = (dark, (self.s.appearance.ui_scale * 100.0) as u32);
        if self.applied_theme != Some(key) {
            self.p = if dark { theme::DARK } else { theme::LIGHT };
            theme::apply(ctx, &self.p, self.s.appearance.ui_scale);
            self.applied_theme = Some(key);
        }
    }

    fn handle_close(&mut self, ctx: &egui::Context, st: &Status) {
        if !ctx.input(|i| i.viewport().close_requested()) || self.exit_requested {
            return;
        }
        if self.s.general.minimize_to_tray && cfg!(windows) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            urec_win::hide_window_titled(WINDOW_TITLE);
            if !self.told_about_tray {
                self.told_about_tray = true;
                self.push_notice(
                    Level::Info,
                    "uRec is still running in the system tray. Use the tray icon to quit.".into(),
                );
            }
        } else if st.recording.is_some() || st.finishing {
            // Finish the file before closing.
            self.exit_requested = true;
            self.ctl.shutdown_and_wait(Duration::from_secs(60));
        } else {
            self.exit_requested = true;
            self.ctl.shutdown_and_wait(Duration::from_secs(15));
        }
    }

    fn tally(&self, ui: &mut egui::Ui, st: &Status) {
        let color = if st.recording.is_some() || st.finishing {
            Some(self.p.rec)
        } else if st.replay == ReplayState::On {
            Some(self.p.replay)
        } else {
            None
        };
        egui::Panel::top("tally")
            .exact_size(3.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(color.unwrap_or(self.p.bg)))
            .show_separator_line(false)
            .show(ui, |_| {});
    }

    fn nav(&mut self, ui: &mut egui::Ui, st: &Status) {
        let p = self.p;
        let ctx = ui.ctx().clone();
        egui::Panel::left("nav")
            .exact_size(184.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin { left: 16, right: 8, top: 18, bottom: 16 }))
            .show(ui, |ui| {
                ui.label(RichText::new("uRec").font(theme::semibold(18.0)).color(p.text));
                ui.add_space(22.0);
                for (page, label) in [(Page::Home, "Home"), (Page::Library, "Library"), (Page::Settings, "Settings")] {
                    let selected = self.page == page;
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
                    if resp.hovered() || selected {
                        ui.painter().rect_filled(
                            rect,
                            CornerRadius::same(6),
                            if selected { p.panel } else { p.panel.gamma_multiply(0.5) },
                        );
                    }
                    if selected {
                        let bar =
                            egui::Rect::from_min_size(rect.left_top() + Vec2::new(0.0, 9.0), Vec2::new(2.5, 16.0));
                        ui.painter().rect_filled(bar, CornerRadius::same(1), p.text);
                    }
                    ui.painter().text(
                        rect.left_center() + Vec2::new(14.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::proportional(14.5),
                        if selected { p.text } else { p.muted },
                    );
                    if resp.clicked() {
                        self.page = page;
                        if page == Page::Library {
                            self.library.refresh(&self.s.output_dir());
                        }
                    }
                    if resp.hovered() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    ui.add_space(2.0);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    if let Some(free) = st.disk_free.or_else(|| urec_win::disk_space(&self.s.output_dir()).map(|x| x.0))
                    {
                        let low = free < self.s.storage.low_space_gb as u64 * 1_000_000_000;
                        ui.label(
                            RichText::new(format!("{} free", urec_engine::video::format_bytes(free)))
                                .small()
                                .color(if low { p.warn } else { p.faint }),
                        );
                    }
                });
            });
    }

    fn notices(&mut self, ctx: &egui::Context, st: &Status) {
        if st.notices.is_empty() {
            return;
        }
        let p = self.p;
        let mut dismiss = None;
        let mut open = None;
        egui::Area::new(egui::Id::new("notices"))
            .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-18.0, -18.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                for (i, n) in st.notices.iter().enumerate().take(3) {
                    let ttl = match n.level {
                        Level::Error | Level::Warning => None,
                        _ => Some(Duration::from_secs(8)),
                    };
                    if ttl.is_some_and(|t| n.at.elapsed() > t) {
                        dismiss.get_or_insert(i);
                        continue;
                    }
                    let accent = match n.level {
                        Level::Error => p.rec,
                        Level::Warning => p.warn,
                        Level::Success => p.ok,
                        Level::Info => p.muted,
                    };
                    egui::Frame::new()
                        .fill(p.panel)
                        .stroke(Stroke::new(1.0, p.hairline))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.set_width(352.0);
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(Vec2::new(3.0, 34.0), Sense::hover());
                                ui.painter().rect_filled(r, CornerRadius::same(1), accent);
                                ui.vertical(|ui| {
                                    ui.add(egui::Label::new(RichText::new(&n.text).color(p.text)).wrap());
                                    ui.horizontal(|ui| {
                                        if let Some(f) = &n.file {
                                            if theme::link_button(ui, &p, "Show in folder").clicked() {
                                                open = Some(f.clone());
                                            }
                                        }
                                        if theme::link_button(ui, &p, "Dismiss").clicked() {
                                            dismiss = Some(i);
                                        }
                                    });
                                });
                            });
                        });
                    ui.add_space(6.0);
                }
            });
        if let Some(f) = open {
            urec_win::reveal_in_explorer(&f);
        }
        if let Some(i) = dismiss {
            self.ctl.dismiss_notice(i);
        }
        // Keep ticking only while a timed notice is visible.
        if st.notices.iter().any(|n| !matches!(n.level, Level::Error | Level::Warning)) {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.first_frame_logged {
            self.first_frame_logged = true;
            crate::controller::log("startup: first frame");
        }
        self.apply_theme(&ctx);
        let st = self.ctl.snapshot();
        self.handle_close(&ctx, &st);
        if let Some(msg) = self.startup_message.take() {
            self.push_notice(Level::Warning, msg);
        }
        self.tally(ui, &st);
        self.nav(ui, &st);
        let p = self.p;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin {
                left: 28,
                right: 28,
                top: 22,
                bottom: 18,
            }))
            .show(ui, |ui| match self.page {
                Page::Home => home::show(self, ui, &st),
                Page::Library => library_page::show(self, ui, &st),
                Page::Settings => settings_page::show(self, ui, &st),
            });
        self.notices(&ctx, &st);

        // Idle: no repaints at all. Active: refresh the timer and meters.
        let every = Duration::from_millis(if self.s.general.performance_mode { 1000 } else { 250 });
        let active = st.recording.is_some() || st.finishing || st.starting || st.probing || !st.ready;
        if active || (st.replay == ReplayState::On && self.page == Page::Home) {
            ctx.request_repaint_after(every);
        }
        if self.exit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.ctl.shutdown_and_wait(Duration::from_secs(60));
    }
}
