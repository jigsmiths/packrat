//! Clipboard history storage, search and pasteboard access for Packrat.

pub mod db;
pub mod history;
pub mod keychain;
pub mod search;

/// Errors returned by this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("clip not found")]
    NotFound,
    #[error("wrong database key, or the file is not a Packrat database")]
    WrongKey,
    #[error("invalid TTL {value:?}: {reason}")]
    InvalidTtl { value: String, reason: String },
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("keychain: {0}")]
    Keychain(#[from] security_framework::base::Error),
    #[error("keychain: stored key has {0} bytes, expected 32")]
    KeyLength(usize),
    #[error("random number generator: {0}")]
    Random(#[from] getrandom::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
