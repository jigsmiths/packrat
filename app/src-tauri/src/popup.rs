use std::error::Error;

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSWindow, NSWindowCollectionBehavior};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow};

use crate::focus;

pub const LABEL: &str = "main";

const SHOWN: &str = "popup-shown";

pub fn follow_active_space(window: &WebviewWindow) -> Result<(), Box<dyn Error>> {
    let mtm = MainThreadMarker::new().ok_or("the popup must be set up on the main thread")?;
    let target = window.ns_window()?.cast::<NSWindow>().cast_const();
    let ns_window = NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .find(|candidate| Retained::as_ptr(candidate) == target)
        .ok_or("the popup's NSWindow is not one of the app's windows")?;
    ns_window.setCollectionBehavior(
        ns_window.collectionBehavior()
            | NSWindowCollectionBehavior::MoveToActiveSpace
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    Ok(())
}

pub fn show(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    focus::remember();
    let result = place_on_cursor_screen(&window)
        .and_then(|()| window.show())
        .and_then(|()| window.set_focus())
        .and_then(|()| app.emit(SHOWN, ()));
    if let Err(err) = result {
        log::error!("could not show the popup: {err}");
    }
}

pub enum Focus {
    ReturnToPrevious,
    Stay,
}

pub fn hide(app: &AppHandle, focus: Focus) {
    let previous = focus::take();
    if let Some(window) = app.get_webview_window(LABEL) {
        if let Err(err) = window.hide() {
            log::error!("could not hide the popup: {err}");
        }
    }
    if let (Some(pid), Focus::ReturnToPrevious) = (previous, focus) {
        if !focus::activate(pid) {
            log::warn!("could not return focus to the previous app");
        }
    }
}

pub fn toggle(app: &AppHandle) {
    let visible = app
        .get_webview_window(LABEL)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    if visible {
        hide(app, Focus::ReturnToPrevious);
    } else {
        show(app);
    }
}

fn place_on_cursor_screen(window: &WebviewWindow) -> tauri::Result<()> {
    let cursor = window.cursor_position()?;
    let Some(monitor) = window.monitor_from_point(cursor.x, cursor.y)? else {
        return window.center();
    };
    let area = monitor.work_area();
    let size = window.outer_size()?;
    let x = area.position.x + (area.size.width as i32 - size.width as i32) / 2;
    let y = area.position.y + (area.size.height as i32 - size.height as i32) / 3;
    window.set_position(PhysicalPosition::new(x, y))
}
