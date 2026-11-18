//! The About window, opened from the tray menu.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::focus;

const LABEL: &str = "about";
const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const COPYRIGHT: &str = "Copyright (c) 2026 jigsmiths";
const LICENSE: &str = concat!(env!("CARGO_PKG_LICENSE"), " License");
// Bundled only by release builds, see tauri.bundle.conf.json.
const LICENSES_FILE: &str = "THIRD-PARTY-LICENSES.txt";

/// What the About window can open. The page names a link, never a URL or path.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Link {
    Repo,
    DataFolder,
    Licenses,
}

struct Places {
    data_folder: PathBuf,
    licenses: PathBuf,
}

impl Places {
    fn of(app: &AppHandle) -> tauri::Result<Self> {
        let path = app.path();
        Ok(Self {
            data_folder: path.app_data_dir()?,
            licenses: path.resource_dir()?.join(LICENSES_FILE),
        })
    }
}

/// What `open` should be given for a link, if it is there to open.
fn resolve(link: Link, places: &Places) -> Option<OsString> {
    let path = match link {
        Link::Repo => return Some(REPO.into()),
        Link::DataFolder => &places.data_folder,
        Link::Licenses => &places.licenses,
    };
    path.exists().then(|| path.clone().into_os_string())
}

pub fn show(app: &AppHandle) {
    let window = match app.get_webview_window(LABEL) {
        Some(window) => Ok(window),
        None => WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("about.html".into()))
            .title(format!("About {}", app.package_info().name))
            .inner_size(380.0, 300.0)
            .resizable(false)
            .center()
            .build(),
    };
    let result = window.and_then(|window| window.show().and_then(|()| window.set_focus()));
    if let Err(err) = result {
        log::error!("could not show the About window: {err}");
        return;
    }
    // A menu bar app is not activated by showing a window, so the window would open behind.
    if !focus::activate(std::process::id() as i32) {
        log::warn!("could not bring Packrat to the front");
    }
}

#[derive(Serialize)]
pub struct Info {
    name: String,
    version: String,
    description: &'static str,
    copyright: &'static str,
    license: &'static str,
    repo: &'static str,
    data_folder: String,
    has_licenses: bool,
}

#[tauri::command]
pub fn about_info(app: AppHandle) -> Result<Info, String> {
    let places = Places::of(&app).map_err(|err| err.to_string())?;
    let package = app.package_info();
    Ok(Info {
        name: package.name.clone(),
        version: package.version.to_string(),
        description: env!("CARGO_PKG_DESCRIPTION"),
        copyright: COPYRIGHT,
        license: LICENSE,
        repo: REPO,
        data_folder: places.data_folder.display().to_string(),
        has_licenses: resolve(Link::Licenses, &places).is_some(),
    })
}

#[tauri::command]
pub fn open_link(app: AppHandle, link: Link) -> Result<(), String> {
    let places = Places::of(&app).map_err(|err| err.to_string())?;
    let target = resolve(link, &places).ok_or_else(|| format!("{link:?} is not available"))?;
    Command::new("open")
        .arg(target)
        .spawn()
        .map(drop)
        .map_err(|err| format!("could not open {link:?}: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(data_folder: PathBuf, licenses: PathBuf) -> Places {
        Places {
            data_folder,
            licenses,
        }
    }

    fn missing() -> PathBuf {
        std::env::temp_dir().join("packrat-about-test-missing")
    }

    #[test]
    fn repo_resolves_to_the_repository_url() {
        let places = places(missing(), missing());
        assert_eq!(
            resolve(Link::Repo, &places),
            Some("https://github.com/jigsmiths/packrat".into())
        );
    }

    #[test]
    fn existing_paths_resolve_to_themselves() {
        let folder = std::env::temp_dir();
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let places = places(folder.clone(), file.clone());
        assert_eq!(
            resolve(Link::DataFolder, &places),
            Some(folder.into_os_string())
        );
        assert_eq!(
            resolve(Link::Licenses, &places),
            Some(file.into_os_string())
        );
    }

    #[test]
    fn missing_paths_do_not_resolve() {
        let places = places(missing(), missing());
        assert_eq!(resolve(Link::DataFolder, &places), None);
        assert_eq!(resolve(Link::Licenses, &places), None);
    }
}
