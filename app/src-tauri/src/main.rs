mod about;
mod capture;
mod commands;
mod config;
mod focus;
mod paste;
mod popup;
mod storage;
mod tray;

use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, PoisonError};

use config::Settings;
use packrat_core::history::History;
use tauri::{ActivationPolicy, Manager, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_global_shortcut::ShortcutState;
use tauri_plugin_log::{Target, TargetKind};

#[derive(Default)]
pub enum HistoryState {
    #[default]
    NotOpened,
    Open(History),
    Failed(String),
}

impl HistoryState {
    pub fn problem(&self) -> Option<&str> {
        match self {
            Self::Failed(problem) => Some(problem),
            Self::NotOpened | Self::Open(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum SettingsState {
    #[default]
    Loading,
    Loaded(Settings),
}

#[derive(Default)]
pub struct AppState {
    history: Mutex<HistoryState>,
    settings_problem: Mutex<Option<String>>,
    settings: Mutex<SettingsState>,
    pub paused: AtomicBool,
}

impl AppState {
    pub fn history(&self) -> MutexGuard<'_, HistoryState> {
        self.history.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn settings_problem(&self) -> MutexGuard<'_, Option<String>> {
        self.settings_problem
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn settings(&self) -> SettingsState {
        *self.settings.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set_settings(&self, settings: Settings) {
        *self.settings.lock().unwrap_or_else(PoisonError::into_inner) =
            SettingsState::Loaded(settings);
        self.apply_ttl(&mut self.history());
    }

    pub fn set_history(&self, opened: Result<History, String>) {
        let mut history = match opened {
            Ok(history) => HistoryState::Open(history),
            Err(problem) => HistoryState::Failed(problem),
        };
        self.apply_ttl(&mut history);
        *self.history() = history;
    }

    fn apply_ttl(&self, history: &mut HistoryState) {
        if let (HistoryState::Open(history), SettingsState::Loaded(settings)) =
            (history, self.settings())
        {
            history.set_ttl(settings.ttl);
        }
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
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let handle = app.clone();
                        #[expect(
                            clippy::let_underscore_must_use,
                            reason = "fails only when the event loop has stopped, as the app quits"
                        )]
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

            if let Some(window) = handle.get_webview_window(popup::LABEL) {
                popup::follow_active_space(&window)?;
            }
            tray::create(handle)?;

            config::write_default_if_missing(handle)?;
            let mut config = config::Watcher::new(handle)?;
            config.poll(handle);
            capture::spawn(handle.clone(), config);
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == popup::LABEL && matches!(event, WindowEvent::Focused(false)) {
                popup::hide(window.app_handle(), popup::Focus::Stay);
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_clips,
            commands::preview_clip,
            commands::image_clip,
            commands::copy_clip,
            commands::paste_allowed,
            commands::open_accessibility_settings,
            commands::toggle_pin,
            commands::delete_clip,
            commands::hide_popup,
            about::about_info,
            about::open_link,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Packrat");
}
