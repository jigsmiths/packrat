use std::thread;
use std::time::Duration;

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

const DELAY: Duration = Duration::from_millis(80);
const KEY_V: u16 = 9;
const ACCESSIBILITY_SETTINGS: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

#[allow(unsafe_code)]
pub fn allowed() -> bool {
    // SAFETY: takes no arguments and only reads this process's trust state.
    unsafe { AXIsProcessTrusted() != 0 }
}

pub fn send_after_delay() {
    thread::spawn(|| {
        thread::sleep(DELAY);
        if let Err(reason) = send() {
            log::error!("paste failed: {reason}");
        }
    });
}

fn send() -> Result<(), &'static str> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|()| "could not create an event source")?;
    for key_down in [true, false] {
        let event = CGEvent::new_keyboard_event(source.clone(), KEY_V, key_down)
            .map_err(|()| "could not create a key event")?;
        event.set_flags(CGEventFlags::CGEventFlagCommand);
        event.post(CGEventTapLocation::HID);
    }
    Ok(())
}

pub fn open_accessibility_settings() {
    if let Err(err) = std::process::Command::new("open")
        .arg(ACCESSIBILITY_SETTINGS)
        .spawn()
    {
        log::error!("could not open Accessibility settings: {err}");
    }
}
