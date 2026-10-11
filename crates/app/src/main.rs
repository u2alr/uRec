//! uRec: lightweight screen and game recorder for Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod controller;
mod library;
mod settings;
#[cfg(windows)]
mod tray;
mod ui;

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
    // Only one uRec at a time: a second launch brings the first one forward.
    if !urec_win::claim_single_instance("Local\\uRec.SingleInstance") {
        urec_win::show_window_titled(ui::WINDOW_TITLE);
        return Ok(());
    }
    let (settings, message) = Settings::load();
    controller::log(&format!("uRec {} starting", env!("CARGO_PKG_VERSION")));
    let ctl = Controller::start(settings.clone());
    controller::log("startup: controller started");

    // Fit small screens (laptops at 1366×768, VMs at 1024×768).
    let (w, h) = urec_win::monitors()
        .into_iter()
        .find(|m| m.primary)
        .map(|m| ((m.bounds.width as f32 * 0.92).min(1060.0), (m.bounds.height as f32 * 0.88).min(720.0)))
        .unwrap_or((1060.0, 720.0));
    let icon = egui::IconData { rgba: include_bytes!("../assets/icon_64.rgba").to_vec(), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(ui::WINDOW_TITLE)
            .with_app_id("uRec")
            .with_inner_size([w, h])
            .with_min_inner_size([w.min(880.0), h.min(560.0)])
            .with_icon(Arc::new(icon))
            .with_visible(!minimized),
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
