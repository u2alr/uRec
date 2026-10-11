//! System tray icon and menu. Handlers run on the UI thread's message loop and act through
//! the controller, so they work even while the main window is hidden.

use crate::controller::{Command, Controller};
use crate::ui::WINDOW_TITLE;
use std::time::Duration;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

pub fn install(ctl: Controller) {
    let menu = Menu::new();
    let open = MenuItem::new("Open uRec", true, None);
    let record = MenuItem::new("Start or stop recording", true, None);
    let save = MenuItem::new("Save Instant Replay", true, None);
    let replay = MenuItem::new("Turn Instant Replay on or off", true, None);
    let quit = MenuItem::new("Quit uRec", true, None);
    let _ = menu.append_items(&[
        &open,
        &PredefinedMenuItem::separator(),
        &record,
        &save,
        &replay,
        &PredefinedMenuItem::separator(),
        &quit,
    ]);
    let icon = match Icon::from_rgba(include_bytes!("../assets/tray_rec_32.rgba").to_vec(), 32, 32) {
        Ok(i) => i,
        Err(e) => {
            crate::controller::log(&format!("tray icon: {e}"));
            return;
        }
    };
    let tray = match TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("uRec").with_icon(icon).build() {
        Ok(t) => t,
        Err(e) => {
            crate::controller::log(&format!("tray: {e}"));
            return;
        }
    };
    // The tray icon lives as long as the app.
    Box::leak(Box::new(tray));

    let (open_id, record_id, save_id, replay_id, quit_id) =
        (open.id().clone(), record.id().clone(), save.id().clone(), replay.id().clone(), quit.id().clone());
    let c = ctl.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == open_id {
            urec_win::show_window_titled(WINDOW_TITLE);
        } else if e.id == record_id {
            c.send(Command::ToggleRecording);
        } else if e.id == save_id {
            c.send(Command::SaveReplay);
        } else if e.id == replay_id {
            c.send(Command::ToggleReplay);
        } else if e.id == quit_id {
            // Finish any recording properly before leaving.
            c.shutdown_and_wait(Duration::from_secs(90));
            crate::update::apply_on_quit(crate::update::Relaunch::No);
            std::process::exit(0);
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            urec_win::show_window_titled(WINDOW_TITLE);
        }
    }));
}
