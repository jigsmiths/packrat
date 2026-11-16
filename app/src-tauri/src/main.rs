mod capture;
mod storage;
mod tray;

use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, PoisonError};

use packrat_core::history::History;
use tauri::{ActivationPolicy, Manager};
use tauri_plugin_log::{Target, TargetKind};

/// Shared between the capture thread and the tray.
#[derive(Default)]
pub struct AppState {
    /// `None` while the history cannot be opened.
    history: Mutex<Option<History>>,
    /// Why the history cannot be opened, shown in the tray menu.
    problem: Mutex<Option<String>>,
    pub paused: AtomicBool,
}

impl AppState {
    pub fn history(&self) -> MutexGuard<'_, Option<History>> {
        self.history.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn problem(&self) -> MutexGuard<'_, Option<String>> {
        self.problem.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set_history(&self, opened: Result<History, String>) {
        let (history, problem) = match opened {
            Ok(history) => (Some(history), None),
            Err(problem) => (None, Some(problem)),
        };
        *self.history() = history;
        *self.problem() = problem;
    }
}

fn main() {
    tauri::Builder::default()
        // Must be registered first so a second launch exits before doing anything else.
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .plugin(
            tauri_plugin_log::Builder::new()
                .clear_targets()
                .targets([
                    Target::new(TargetKind::LogDir { file_name: None }),
                    Target::new(TargetKind::Stdout),
                ])
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|app| {
            app.set_activation_policy(ActivationPolicy::Accessory);

            let handle = app.handle();
            let data_dir = handle.path().app_data_dir()?;
            let opened = storage::open(&data_dir, &handle.config().identifier);
            match &opened {
                Ok(history) => log::info!("history opened: clips={}", history.clips().len()),
                Err(problem) => log::error!("history not opened: {problem}"),
            }
            handle.state::<AppState>().set_history(opened);

            tray::create(handle)?;
            capture::spawn(handle.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run Packrat");
}
