//! The menu bar icon and its menu.

use std::sync::atomic::Ordering;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::{popup, storage, AppState};

const TRAY_ID: &str = "main";
const OPEN: &str = "open";
const PAUSE: &str = "pause";
const RESET: &str = "reset";
const QUIT: &str = "quit";

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        // A template image is recolored by macOS to suit a light or dark menu bar.
        .icon_as_template(true)
        .menu(&build_menu(app)?)
        .on_menu_event(on_menu_event)
        .build(app)?;
    refresh(app);
    Ok(())
}

/// Rebuilds the menu and the icon's label from the current state.
pub fn refresh(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let state = app.state::<AppState>();
    let has_problem = state.problem().is_some() || state.settings_problem().is_some();
    let tooltip = if has_problem {
        "Packrat: needs attention"
    } else if state.paused.load(Ordering::Relaxed) {
        "Packrat: paused"
    } else {
        "Packrat"
    };
    let result = build_menu(app)
        .and_then(|menu| tray.set_menu(Some(menu)))
        .and_then(|()| tray.set_tooltip(Some(tooltip)))
        .and_then(|()| tray.set_title(has_problem.then_some("!")));
    if let Err(err) = result {
        log::error!("could not update the tray: {err}");
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let state = app.state::<AppState>();
    let menu = Menu::new(app)?;

    if let Some(problem) = state.problem().as_deref() {
        let label = format!("Not capturing: {problem}");
        menu.append(&MenuItem::new(app, label, false, None::<&str>)?)?;
        menu.append(&MenuItem::with_id(
            app,
            RESET,
            "Reset history…",
            true,
            None::<&str>,
        )?)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    if let Some(problem) = state.settings_problem().as_deref() {
        let label = format!("Settings problem: {problem}");
        menu.append(&MenuItem::new(app, label, false, None::<&str>)?)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }

    menu.append(&MenuItem::with_id(
        app,
        OPEN,
        "Open Packrat",
        true,
        None::<&str>,
    )?)?;
    let paused = state.paused.load(Ordering::Relaxed);
    menu.append(&CheckMenuItem::with_id(
        app,
        PAUSE,
        "Pause capture",
        true,
        paused,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(app, QUIT, "Quit", true, None::<&str>)?)?;
    Ok(menu)
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id.as_ref() {
        OPEN => popup::show(app),
        PAUSE => {
            let paused = !app
                .state::<AppState>()
                .paused
                .fetch_xor(true, Ordering::Relaxed);
            log::info!("capture paused={paused}");
            refresh(app);
        }
        RESET => confirm_reset(app),
        QUIT => app.exit(0),
        _ => {}
    }
}

fn confirm_reset(app: &AppHandle) {
    let handle = app.clone();
    app.dialog()
        .message("This deletes every saved clip, including pinned ones. It cannot be undone.")
        .title("Reset Packrat history?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Reset".to_owned(),
            "Cancel".to_owned(),
        ))
        .show(move |confirmed| {
            if confirmed {
                reset(&handle);
            }
        });
}

fn reset(app: &AppHandle) {
    let state = app.state::<AppState>();
    let result = app
        .path()
        .app_data_dir()
        .map_err(|err| err.to_string())
        .and_then(|dir| storage::reset(&dir, &app.config().identifier));
    match result {
        Ok(history) => {
            log::warn!("history reset by the user");
            state.set_history(Ok(history));
        }
        Err(problem) => {
            log::error!("history reset failed: {problem}");
            state.set_history(Err(problem));
        }
    }
    refresh(app);
}
