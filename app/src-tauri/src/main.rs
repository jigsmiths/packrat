mod capture;
mod commands;
mod popup;
mod storage;
mod tray;

use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, PoisonError};

use packrat_core::history::History;
use tauri::{ActivationPolicy, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_log::{Target, TargetKind};

/// Shared between the capture thread, the tray and the popup.
#[derive(Default)]
pub struct AppState {
    /// `None` while the history cannot be opened.
    history: Mutex<Option<History>>,
    /// Why the history cannot be opened, shown in the tray menu.
    problem: Mutex<Option<String>>,
    /// Why the global hotkey could not be registered, shown in the tray menu.
    hotkey_problem: Mutex<Option<String>>,
    pub paused: AtomicBool,
}

impl AppState {
    pub fn history(&self) -> MutexGuard<'_, Option<History>> {
        self.history.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn problem(&self) -> MutexGuard<'_, Option<String>> {
        self.problem.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn hotkey_problem(&self) -> MutexGuard<'_, Option<String>> {
        self.hotkey_problem
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            popup::show(app);
        }))
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
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let handle = app.clone();
                        let _ = app.run_on_main_thread(move || popup::toggle(&handle));
                    }
                })
                .build(),
        )
        .manage(AppState::default())
        .setup(|app| {
            app.set_activation_policy(ActivationPolicy::Accessory);

            let handle = app.handle();
            let state = handle.state::<AppState>();
            let data_dir = handle.path().app_data_dir()?;
            let opened = storage::open(&data_dir, &handle.config().identifier);
            match &opened {
                Ok(history) => log::info!("history opened: clips={}", history.clips().len()),
                Err(problem) => log::error!("history not opened: {problem}"),
            }
            state.set_history(opened);

            let hotkey = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyV);
            if let Err(err) = handle.global_shortcut().register(hotkey) {
                log::error!("hotkey not registered: {err}");
                *state.hotkey_problem() = Some(format!("Cmd+Shift+V ({err})"));
            }

            if let Some(window) = handle.get_webview_window(popup::LABEL) {
                popup::prepare(&window)?;
            }
            tray::create(handle)?;
            capture::spawn(handle.clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == popup::LABEL && matches!(event, WindowEvent::Focused(false)) {
                popup::hide(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_clips,
            commands::preview_clip,
            commands::copy_clip,
            commands::toggle_pin,
            commands::delete_clip,
            commands::hide_popup,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Packrat");
}
