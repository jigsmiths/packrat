//! Background thread that feeds the pasteboard into the history.

use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use packrat_core::collector::Collector;
use packrat_core::pasteboard::SystemPasteboard;
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Emitted to the frontend whenever the history gains or loses clips.
pub const HISTORY_CHANGED: &str = "history-changed";

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

pub fn spawn(app: AppHandle) {
    thread::spawn(move || {
        let pasteboard = SystemPasteboard;
        let mut collector = Collector::new(&pasteboard);
        loop {
            thread::sleep(POLL_INTERVAL);
            let state = app.state::<AppState>();
            collector.set_paused(state.paused.load(Ordering::Relaxed));

            let mut history = state.history();
            let Some(history) = history.as_mut() else {
                continue;
            };
            match collector.poll(&pasteboard, history, now_ms()) {
                Ok(poll) if poll.changed() => {
                    // Never log clip content.
                    log::info!(
                        "history changed: added={:?} pruned={} clips={}",
                        poll.added,
                        poll.pruned,
                        history.clips().len()
                    );
                    let _ = app.emit(HISTORY_CHANGED, ());
                }
                Ok(_) => {}
                Err(err) => log::error!("capture failed: {err}"),
            }
        }
    });
}
