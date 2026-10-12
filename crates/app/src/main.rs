//! uRec: lightweight screen and game recorder for Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod controller;
mod editor;
mod library;
mod settings;
#[cfg(windows)]
mod tray;
mod ui;
mod update;

use controller::Controller;
use eframe::egui;
use settings::Settings;
use std::sync::Arc;

fn main() -> eframe::Result {
    // Record crashes in the log (the window app has no console).
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        controller::log(&format!("CRASH at {location}: {msg}\n{}", std::backtrace::Backtrace::force_capture()));
    }));
    let args: Vec<String> = std::env::args().collect();
    let minimized = args.iter().any(|a| a == "--minimized");
    // A button on an Instant Replay notification: open or reveal that clip, then exit.
    if let Some(i) = args.iter().position(|a| a == "--clip-link") {
        handle_clip_link(args.get(i + 1).map(|s| s.as_str()).unwrap_or(""));
        return Ok(());
    }
    // Headless update check (used by the release pipeline's end-to-end test).
    if args.iter().any(|a| a == "--update-check") {
        std::process::exit(update::cli_check());
    }
    // Only one uRec at a time: a second launch brings the first one forward.
    if !urec_win::claim_single_instance("Local\\uRec.SingleInstance") {
        urec_win::show_window_titled(ui::WINDOW_TITLE);
        return Ok(());
    }
    let (settings, message) = Settings::load();
    controller::log(&format!("uRec {} starting", env!("CARGO_PKG_VERSION")));
    // A verified update was downloaded last time: install it now and come back.
    if settings.general.auto_update && update::has_pending() {
        let relaunch = if minimized { update::Relaunch::Minimized } else { update::Relaunch::Normal };
        if update::apply_pending(relaunch) {
            return Ok(());
        }
    }
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        if !urec_win::register_notifications(&exe) {
            controller::log("notifications: couldn't register (results stay in the app)");
        }
    }
    let ctl = Controller::start(settings.clone());
    controller::log("startup: controller started");

    // Fit small screens (laptops at 1366×768, VMs at 1024×768): size and centre the window in
    // the usable area, so its bottom never ends up behind the taskbar.
    const TITLE_BAR: f32 = 32.0;
    let work = urec_win::work_area();
    let (w, h) = work
        .map(|(_, _, ww, wh)| ((ww * 0.92).min(1060.0), ((wh - TITLE_BAR) * 0.94).min(720.0)))
        .unwrap_or((1060.0, 720.0));
    let position = work.map(|(x, y, ww, wh)| [x + (ww - w) / 2.0, y + (wh - h - TITLE_BAR) / 2.0]);
    let icon = egui::IconData { rgba: include_bytes!("../assets/icon_64.rgba").to_vec(), width: 64, height: 64 };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(ui::WINDOW_TITLE)
        .with_app_id("uRec")
        .with_inner_size([w, h])
        .with_min_inner_size([w.min(880.0), h.min(560.0)])
        .with_icon(Arc::new(icon))
        .with_visible(!minimized);
    if let Some(pos) = position {
        viewport = viewport.with_position(pos);
    }
    let options = eframe::NativeOptions {
        viewport,
        centered: position.is_none(),
        renderer: match std::env::var("UREC_RENDERER").as_deref() {
            Ok("glow") => eframe::Renderer::Glow,
            _ => eframe::Renderer::Wgpu,
        },
        ..Default::default()
    };
    // Development/CI: open a specific page, e.g. UREC_PAGE=settings:video
    let start_page = std::env::var("UREC_PAGE").ok();
    controller::log("startup: opening window");
    let result = eframe::run_native(
        ui::WINDOW_TITLE,
        options,
        Box::new(move |cc| {
            controller::log("startup: window created");
            #[cfg(windows)]
            tray::install(ctl.clone());
            controller::log("startup: tray installed");
            let mut app = ui::App::new(cc, ctl, settings, message);
            controller::log("startup: app ready");
            if let Some(p) = start_page {
                app.open_page(&p);
            }
            Ok(Box::new(app))
        }),
    );
    if let Err(e) = &result {
        controller::log(&format!("window error: {e}"));
    }
    result
}

/// Handle a `urec://open?path=...` / `urec://reveal?path=...` link from a notification.
/// Only existing video files inside the recordings folder are opened.
fn handle_clip_link(uri: &str) {
    use urec_win::notify::{clip_allowed, parse_clip_uri, ClipAction};
    let Some((action, path)) = parse_clip_uri(uri) else {
        controller::log(&format!("clip link refused (malformed): {uri}"));
        return;
    };
    let (settings, _) = Settings::load();
    match clip_allowed(&path, &settings.output_dir()) {
        Ok(()) => match action {
            ClipAction::Open => urec_win::open_file(&path),
            ClipAction::Reveal => urec_win::reveal_in_explorer(&path),
        },
        Err("missing") => {
            // Moved or deleted since: show the folder instead, if it's still there.
            controller::log(&format!("clip link: {} no longer exists", path.display()));
            match path.parent().filter(|d| d.is_dir()) {
                Some(d) => urec_win::open_file(d),
                None => urec_win::open_file(&settings.output_dir()),
            }
        }
        Err(why) => controller::log(&format!("clip link refused ({why}): {}", path.display())),
    }
}
