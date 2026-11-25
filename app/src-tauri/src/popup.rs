use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow};

use crate::focus;

pub const LABEL: &str = "main";

const SHOWN: &str = "popup-shown";

pub fn prepare(window: &WebviewWindow) -> tauri::Result<()> {
    let ns_window = window.ns_window()?.cast::<NSWindow>();
    // SAFETY: Tauri returns a valid NSWindow that outlives this call, and
    // setup runs on the main thread.
    #[allow(unsafe_code)]
    let ns_window = unsafe { &*ns_window };
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

pub fn hide(app: &AppHandle, restore_focus: bool) {
    let previous = focus::take();
    if let Some(window) = app.get_webview_window(LABEL) {
        if let Err(err) = window.hide() {
            log::error!("could not hide the popup: {err}");
        }
    }
    if let Some(pid) = previous.filter(|_| restore_focus) {
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
        hide(app, true);
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
