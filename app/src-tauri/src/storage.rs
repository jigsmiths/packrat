//! Opens the encrypted history for this app identity.

use std::path::Path;

use packrat_core::db::{Db, Key};
use packrat_core::history::History;
use packrat_core::{keychain, Error};

const DB_FILE: &str = "packrat.db";

#[derive(Debug, PartialEq, Eq)]
enum KeyPlan {
    Use(Key),
    Create,
    /// A database exists but its key is gone. Making a new key would only hide that.
    Missing,
}

fn plan_key(stored: Option<Key>, db_exists: bool) -> KeyPlan {
    match (stored, db_exists) {
        (Some(key), _) => KeyPlan::Use(key),
        (None, false) => KeyPlan::Create,
        (None, true) => KeyPlan::Missing,
    }
}

/// Opens the history in `data_dir`. `service` names the Keychain entry.
/// The error is a message fit to show in the tray menu.
pub fn open(data_dir: &Path, service: &str) -> Result<History, String> {
    let path = data_dir.join(DB_FILE);
    let stored = keychain::load_key(service).map_err(|err| err.to_string())?;
    let key = match plan_key(stored, path.exists()) {
        KeyPlan::Use(key) => key,
        KeyPlan::Create => keychain::create_key(service).map_err(|err| err.to_string())?,
        KeyPlan::Missing => {
            return Err("the history key is missing from the Keychain".to_owned());
        }
    };
    Db::open(&path, &key)
        .and_then(History::open)
        .map_err(|err| match err {
            Error::WrongKey => "the Keychain key does not open the history".to_owned(),
            other => other.to_string(),
        })
}

/// Deletes the history file and starts again with a new key.
pub fn reset(data_dir: &Path, service: &str) -> Result<History, String> {
    let path = data_dir.join(DB_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("could not delete the old history: {err}")),
    }
    let key = keychain::create_key(service).map_err(|err| err.to_string())?;
    Db::open(&path, &key)
        .and_then(History::open)
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: Key = [7; 32];

    #[test]
    fn first_run_creates_a_key() {
        assert_eq!(plan_key(None, false), KeyPlan::Create);
    }

    #[test]
    fn stored_key_is_used_with_or_without_a_database() {
        assert_eq!(plan_key(Some(KEY), true), KeyPlan::Use(KEY));
        assert_eq!(plan_key(Some(KEY), false), KeyPlan::Use(KEY));
    }

    #[test]
    fn database_without_a_key_is_not_silently_replaced() {
        assert_eq!(plan_key(None, true), KeyPlan::Missing);
    }
}
