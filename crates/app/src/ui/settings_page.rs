//! Settings: General, Recording, Video, Audio, Instant Replay, Hotkeys, Storage, Appearance, Advanced.

use super::home::preset_name;
use super::theme;
use super::{App, Section};
use crate::controller::{format_secs, preset_id, Command, Level, Status};
use crate::settings::{Settings, REPLAY_CHOICES, REPLAY_MAX};
use eframe::egui::{self, Align, CornerRadius, Layout, RichText, Sense, Vec2};
use urec_engine::encoder::{Backend, Encoder};
use urec_engine::hotkey;
use urec_engine::replay;
use urec_engine::video::{self, Capabilities, Codec, Preset};

pub fn show(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    ui.horizontal_top(|ui| {
        // Section list
        ui.allocate_ui_with_layout(Vec2::new(170.0, ui.available_height()), Layout::top_down(Align::Min), |ui| {
            ui.set_width(170.0);
            ui.label(RichText::new("Settings").font(theme::semibold(22.0)));
            ui.add_space(14.0);
            for s in Section::ALL {
                let selected = app.section == s;
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(160.0, 30.0), Sense::click());
                if selected || resp.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        CornerRadius::same(6),
                        if selected { p.panel } else { p.panel.gamma_multiply(0.5) },
                    );
                }
                ui.painter().text(
                    rect.left_center() + Vec2::new(10.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    s.label(),
                    egui::FontId::proportional(14.0),
                    if selected { p.text } else { p.muted },
                );
                if resp.clicked() {
                    app.section = s;
                    app.listening = None;
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        });
        ui.add_space(24.0);
        ui.vertical(|ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width((ui.available_width() - 16.0).min(640.0));
                let before = app.s.clone();
                match app.section {
                    Section::General => general(app, ui, st),
                    Section::Recording => recording(app, ui, st),
                    Section::Video => video_page(app, ui, st),
                    Section::Audio => audio(app, ui),
                    Section::Replay => replay_page(app, ui),
                    Section::Hotkeys => hotkeys(app, ui, st),
                    Section::Storage => storage(app, ui, st),
                    Section::Appearance => appearance(app, ui),
                    Section::Advanced => advanced(app, ui, st),
                }
                if app.s != before {
                    app.commit();
                }
                ui.add_space(30.0);
            });
        });
    });
}

fn combo<T: PartialEq + Clone>(ui: &mut egui::Ui, id: &str, value: &mut T, options: &[(T, String)], width: f32) {
    let current = options.iter().find(|(v, _)| v == value).map(|(_, l)| l.clone()).unwrap_or_default();
    // Long names (audio devices, displays) are shortened to fit the box; the full name is on hover.
    let fit = ((width - 34.0) / 7.2).max(8.0) as usize;
    let r = egui::ComboBox::from_id_salt(id).selected_text(ellipsize(&current, fit)).width(width).show_ui(ui, |ui| {
        ui.set_max_width(width.max(420.0));
        for (v, l) in options {
            ui.selectable_value(value, v.clone(), ellipsize(l, 60)).on_hover_text(l);
        }
    });
    if current.chars().count() > fit {
        r.response.on_hover_text(current);
    }
}

/// Shorten long text with an ellipsis (by characters, so it never splits a code point).
pub(super) fn ellipsize(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut t: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    t = t.trim_end().to_string();
    t.push('…');
    t
}

fn opts(xs: &[(&str, &str)]) -> Vec<(String, String)> {
    xs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

// ------------------------------------------------------------------ General

fn general(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    theme::page_title(ui, &p, "General", "");
    let mut startup = urec_win::startup_enabled("uRec");
    theme::row(ui, &p, "Start with Windows", "Opens in the tray when you sign in. Off by default.", |ui| {
        if theme::switch(ui, &p, &mut startup).changed() {
            let cmd = std::env::current_exe().ok().map(|e| format!("\"{}\" --minimized", e.display()));
            if !urec_win::set_startup("uRec", if startup { cmd.as_deref() } else { None }) {
                app.push_notice(Level::Error, "Couldn't change the startup setting.".into());
            }
        }
    });
    let s = &mut app.s.general;
    theme::row(ui, &p, "Keep running in the tray when closed", "Hotkeys and Instant Replay keep working.", |ui| {
        theme::switch(ui, &p, &mut s.minimize_to_tray);
    });
    theme::row(ui, &p, "Play a sound when a clip is saved", "", |ui| {
        theme::switch(ui, &p, &mut s.sound_on_save);
    });
    theme::row(
        ui,
        &p,
        "Performance mode",
        "For games: updates the window less often, keeps frames on the graphics card and warns when recording falls behind.",
        |ui| {
            theme::switch(ui, &p, &mut s.performance_mode);
        },
    );
    theme::row(
        ui,
        &p,
        "Lower Instant Replay quality automatically",
        "If your PC can't keep up, Instant Replay drops to 30 FPS and tells you. Recordings are never changed while running.",
        |ui| {
            theme::switch(ui, &p, &mut s.auto_adjust);
        },
    );
    updates(app, ui, st);
}

fn updates(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    use crate::update::State;
    let p = app.p;
    theme::group_title(ui, &p, "Updates");
    let state = app.updater.state();
    let installed = crate::update::is_installed();
    let current = crate::update::current_version();
    let (status, help) = match &state {
        State::Idle => (format!("uRec {current}"), String::new()),
        State::Checking => (format!("uRec {current}"), "Checking for updates…".into()),
        State::UpToDate => (format!("uRec {current}"), "You have the latest version.".into()),
        State::Available { version, .. } if !installed => (
            format!("uRec {version} is available"),
            "This is a portable copy, so it isn't updated automatically.".into(),
        ),
        State::Available { version, .. } => (format!("uRec {version} is available"), String::new()),
        State::Downloading { version, percent } => (
            format!("Downloading uRec {version}"),
            percent.map(|x| format!("{x}%")).unwrap_or_else(|| "Starting…".into()),
        ),
        State::Ready { version } => (
            format!("uRec {version} is ready to install"),
            "It installs when you quit uRec, or the next time it starts.".into(),
        ),
        State::InstallFailed { version } => {
            (format!("uRec {version} didn't install"), "Download the installer from GitHub to update by hand.".into())
        }
        State::Failed(e) => (format!("uRec {current}"), format!("Couldn't check: {e}")),
    };
    theme::row(ui, &p, &status, &help, |ui| match &state {
        State::Ready { .. } => {
            let busy = st.busy();
            let r = ui.add_enabled_ui(!busy, |ui| theme::primary_button(ui, &p, "Restart now")).inner;
            let r = r.on_disabled_hover_text("Stop recording and Instant Replay first.");
            if r.clicked() {
                app.restart_to_update();
            }
        }
        State::Available { .. } | State::InstallFailed { .. } => {
            let url = match &state {
                State::Available { url, .. } => url.as_str(),
                _ => crate::update::RELEASES_PAGE,
            };
            if theme::quiet_button(ui, &p, "Open download page").clicked() {
                urec_win::open_url(url);
            }
        }
        State::Checking | State::Downloading { .. } => {
            ui.add(egui::Spinner::new().size(16.0).color(p.muted));
        }
        _ => {
            if theme::quiet_button(ui, &p, "Check now").clicked() {
                app.updater.check_now();
            }
        }
    });
    let s = &mut app.s.general;
    theme::row(ui, &p, "Check for updates", "Asks GitHub once a day. Nothing else is sent.", |ui| {
        theme::switch(ui, &p, &mut s.check_updates);
    });
    if installed {
        theme::row(
            ui,
            &p,
            "Install updates automatically",
            "Downloads in the background, checks the file's SHA-256 and installs when you quit. Never during a recording.",
            |ui| {
                theme::switch(ui, &p, &mut s.auto_update);
            },
        );
    }
}

// ------------------------------------------------------------------ Recording

fn recording(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    app.refresh_devices(false);
    theme::page_title(ui, &p, "Recording", "What gets captured and how.");
    let monitors = app.monitors.clone();
    let windows = app.windows.clone();
    let c = &mut app.s.capture;
    theme::row(ui, &p, "Source", "", |ui| {
        combo(
            ui,
            "kind",
            &mut c.kind,
            &opts(&[("monitor", "A display"), ("window", "One window"), ("region", "Part of a display")]),
            200.0,
        );
    });
    if c.kind != "window" {
        theme::row(ui, &p, "Display", "", |ui| {
            let mut o: Vec<(String, String)> = vec![(String::new(), "Main display".into())];
            o.extend(monitors.iter().map(|m| (m.device.clone(), m.name.clone())));
            combo(ui, "monitor", &mut c.monitor, &o, 260.0);
        });
    } else {
        theme::row(ui, &p, "Window", "Captures only this window, even when other windows are on top of it.", |ui| {
            let current =
                if c.window_title.is_empty() { "Choose a window".to_string() } else { ellipsize(&c.window_title, 32) };
            let r = egui::ComboBox::from_id_salt("window").selected_text(current).width(260.0).show_ui(ui, |ui| {
                ui.set_max_width(420.0);
                for w in &windows {
                    let label = format!("{} — {}", ellipsize(&w.title, 48), w.app_name());
                    if ui.selectable_label(c.window_title == w.title, label).on_hover_text(&w.title).clicked() {
                        c.window_title = w.title.clone();
                        c.window_exe = w.exe.clone();
                    }
                }
            });
            if !c.window_title.is_empty() {
                r.response.on_hover_text(&c.window_title);
            }
        });
    }
    if c.kind == "region" {
        theme::row(ui, &p, "Area", "Position and size in pixels, from the display's top-left corner.", |ui| {
            for (i, label) in [(3, "H"), (2, "W"), (1, "Y"), (0, "X")] {
                ui.add(egui::DragValue::new(&mut c.region[i]).range(0..=16384).speed(4.0));
                ui.label(RichText::new(label).color(p.muted));
            }
        });
    }
    theme::row(ui, &p, "Record the mouse cursor", "", |ui| {
        theme::switch(ui, &p, &mut c.cursor);
    });
    let wgc = urec_win::supports_wgc();
    let method_help = if wgc {
        "Automatic tries Windows Graphics Capture, then Desktop Duplication, then the compatibility method."
    } else {
        "This version of Windows doesn't have Windows Graphics Capture (needs Windows 10 version 1903)."
    };
    theme::row(ui, &p, "Capture method", method_help, |ui| {
        let mut o = opts(&[("auto", "Automatic"), ("dda", "Desktop Duplication"), ("gdi", "Compatibility (slower)")]);
        if wgc {
            o.insert(1, ("wgc".into(), "Windows Graphics Capture".into()));
        }
        combo(ui, "method", &mut c.method, &o, 220.0);
    });
    if let Some(cap) = &st.capture {
        ui.add_space(8.0);
        theme::note(
            ui,
            &p,
            &format!(
                "Now capturing with {}{}.",
                cap.method.label(),
                if cap.gpu_path {
                    ", frames stay on the graphics card"
                } else {
                    ", frames are copied through system memory"
                }
            ),
        );
        for f in &cap.fallbacks {
            theme::note(ui, &p, &format!("Skipped {f}"));
        }
    }
    ui.add_space(10.0);
    theme::note(
        ui,
        &p,
        "Games in exclusive fullscreen may record as black with some methods. Borderless or windowed fullscreen works best. Protected video (some streaming apps) can't be recorded.",
    );
}

// ------------------------------------------------------------------ Video

pub(super) fn caps(app: &App, st: &Status) -> Capabilities {
    let m = app.monitors.iter().find(|m| m.primary);
    Capabilities {
        hardware_encoder: st.best_encoder(Codec::H264).is_some_and(|e| e.backend.is_hardware()),
        hevc: st.best_encoder(Codec::Hevc).is_some(),
        av1: st.best_encoder(Codec::Av1).is_some(),
        source_width: m.map(|m| m.bounds.width).unwrap_or(1920),
        source_height: m.map(|m| m.bounds.height).unwrap_or(1080),
        source_refresh: m.and_then(|m| m.refresh_hz),
        logical_cpus: urec_engine::probe::logical_cpus(),
    }
}

pub(super) fn recommended(caps: &Capabilities) -> Preset {
    if caps.hardware_encoder {
        Preset::High
    } else if caps.logical_cpus >= 8 {
        Preset::Balanced
    } else {
        Preset::LowEnd
    }
}

fn video_page(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    theme::page_title(ui, &p, "Video", "Quality, size and the encoder used.");
    let caps = caps(app, st);
    let rec = recommended(&caps);

    // Presets
    ui.add_space(10.0);
    ui.label("Quality preset");
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for preset in
            [Preset::LowEnd, Preset::Balanced, Preset::High, Preset::VeryHigh, Preset::Compact, Preset::Custom]
        {
            let id = preset_id(preset);
            let selected = app.s.video.preset == id;
            let label =
                if preset == rec { format!("{} (recommended)", preset.label()) } else { preset.label().to_string() };
            if ui.selectable_label(selected, label).clicked() && !selected {
                if preset != Preset::Custom {
                    let v = video::apply_preset(preset, &caps);
                    let codec = app.s.video.codec.clone();
                    app.s.set_video(&v);
                    // Keep the chosen codec, except for "Smaller files", which picks its own.
                    if preset != Preset::Compact {
                        app.s.video.codec = codec;
                    }
                    app.s.video.preset = id.into();
                    app.s.video.bitrate_kbps = scale_bitrate(&app.s, &caps);
                }
                app.s.video.preset = id.into();
            }
        }
    });
    ui.add_space(4.0);
    let preset_note = match app.s.video.preset.as_str() {
        "low" => "720p at 30 FPS with light encoding. Best for older PCs.",
        "balanced" => "1080p with moderate bitrate. Good for everyday recording.",
        "high" => "1080p at 60 FPS with a higher bitrate.",
        "very_high" => "1440p or 4K with a high bitrate. Needs a hardware encoder and lots of disk space.",
        "compact" => "1080p, constant quality, HEVC when this PC can encode it. Much smaller files for the same look; HEVC needs Windows' HEVC extension to play.",
        _ => "Your own settings below.",
    };
    theme::note(ui, &p, preset_note);
    if app.s.video.preset != preset_id(rec)
        && theme::link_button(ui, &p, &format!("Use recommended ({})", preset_name(preset_id(rec)))).clicked()
    {
        let v = video::apply_preset(rec, &caps);
        app.s.set_video(&v);
        app.s.video.preset = preset_id(rec).into();
        app.s.video.encoder = "auto".into();
    }
    ui.add_space(8.0);
    theme::hairline(ui, &p);

    let before = app.s.video.clone();
    let src = (caps.source_width, caps.source_height);
    theme::row(ui, &p, "Resolution", &resolution_help(&app.s, src), |ui| {
        let mut choice: u32 = if app.s.video.custom { 1 } else { app.s.video.height };
        let o: Vec<(u32, String)> = vec![
            (0, "Native".into()),
            (2160, "3840 × 2160 (4K)".into()),
            (1440, "2560 × 1440".into()),
            (1080, "1920 × 1080".into()),
            (900, "1600 × 900".into()),
            (720, "1280 × 720".into()),
            (1, "Custom".into()),
        ];
        combo(ui, "res", &mut choice, &o, 200.0);
        app.s.video.custom = choice == 1;
        if choice != 1 {
            app.s.video.height = choice;
        }
    });
    if app.s.video.custom {
        theme::row(ui, &p, "Custom size", "Width and height in pixels. Odd values are rounded down to even.", |ui| {
            ui.add(egui::DragValue::new(&mut app.s.video.custom_h).range(128..=4320).speed(2.0));
            ui.label(RichText::new("×").color(p.muted));
            ui.add(egui::DragValue::new(&mut app.s.video.custom_w).range(128..=7680).speed(2.0));
        });
    }
    let refresh = caps.source_refresh;
    let fps_help = match refresh {
        Some(r) if app.s.video.fps > r => format!("Your display runs at {r} Hz, so frames above that are repeats."),
        _ => "Higher frame rates need more processing and disk space.".to_string(),
    };
    theme::row(ui, &p, "Frame rate", &fps_help, |ui| {
        let custom = !video::FrameRate::CHOICES.contains(&app.s.video.fps) && app.s.video.fps != 0;
        let mut choice = if custom { 1 } else { app.s.video.fps };
        let mut o: Vec<(u32, String)> =
            vec![(0, format!("Match display{}", refresh.map(|r| format!(" ({r})")).unwrap_or_default()))];
        o.extend(video::FrameRate::CHOICES.iter().map(|f| (*f, format!("{f} FPS"))));
        o.push((1, "Custom".into()));
        combo(ui, "fps", &mut choice, &o, 200.0);
        if choice == 1 {
            if !custom {
                app.s.video.fps = 50;
            }
            ui.add(egui::DragValue::new(&mut app.s.video.fps).range(1..=240).suffix(" FPS"));
        } else {
            app.s.video.fps = choice;
        }
    });
    let bps = video::bytes_per_second(app.s.video.bitrate_kbps, app.s.audio.bitrate_kbps);
    let cq = app.s.video.rate_control == "quality";
    if cq {
        theme::row(
            ui,
            &p,
            "Quality level",
            "Higher looks better and takes more space. Still or simple scenes stay small either way.",
            |ui| {
                ui.add(egui::Slider::new(&mut app.s.video.quality, 1..=10));
            },
        );
    }
    theme::row(
        ui,
        &p,
        if cq { "Maximum bitrate" } else { "Bitrate" },
        &if cq {
            format!("Busy scenes never use more than this: at most {} per minute.", video::format_bytes(bps * 60))
        } else {
            format!("About {} per minute, {} per hour.", video::format_bytes(bps * 60), video::format_bytes(bps * 3600))
        },
        |ui| {
            let mut mbps = app.s.video.bitrate_kbps as f32 / 1000.0;
            if ui
                .add(egui::DragValue::new(&mut mbps).range(0.5..=200.0).speed(0.5).suffix(" Mbps").max_decimals(1))
                .changed()
            {
                app.s.video.bitrate_kbps = (mbps * 1000.0).round() as u32;
            }
        },
    );

    // Codec: only offer what works here.
    let codec_opts: Vec<(String, String)> = Codec::ALL
        .iter()
        .map(|c| {
            let ok = st.best_encoder(*c).is_some() || st.probe.is_empty();
            let id = match c {
                Codec::H264 => "h264",
                Codec::Hevc => "hevc",
                Codec::Av1 => "av1",
            };
            (id.to_string(), if ok { c.label().to_string() } else { format!("{} (not available)", c.label()) })
        })
        .collect();
    theme::row(
        ui,
        &p,
        "Codec",
        "H.264 plays everywhere. HEVC and AV1 give smaller files but need newer players and editors.",
        |ui| {
            combo(ui, "codec", &mut app.s.video.codec, &codec_opts, 200.0);
        },
    );
    let codec = app.s.codec();
    if st.best_encoder(codec).is_none() && !st.probe.is_empty() {
        ui.colored_label(
            p.warn,
            format!("No encoder for {codec} works on this PC. Recording will fail until you choose another codec."),
        );
    }

    // Encoder
    let best = st.best_encoder(codec);
    let mut enc_opts: Vec<(String, String)> = vec![(
        "auto".into(),
        format!("Automatic{}", best.map(|e| format!(" ({})", e.backend.label())).unwrap_or_default()),
    )];
    for b in Backend::PREFERENCE {
        let e = Encoder::new(b, codec);
        if e.ffmpeg_name().is_none() {
            continue;
        }
        let r = st.probe.iter().find(|r| r.encoder == e);
        let label = match r {
            Some(r) if r.ok => b.label().to_string(),
            Some(_) => format!("{} (not available)", b.label()),
            None => b.label().to_string(),
        };
        enc_opts.push((b.id().to_string(), label));
    }
    let enc_help =
        match st.probe.iter().find(|r| r.encoder.codec == codec && r.encoder.backend.id() == app.s.video.encoder) {
            Some(r) if !r.ok => r.reason.clone(),
            _ if app.s.video.encoder == "software" || app.s.video.encoder == "mf" => {
                "Encoding on the processor uses more CPU and can lower game performance.".into()
            }
            _ => "Automatic uses the graphics card's encoder when one works.".into(),
        };
    theme::row(ui, &p, "Encoder", &enc_help, |ui| {
        combo(ui, "encoder", &mut app.s.video.encoder, &enc_opts, 240.0);
    });
    theme::row(
        ui,
        &p,
        "Rate control",
        "Constant quality gives the smallest files for how it looks. Variable aims at the bitrate. Constant keeps it steady (for streaming-like files).",
        |ui| {
            combo(
                ui,
                "rc",
                &mut app.s.video.rate_control,
                &opts(&[("quality", "Constant quality"), ("vbr", "Variable (VBR)"), ("cbr", "Constant (CBR)")]),
                200.0,
            );
        },
    );
    theme::row(ui, &p, "Encoder preset", "Faster presets use less processing power at slightly lower quality.", |ui| {
        combo(
            ui,
            "effort",
            &mut app.s.video.effort,
            &opts(&[("speed", "Faster"), ("balanced", "Balanced"), ("quality", "Better quality")]),
            200.0,
        );
    });
    if app.s.video != before {
        app.s.video.preset = "custom".into();
    }
    if !caps.hardware_encoder && !st.probe.is_empty() {
        let (w, h) = video::output_size(app.s.resolution(), src.0, src.1);
        let heavy = w as u64 * h as u64 * app.s.video.fps.max(30) as u64 > 1920 * 1080 * 30;
        if heavy {
            ui.add_space(8.0);
            ui.colored_label(p.warn, "No hardware encoder was found, so this will encode on your processor. Above 1080p30 that can slow games down.");
        }
    }
}

fn scale_bitrate(s: &Settings, caps: &Capabilities) -> u32 {
    let (w, h) = video::output_size(s.resolution(), caps.source_width, caps.source_height);
    let fps = s.video_settings().frame_rate.resolve(caps.source_refresh);
    let factor = match s.video.preset.as_str() {
        "low" => 0.55,
        "balanced" => 0.8,
        "very_high" => 1.5,
        _ => 1.0,
    };
    video::recommended_bitrate_kbps(w, h, fps, s.codec(), factor)
}

fn resolution_help(s: &Settings, src: (u32, u32)) -> String {
    let (w, h) = video::output_size(s.resolution(), src.0, src.1);
    if (w, h) == (src.0 & !1, src.1 & !1) {
        format!("Records at {w} × {h} without scaling.")
    } else {
        format!("Your display is {} × {}; recordings are scaled to {w} × {h}.", src.0, src.1)
    }
}

// ------------------------------------------------------------------ Audio

fn audio(app: &mut App, ui: &mut egui::Ui) {
    let p = app.p;
    app.refresh_devices(false);
    theme::page_title(ui, &p, "Audio", "");
    let speakers = app.speakers.clone();
    let mics = app.mics.clone();
    let a = &mut app.s.audio;
    theme::group_title(ui, &p, "Game and desktop sound");
    theme::row(ui, &p, "Record game and desktop sound", "Everything you hear from the selected output device.", |ui| {
        theme::switch(ui, &p, &mut a.system);
    });
    ui.add_enabled_ui(a.system, |ui| {
        theme::row(ui, &p, "Output device", "", |ui| {
            let mut o = vec![(String::new(), "Windows default".to_string())];
            o.extend(speakers.iter().map(|d| (d.id.clone(), d.name.clone())));
            combo(ui, "spk", &mut a.system_device, &o, 260.0);
        });
        theme::row(ui, &p, "Volume", "", |ui| {
            ui.add(
                egui::Slider::new(&mut a.system_volume, 0.0..=2.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
            );
        });
    });
    theme::group_title(ui, &p, "Microphone");
    theme::row(ui, &p, "Record microphone", "", |ui| {
        theme::switch(ui, &p, &mut a.mic);
    });
    ui.add_enabled_ui(a.mic, |ui| {
        theme::row(ui, &p, "Microphone", if mics.is_empty() { "No microphone found." } else { "" }, |ui| {
            let mut o = vec![(String::new(), "Windows default".to_string())];
            o.extend(mics.iter().map(|d| (d.id.clone(), d.name.clone())));
            combo(ui, "mic", &mut a.mic_device, &o, 260.0);
        });
        theme::row(ui, &p, "Volume", "", |ui| {
            ui.add(
                egui::Slider::new(&mut a.mic_volume, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
            );
        });
        let help = if a.mic_mode == "push_to_talk" {
            "Silent until you hold the push-to-talk key (Settings › Hotkeys). The key still works in your game."
        } else {
            "Always recorded. The mute hotkey turns it off and on."
        };
        theme::row(ui, &p, "Microphone mode", help, |ui| {
            combo(
                ui,
                "mic_mode",
                &mut a.mic_mode,
                &opts(&[("open", "Always on"), ("push_to_talk", "Push to talk")]),
                180.0,
            );
        });
    });
    theme::group_title(ui, &p, "Output");
    theme::row(
        ui,
        &p,
        "Audio tracks",
        if a.layout == "separate" {
            "Track 1 is game and desktop sound, track 2 is the microphone. Editors can adjust them separately; many players only play track 1."
        } else {
            "Game sound and microphone mixed into one track that plays everywhere."
        },
        |ui| {
            combo(
                ui,
                "layout",
                &mut a.layout,
                &opts(&[("mixed", "One mixed track"), ("separate", "Separate tracks")]),
                200.0,
            );
        },
    );
    theme::row(ui, &p, "Audio quality", "AAC. 160 kbps is plenty for most recordings.", |ui| {
        let o: Vec<(u32, String)> = [128, 160, 192, 256, 320].iter().map(|k| (*k, format!("{k} kbps"))).collect();
        combo(ui, "abr", &mut a.bitrate_kbps, &o, 200.0);
    });
}

// ------------------------------------------------------------------ Instant Replay

fn replay_page(app: &mut App, ui: &mut egui::Ui) {
    let p = app.p;
    theme::page_title(
        ui,
        &p,
        "Instant Replay",
        "Records in the background so you can save what just happened. Uses the same video settings as recordings.",
    );
    let mut on = app.s.replay.enabled;
    theme::row(ui, &p, "Instant Replay", "Off by default. While on, it continuously captures and encodes.", |ui| {
        if theme::switch(ui, &p, &mut on).changed() {
            app.s.replay.enabled = on;
        }
    });
    let mut secs = app.s.replay.seconds;
    let preset = REPLAY_CHOICES.contains(&secs);
    theme::row(
        ui,
        &p,
        "Replay length",
        &format!("Custom lengths can be 5 seconds to {} minutes.", REPLAY_MAX / 60),
        |ui| {
            let mut choice = if preset { secs } else { 0 };
            let mut o: Vec<(u32, String)> = REPLAY_CHOICES.iter().map(|s| (*s, format_secs(*s))).collect();
            o.push((0, "Custom".into()));
            combo(ui, "rlen", &mut choice, &o, 140.0);
            if choice == 0 {
                let r = ui.add(egui::TextEdit::singleline(&mut app.custom_replay).desired_width(56.0));
                ui.label(RichText::new("seconds").color(p.muted));
                if r.lost_focus() || r.changed() {
                    if let Ok(v) = app.custom_replay.trim().parse::<u32>() {
                        secs = v.clamp(5, REPLAY_MAX);
                    }
                }
                if preset {
                    secs = 600;
                    app.custom_replay = "600".into();
                }
            } else {
                secs = choice;
            }
        },
    );
    app.s.replay.seconds = secs;
    let bps = video::bytes_per_second(app.s.video.bitrate_kbps, app.s.audio.bitrate_kbps);
    let ring = replay::ring_bytes(secs, 2, bps);
    theme::row(
        ui,
        &p,
        "Buffer",
        &format!(
            "Stored on disk, not in memory: about {} at the current bitrate, rewritten continuously.",
            video::format_bytes(ring)
        ),
        |ui| {
            if theme::quiet_button(ui, &p, "Change").clicked() {
                if let Some(d) = urec_win::pick_folder("Instant Replay buffer folder", Some(&app.s.replay_dir())) {
                    app.s.replay.buffer_dir = d.to_string_lossy().into();
                }
            }
            if !app.s.replay.buffer_dir.is_empty() && theme::link_button(ui, &p, "Use default").clicked() {
                app.s.replay.buffer_dir.clear();
            }
        },
    );
    theme::note(ui, &p, &format!("Buffer folder: {}", app.s.replay_dir().display()));
    ui.add_space(8.0);
    theme::note(
        ui,
        &p,
        "Saved replays may be up to 2 seconds longer than the chosen length, because the buffer is kept in 2-second pieces. Starting a recording while Instant Replay is on records from the same buffer, so nothing is encoded twice.",
    );
}

// ------------------------------------------------------------------ Hotkeys

const ACTIONS: [&str; 6] = [
    "Start or stop recording",
    "Save Instant Replay",
    "Turn Instant Replay on or off",
    "Mute or unmute microphone",
    "Show or hide the overlay",
    "Push to talk (hold)",
];
const PUSH_TO_TALK: usize = 5;

fn hotkey_field(s: &mut Settings, i: usize) -> &mut String {
    match i {
        0 => &mut s.hotkeys.record,
        1 => &mut s.hotkeys.save_replay,
        2 => &mut s.hotkeys.toggle_replay,
        3 => &mut s.hotkeys.mute_mic,
        4 => &mut s.hotkeys.overlay,
        _ => &mut s.hotkeys.push_to_talk,
    }
}

fn key_name(k: egui::Key) -> Option<String> {
    use egui::Key::*;
    let s = match k {
        Num0 | Num1 | Num2 | Num3 | Num4 | Num5 | Num6 | Num7 | Num8 | Num9 => {
            k.name().trim_start_matches("Num").to_string()
        }
        Insert => "Insert".into(),
        Delete => "Delete".into(),
        Home => "Home".into(),
        End => "End".into(),
        PageUp => "PgUp".into(),
        PageDown => "PgDown".into(),
        Space => "Space".into(),
        ArrowUp => "Up".into(),
        ArrowDown => "Down".into(),
        ArrowLeft => "Left".into(),
        ArrowRight => "Right".into(),
        Backtick => "`".into(),
        _ => {
            let n = k.name();
            if (n.len() == 1 && n.chars().all(|c| c.is_ascii_alphanumeric()))
                || (n.starts_with('F') && n[1..].parse::<u32>().is_ok_and(|v| (1..=24).contains(&v)))
            {
                n.to_string()
            } else {
                return None;
            }
        }
    };
    Some(s)
}

fn hotkeys(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    theme::page_title(
        ui,
        &p,
        "Hotkeys",
        "Work everywhere, including while a game is open. Click a shortcut, then press the new keys.",
    );

    // Capture a new combination.
    if let Some(i) = app.listening {
        let events = ui.input(|inp| inp.events.clone());
        for e in events {
            // Side mouse buttons work for push-to-talk only.
            if let egui::Event::PointerButton { button, pressed: true, .. } = e {
                let name = match button {
                    egui::PointerButton::Extra1 => Some("Mouse4"),
                    egui::PointerButton::Extra2 => Some("Mouse5"),
                    _ => None,
                };
                if let (Some(n), true) = (name, i == PUSH_TO_TALK) {
                    *hotkey_field(&mut app.s, i) = n.into();
                    app.listening = None;
                }
            }
            if let egui::Event::Key { key, pressed: true, modifiers, .. } = e {
                if key == egui::Key::Escape {
                    app.listening = None;
                } else if (key == egui::Key::Backspace || key == egui::Key::Delete) && !modifiers.any() {
                    hotkey_field(&mut app.s, i).clear();
                    app.listening = None;
                } else if let Some(name) = key_name(key) {
                    let mut parts = Vec::new();
                    if modifiers.ctrl {
                        parts.push("Ctrl");
                    }
                    if modifiers.alt {
                        parts.push("Alt");
                    }
                    if modifiers.shift {
                        parts.push("Shift");
                    }
                    let text = format!("{}{}{}", parts.join("+"), if parts.is_empty() { "" } else { "+" }, name);
                    // A plain letter or digit as a global hotkey would stop working everywhere
                    // else; push-to-talk only watches its key, so it may be a plain key.
                    let plain = parts.is_empty() && name.chars().count() == 1;
                    if plain && i != PUSH_TO_TALK {
                        app.push_notice(
                            Level::Warning,
                            format!("Add Ctrl, Alt or Shift: on its own, {name} would stop typing in every other app."),
                        );
                    } else if hotkey::Hotkey::parse(&text).is_some() {
                        *hotkey_field(&mut app.s, i) = text;
                    }
                    app.listening = None;
                }
            }
        }
    }

    let bindings: Vec<(&str, String)> =
        (0..ACTIONS.len()).map(|i| (ACTIONS[i], hotkey_field(&mut app.s, i).clone())).collect();
    let pairs: Vec<(&str, &str)> = bindings.iter().map(|(a, k)| (*a, k.as_str())).collect();
    let conflicts = hotkey::conflicts(&pairs);
    for (i, action) in ACTIONS.iter().enumerate() {
        let action = *action;
        let value = hotkey_field(&mut app.s, i).clone();
        let mut help = String::new();
        if let Some((a, b)) = conflicts.iter().find(|(a, b)| *a == action || *b == action) {
            let other = if *a == action { b } else { a };
            help = format!("Also used for \"{other}\". Pick a different shortcut.");
        } else if st.hotkey_failures.iter().any(|f| f.starts_with(short_name(i))) {
            help = "Another app is already using this shortcut.".into();
        } else if i == PUSH_TO_TALK {
            help = if app.s.audio.mic && app.s.audio.mic_mode == "push_to_talk" {
                "Hold to talk. The key keeps working in your game. A side mouse button works too.".into()
            } else {
                "Used when the microphone is set to push to talk (Settings › Audio).".into()
            };
        }
        theme::row(ui, &p, action, &help, |ui| {
            let listening = app.listening == Some(i);
            let label = if listening {
                "Press keys, Esc to cancel".to_string()
            } else if value.is_empty() {
                "Not set".to_string()
            } else {
                value.replace('+', " + ")
            };
            let b = egui::Button::new(RichText::new(label).color(if listening { p.muted } else { p.text }))
                .min_size(Vec2::new(190.0, 30.0));
            if ui.add(b).clicked() {
                app.listening = Some(i);
            }
            if !value.is_empty() && !listening && theme::link_button(ui, &p, "Clear").clicked() {
                hotkey_field(&mut app.s, i).clear();
            }
        });
    }
    ui.add_space(8.0);
    theme::note(ui, &p, "Backspace while choosing removes a shortcut.");
    if theme::link_button(ui, &p, "Restore default shortcuts").clicked() {
        app.s.hotkeys = crate::settings::Hotkeys::default();
    }
}

fn short_name(i: usize) -> &'static str {
    [
        "Start/stop recording",
        "Save Instant Replay",
        "Turn Instant Replay on/off",
        "Mute microphone",
        "Show/hide overlay",
        "Push to talk",
    ][i]
}

// ------------------------------------------------------------------ Storage

fn storage(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    theme::page_title(ui, &p, "Storage", "Recordings stay in this folder when uRec is uninstalled.");
    let free = st.disk_free.or_else(|| urec_win::disk_space(&app.s.output_dir()).map(|x| x.0));
    theme::row(
        ui,
        &p,
        "Recordings folder",
        &format!(
            "{}{}",
            app.s.storage.output_dir,
            free.map(|f| format!(". {} free on this drive.", video::format_bytes(f))).unwrap_or_default()
        ),
        |ui| {
            if theme::quiet_button(ui, &p, "Open").clicked() {
                let _ = std::fs::create_dir_all(app.s.output_dir());
                urec_win::reveal_in_explorer(&app.s.output_dir());
            }
            if theme::quiet_button(ui, &p, "Change").clicked() {
                if let Some(d) = urec_win::pick_folder("Folder for recordings", Some(&app.s.output_dir())) {
                    let test = d.join(".urec-write-test");
                    match std::fs::write(&test, b"ok") {
                        Ok(()) => {
                            let _ = std::fs::remove_file(test);
                            app.s.storage.output_dir = d.to_string_lossy().into();
                            app.library.refresh(&d);
                        }
                        Err(e) => app.push_notice(Level::Error, format!("uRec can't save to {}: {e}", d.display())),
                    }
                }
            }
        },
    );
    theme::row(ui, &p, "Sort into folders by game", "Clips from VALORANT go in a VALORANT folder, and so on.", |ui| {
        theme::switch(ui, &p, &mut app.s.storage.folder_per_app);
    });
    theme::row(
        ui,
        &p,
        "Warn when free space drops below",
        "Recording stops by itself below 1 GB to protect your system.",
        |ui| {
            ui.add(egui::DragValue::new(&mut app.s.storage.low_space_gb).range(1..=500).suffix(" GB"));
        },
    );
    theme::group_title(ui, &p, "Clean-up");
    theme::row(
        ui,
        &p,
        "Remove old recordings automatically",
        "Off unless you turn it on. Files go to the Recycle Bin, and starred favorites are always kept.",
        |ui| {
            theme::switch(ui, &p, &mut app.s.storage.retention);
        },
    );
    if app.s.storage.retention {
        let st = &mut app.s.storage;
        theme::row(ui, &p, "Older than", "", |ui| {
            ui.add(egui::DragValue::new(&mut st.retention_days).range(1..=3650).suffix(" days"));
        });
        theme::row(ui, &p, "Applies to", "", |ui| {
            combo(
                ui,
                "retention_scope",
                &mut st.retention_scope,
                &opts(&[("replays", "Instant Replay clips"), ("all", "All recordings")]),
                200.0,
            );
        });
        let (days, all) = (st.retention_days, st.retention_scope == "all");
        let n = app.library.retention_preview(days, all);
        theme::row(
            ui,
            &p,
            &if n == 0 {
                "Nothing to remove right now".to_string()
            } else {
                format!("{n} recording{} would be removed now", if n == 1 { "" } else { "s" })
            },
            "Runs each time uRec starts.",
            |ui| {
                let r = ui.add_enabled_ui(n > 0, |ui| theme::quiet_button(ui, &p, "Run now")).inner;
                if r.clicked() {
                    app.library.retention(&app.s.output_dir(), days, all);
                }
            },
        );
    }
    let bps = video::bytes_per_second(app.s.video.bitrate_kbps, app.s.audio.bitrate_kbps);
    ui.add_space(10.0);
    theme::note(
        ui,
        &p,
        &format!(
            "At the current settings a recording uses about {} per minute{}.",
            video::format_bytes(bps * 60),
            free.map(|f| format!(", so there's room for roughly {} of footage", hours(f / bps.max(1))))
                .unwrap_or_default()
        ),
    );
    ui.add_space(6.0);
    theme::note(
        ui,
        &p,
        "Recordings are saved as fragmented MP4: if the PC crashes, everything recorded up to that moment still plays. Choose MKV under Advanced if you prefer it.",
    );
}

fn hours(secs: u64) -> String {
    if secs >= 3600 {
        format!("{} hours", secs / 3600)
    } else {
        format!("{} minutes", secs / 60)
    }
}

// ------------------------------------------------------------------ Appearance

fn appearance(app: &mut App, ui: &mut egui::Ui) {
    let p = app.p;
    theme::page_title(ui, &p, "Appearance", "");
    let a = &mut app.s.appearance;
    theme::row(ui, &p, "Theme", "", |ui| {
        combo(
            ui,
            "theme",
            &mut a.theme,
            &opts(&[("system", "Same as Windows"), ("dark", "Dark"), ("light", "Light")]),
            180.0,
        );
    });
    theme::row(ui, &p, "Text size", "", |ui| {
        let o: Vec<(u32, String)> = [90, 100, 110, 125, 150].iter().map(|v| (*v, format!("{v}%"))).collect();
        let mut v = (a.ui_scale * 100.0).round() as u32;
        combo(ui, "scale", &mut v, &o, 180.0);
        a.ui_scale = v as f32 / 100.0;
    });
    let excl = urec_win::supports_exclude_from_capture();
    theme::row(
        ui,
        &p,
        "Recording overlay",
        if excl {
            "A small dot in a corner of your screen: red while recording, amber while Instant Replay is on. It briefly shows \"Instant Replay Saved\" when a clip is saved. It never appears in your videos."
        } else {
            "A small dot in a corner of your screen while recording. On this version of Windows it may appear in recordings."
        },
        |ui| {
            theme::switch(ui, &p, &mut a.overlay);
        },
    );
    ui.add_enabled_ui(a.overlay, |ui| {
        theme::row(ui, &p, "Overlay position", "", |ui| {
            combo(
                ui,
                "corner",
                &mut a.overlay_corner,
                &opts(&[
                    ("top_right", "Top right"),
                    ("top_left", "Top left"),
                    ("bottom_right", "Bottom right"),
                    ("bottom_left", "Bottom left"),
                ]),
                180.0,
            );
        });
        theme::row(ui, &p, "Overlay size", "", |ui| {
            combo(
                ui,
                "overlay_size",
                &mut a.overlay_size,
                &opts(&[("small", "Small"), ("medium", "Medium"), ("large", "Large")]),
                180.0,
            );
        });
        theme::row(
            ui,
            &p,
            "Dot opacity",
            "Save messages always appear at full strength so they stay readable.",
            |ui| {
                ui.add(egui::Slider::new(&mut a.overlay_opacity, 30..=100).suffix("%"));
            },
        );
    });
    let overlay_on = a.overlay;
    theme::row(
        ui,
        &p,
        "Animations",
        "Same as Windows follows Settings › Accessibility › Visual effects › Animation effects.",
        |ui| {
            combo(
                ui,
                "motion",
                &mut a.motion,
                &opts(&[("system", "Same as Windows"), ("reduced", "Reduced"), ("full", "Full")]),
                180.0,
            );
        },
    );
    let g = &mut app.s.general;
    theme::row(
        ui,
        &p,
        "Windows notifications",
        if overlay_on {
            "Used for Instant Replay results only when the overlay is off."
        } else {
            "The overlay is off, so Instant Replay results appear as Windows notifications."
        },
        |ui| {
            theme::switch(ui, &p, &mut g.notifications);
        },
    );
}

// ------------------------------------------------------------------ Advanced

fn advanced(app: &mut App, ui: &mut egui::Ui, st: &Status) {
    let p = app.p;
    theme::page_title(ui, &p, "Advanced", "");
    theme::row(
        ui,
        &p,
        "Keyframe interval",
        "Seconds between full frames. 2 is a good default for editing and Instant Replay.",
        |ui| {
            ui.add(egui::DragValue::new(&mut app.s.video.keyframe_seconds).range(1..=10).suffix(" s"));
        },
    );
    theme::row(
        ui,
        &p,
        "File format",
        "MP4 plays everywhere. MKV is more robust but some editors don't open it.",
        |ui| {
            combo(ui, "container", &mut app.s.video.container, &opts(&[("mp4", "MP4"), ("mkv", "MKV")]), 160.0);
        },
    );

    theme::row(ui, &p, "Setup", "Go through the first-run setup again, including the test recording.", |ui| {
        if theme::quiet_button(ui, &p, "Run setup again").clicked() {
            app.setup_step = 0;
            app.page = super::Page::Setup;
        }
    });
    theme::group_title(ui, &p, "Encoders on this PC");
    if st.probing {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Checking");
        });
    }
    egui::Grid::new("probe").num_columns(2).spacing(Vec2::new(18.0, 6.0)).show(ui, |ui| {
        for r in &st.probe {
            ui.label(r.encoder.to_string());
            if r.ok {
                ui.colored_label(p.ok, "Works");
            } else {
                ui.label(RichText::new(&r.reason).color(p.muted));
            }
            ui.end_row();
        }
    });
    ui.add_space(6.0);
    ui.add_enabled_ui(!st.busy(), |ui| {
        if theme::quiet_button(ui, &p, "Check again").clicked() {
            app.ctl.send(Command::Reprobe);
        }
    });

    theme::group_title(ui, &p, "About this installation");
    let build = urec_win::windows_build().map(|b| b.to_string()).unwrap_or_else(|| "unknown".into());
    theme::note(ui, &p, &format!("uRec {}. Windows build {build}.", env!("CARGO_PKG_VERSION")));
    theme::note(ui, &p, if st.ffmpeg_version.is_empty() { "FFmpeg not found." } else { &st.ffmpeg_version });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if theme::quiet_button(ui, &p, "Open log folder").clicked() {
            let d = crate::settings::local_dir().join("logs");
            let _ = std::fs::create_dir_all(&d);
            urec_win::reveal_in_explorer(&d);
        }
        if theme::quiet_button(ui, &p, "Open settings folder").clicked() {
            urec_win::reveal_in_explorer(&crate::settings::data_dir());
        }
    });
    ui.add_space(6.0);
    theme::note(ui, &p, "Logs record what uRec did (encoders, errors). They never contain your screen, audio or recordings, and nothing is sent anywhere.");

    theme::group_title(ui, &p, "Reset");
    if theme::quiet_button(ui, &p, "Reset all settings").clicked() {
        let keep_folder = app.s.storage.output_dir.clone();
        app.s = Settings { first_run_done: true, setup_done: true, ..Settings::default() };
        app.s.storage.output_dir = keep_folder;
        let caps = caps(app, st);
        let rec = recommended(&caps);
        app.s.set_video(&video::apply_preset(rec, &caps));
        app.s.video.preset = preset_id(rec).into();
        app.push_notice(Level::Info, "Settings were reset to the recommended values.".into());
    }
}
