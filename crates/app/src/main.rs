//! uRec: lightweight screen and game recorder for Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod controller;
mod editor;
mod library;
mod settings;
mod startup;
#[cfg(windows)]
mod tray;
mod ui;
mod update;

use controller::Controller;
use eframe::egui;
use settings::Settings;
use std::sync::atomic::{AtomicBool, Ordering};
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
        // Bring the running uRec forward. It may still be starting, so give it a moment.
        let mut shown = false;
        for _ in 0..8 {
            if urec_win::show_window_titled(ui::WINDOW_TITLE) {
                shown = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        if !shown {
            controller::log("second launch: the running uRec has no window to show");
            if std::env::var_os("CI").is_none() {
                urec_win::error_box(
                    "uRec",
                    "uRec is already running but has no window to show. It may still be starting, or it may be stuck.\n\nIf this keeps happening, close \"uRec\" in Task Manager (Ctrl+Shift+Esc) and open it again.",
                );
            }
        }
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
    // Development/CI: open a specific page, e.g. UREC_PAGE=settings:video
    let start_page = std::env::var("UREC_PAGE").ok();
    let look = WindowLook { w, h, position, hidden: minimized };
    let first = startup::choose();
    controller::log(&format!("startup: opening window ({})", first.name()));
    let created = Arc::new(AtomicBool::new(false));
    let mut result = open_window(first, &look, &ctl, &settings, &message, &start_page, &created);
    // The window couldn't be created (graphics driver): try the other renderer before giving up.
    if let (Err(e), false) = (&result, created.load(Ordering::Relaxed)) {
        controller::log(&format!("window error with {}: {e}; trying {}", first.name(), first.other().name()));
        result = open_window(first.other(), &look, &ctl, &settings, &message, &start_page, &created);
        if result.is_ok() && std::env::var_os("UREC_RENDERER").is_none() {
            match first {
                startup::Renderer::Wgpu => startup::remember_opengl(),
                startup::Renderer::Glow => startup::forget_opengl(),
            }
        }
    }
    if let Err(e) = &result {
        controller::log(&format!("window error: {e}"));
        startup::show_failure(&e.to_string());
    }
    result
}

/// Where and how big the window is.
struct WindowLook {
    w: f32,
    h: f32,
    position: Option<[f32; 2]>,
    hidden: bool,
}

fn open_window(
    renderer: startup::Renderer,
    look: &WindowLook,
    ctl: &Controller,
    settings: &Settings,
    message: &Option<String>,
    start_page: &Option<String>,
    created: &Arc<AtomicBool>,
) -> eframe::Result {
    let (w, h) = (look.w, look.h);
    let icon = egui::IconData { rgba: include_bytes!("../assets/icon_64.rgba").to_vec(), width: 64, height: 64 };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(ui::WINDOW_TITLE)
        .with_app_id("uRec")
        .with_inner_size([w, h])
        .with_min_inner_size([w.min(880.0), h.min(560.0)])
        .with_icon(Arc::new(icon))
        .with_visible(!look.hidden);
    if let Some(pos) = look.position {
        viewport = viewport.with_position(pos);
    }
    let options = eframe::NativeOptions {
        viewport,
        centered: look.position.is_none(),
        renderer: match renderer {
            startup::Renderer::Glow => eframe::Renderer::Glow,
            startup::Renderer::Wgpu => eframe::Renderer::Wgpu,
        },
        ..Default::default()
    };
    let (ctl, settings, message, start_page, created) =
        (ctl.clone(), settings.clone(), message.clone(), start_page.clone(), created.clone());
    startup::begin_trial(renderer);
    eframe::run_native(
        ui::WINDOW_TITLE,
        options,
        Box::new(move |cc| {
            created.store(true, Ordering::Relaxed);
            controller::log("startup: window created");
            #[cfg(windows)]
            tray::install(ctl.clone());
            controller::log("startup: tray installed");
            let mut app = ui::App::new(cc, ctl, settings, message);
            controller::log("startup: app ready");
            if let Some(p) = start_page {
                app.open_page(&p);
            }
            // CI: open the editor on these files ("a.mp4|b.mp4").
            if let Ok(files) = std::env::var("UREC_EDIT") {
                app.open_editor(files.split('|').map(std::path::PathBuf::from).collect());
            }
            Ok(Box::new(app))
        }),
    )
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
