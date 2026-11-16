//! The database key, stored as a generic password in the login Keychain.

use security_framework::passwords::{get_generic_password, set_generic_password};

use crate::db::Key;
use crate::{Error, Result};

const ACCOUNT: &str = "db-key";
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// Returns the stored key, or `None` when there is none yet.
pub fn load_key(service: &str) -> Result<Option<Key>> {
    match get_generic_password(service, ACCOUNT) {
        Ok(bytes) => {
            let len = bytes.len();
            let key = Key::try_from(bytes).map_err(|_| Error::KeyLength(len))?;
            Ok(Some(key))
        }
        Err(err) if err.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Generates a random key and stores it, replacing any existing one.
pub fn create_key(service: &str) -> Result<Key> {
    let mut key = Key::default();
    getrandom::fill(&mut key)?;
    set_generic_password(service, ACCOUNT, &key)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use security_framework::passwords::delete_generic_password;

    use super::*;

    #[test]
    #[ignore = "writes to the login Keychain, run by hand with --ignored"]
    fn key_round_trip() {
        let service = "com.jigsmiths.packrat.test";
        let _ = delete_generic_password(service, ACCOUNT);

        assert_eq!(load_key(service).unwrap(), None);
        let created = create_key(service).unwrap();
        assert_ne!(created, Key::default());
        assert_eq!(load_key(service).unwrap(), Some(created));

        delete_generic_password(service, ACCOUNT).unwrap();
    }
}
