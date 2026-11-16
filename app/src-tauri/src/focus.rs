//! Hands keyboard focus back to the app the user was in before the popup.
//!
//! A menu bar app stays frontmost after its window hides, so without this the
//! user would have to click back into their own window.

use std::sync::{Mutex, PoisonError};

use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};

static PREVIOUS: Mutex<Option<i32>> = Mutex::new(None);

/// Records the frontmost app. Call before showing the popup.
pub fn remember() {
    let frontmost = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier());
    // When the popup is reopened while Packrat is frontmost, keep the earlier answer.
    if let Some(pid) = frontmost.filter(|pid| *pid != std::process::id() as i32) {
        *PREVIOUS.lock().unwrap_or_else(PoisonError::into_inner) = Some(pid);
    }
}

/// Returns the remembered app's process id and forgets it.
pub fn take() -> Option<i32> {
    PREVIOUS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
}

/// Makes the app with this process id frontmost. False if it has quit.
pub fn activate(pid: i32) -> bool {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .is_some_and(|app| app.activateWithOptions(NSApplicationActivationOptions::empty()))
}
