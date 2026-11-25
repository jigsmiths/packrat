use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use packrat_core::history::parse_ttl;
use serde::Deserialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::{tray, AppState};

const FILE_NAME: &str = "config.toml";

const TEMPLATE: &str = r#"# Packrat settings. Changes apply as soon as this file is saved.

# How long unpinned clips are kept, for example "24h" or "90m".
# "0" keeps them until you delete them.
ttl = "24h"

# The global hotkey that opens Packrat.
hotkey = "Cmd+Shift+V"
"#;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    ttl: String,
    hotkey: String,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            ttl: "24h".to_owned(),
            hotkey: "Cmd+Shift+V".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub ttl: Option<Duration>,
    pub hotkey: Shortcut,
}

fn parse(text: &str) -> Result<Settings, String> {
    let file: ConfigFile = toml::from_str(text).map_err(|err| err.message().to_owned())?;
    let ttl = parse_ttl(&file.ttl).map_err(|err| err.to_string())?;
    let hotkey = file
        .hotkey
        .parse()
        .map_err(|err| format!("invalid hotkey {:?}: {err}", file.hotkey))?;
    Ok(Settings { ttl, hotkey })
}

fn read(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(format!("could not read the file: {err}")),
    }
}

pub struct Watcher {
    path: PathBuf,
    loaded: bool,
    modified: Option<SystemTime>,
}

impl Watcher {
    pub fn new(app: &AppHandle) -> tauri::Result<Self> {
        let dir = app.path().app_config_dir()?;
        let path = dir.join(FILE_NAME);
        if !path.exists() {
            let written =
                std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, TEMPLATE));
            if let Err(err) = written {
                log::warn!("could not write the default config: {err}");
            }
        }
        Ok(Self {
            path,
            loaded: false,
            modified: None,
        })
    }

    pub fn poll(&mut self, app: &AppHandle) {
        let modified = std::fs::metadata(&self.path)
            .and_then(|meta| meta.modified())
            .ok();
        if self.loaded && modified == self.modified {
            return;
        }
        let first_load = !self.loaded;
        self.loaded = true;
        self.modified = modified;

        let problem = match read(&self.path).and_then(|text| parse(&text)) {
            Ok(settings) => apply(app, settings),
            Err(reason) => {
                if first_load {
                    if let Ok(defaults) = parse("") {
                        apply(app, defaults);
                    }
                }
                Some(reason)
            }
        };
        if let Some(problem) = &problem {
            log::error!("{FILE_NAME}: {problem}");
        }
        *app.state::<AppState>().settings_problem() = problem.map(|p| format!("{FILE_NAME}: {p}"));
        tray::refresh(app);
    }
}

fn apply(app: &AppHandle, wanted: Settings) -> Option<String> {
    let state = app.state::<AppState>();
    let previous = state.settings().map(|settings| settings.hotkey);
    let mut effective = wanted;
    let mut problem = None;

    if previous != Some(wanted.hotkey) {
        let shortcuts = app.global_shortcut();
        if let Some(previous) = previous {
            let _ = shortcuts.unregister(previous);
        }
        if let Err(err) = shortcuts.register(wanted.hotkey) {
            problem = Some(format!("hotkey {} is unavailable ({err})", wanted.hotkey));
            if let Some(previous) = previous {
                let _ = shortcuts.register(previous);
                effective.hotkey = previous;
            }
        }
    }

    state.set_settings(effective);
    log::info!(
        "settings applied: ttl={:?} hotkey={}",
        effective.ttl,
        effective.hotkey
    );
    problem
}

#[cfg(test)]
mod tests {
    use tauri_plugin_global_shortcut::{Code, Modifiers};

    use super::*;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn default_hotkey() -> Shortcut {
        Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyV)
    }

    #[test]
    fn empty_file_gives_the_defaults() {
        let settings = parse("").unwrap();
        assert_eq!(settings.ttl, Some(DAY));
        assert_eq!(settings.hotkey, default_hotkey());
    }

    #[test]
    fn the_template_parses_to_the_defaults() {
        assert_eq!(parse(TEMPLATE).unwrap(), parse("").unwrap());
    }

    #[test]
    fn values_are_read_from_the_file() {
        let settings = parse("ttl = \"90m\"\nhotkey = \"Ctrl+Alt+Space\"").unwrap();
        assert_eq!(settings.ttl, Some(Duration::from_secs(90 * 60)));
        assert_eq!(
            settings.hotkey,
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space)
        );
    }

    #[test]
    fn one_setting_can_be_given_alone() {
        let settings = parse("ttl = \"0\"").unwrap();
        assert_eq!(settings.ttl, None);
        assert_eq!(settings.hotkey, default_hotkey());
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let path = std::env::temp_dir().join("packrat-test-no-such-config.toml");
        assert_eq!(read(&path), Ok(String::new()));
    }

    #[test]
    fn unreadable_file_is_an_error() {
        let err = read(&std::env::temp_dir()).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn bad_values_are_rejected_with_a_reason() {
        for text in [
            "ttl = \"soon\"",
            "hotkey = \"Cmd+Shift+NotAKey\"",
            "hotkey = \"\"",
            "ttl = 24",
            "tll = \"24h\"",
            "ttl = ",
        ] {
            let err = parse(text).unwrap_err();
            assert!(!err.is_empty(), "{text:?}");
        }
    }
}
