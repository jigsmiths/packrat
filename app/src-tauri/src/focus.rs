//! A menu bar app stays frontmost after its window hides, so without this the
//! user would have to click back into their own window.

use std::sync::{Mutex, PoisonError};

use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};

static PREVIOUS: Mutex<Option<i32>> = Mutex::new(None);

pub fn remember() {
    let frontmost = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier());
    if let Some(pid) = frontmost.filter(|pid| *pid != std::process::id() as i32) {
        *PREVIOUS.lock().unwrap_or_else(PoisonError::into_inner) = Some(pid);
    }
}

pub fn take() -> Option<i32> {
    PREVIOUS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
}

pub fn activate(pid: i32) -> bool {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .is_some_and(|app| app.activateWithOptions(NSApplicationActivationOptions::empty()))
}
