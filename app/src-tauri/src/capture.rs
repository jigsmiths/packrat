use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use packrat_core::collector::Collector;
use packrat_core::pasteboard::SystemPasteboard;
use tauri::{AppHandle, Emitter, Manager};

use crate::{config, AppState};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub const HISTORY_CHANGED: &str = "history-changed";

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

pub fn spawn(app: AppHandle, mut config: config::Watcher) {
    thread::spawn(move || {
        let pasteboard = SystemPasteboard;
        let mut collector = Collector::starting_after_current(&pasteboard);
        loop {
            thread::sleep(POLL_INTERVAL);
            config.poll(&app);
            let state = app.state::<AppState>();
            collector.set_paused(state.paused.load(Ordering::Relaxed));

            let mut history = state.history();
            let Some(history) = history.as_mut() else {
                continue;
            };
            match collector.poll(&pasteboard, history, now_ms()) {
                Ok(poll) if poll.changed() => {
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
