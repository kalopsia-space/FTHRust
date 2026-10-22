//! Error type shared by every `fthrust-core` module.

use std::path::PathBuf;

/// Errors produced by `fthrust-core`.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// An underlying I/O operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A JSON file could not be parsed or serialised.
    #[error("JSON error in {path}: {source}")]
    Json {
        /// File that failed to parse or serialise.
        path: PathBuf,
        /// Underlying serde_json error.
        source: serde_json::Error,
    },
    /// A file was unreadable and has been moved aside.
    #[error("corrupt file {path} quarantined as {quarantined}")]
    Corrupt {
        /// Original path of the corrupt file.
        path: PathBuf,
        /// Path the corrupt file was renamed to.
        quarantined: PathBuf,
    },
    /// The private runtime directory is missing or unsafe to use.
    #[error("unsafe runtime directory: {0}")]
    RuntimeDir(String),
    /// A SQLite operation failed.
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A value failed validation.
    #[error("invalid value: {0}")]
    Invalid(String),
}

/// Result alias using [`CoreError`].
pub type Result<T> = std::result::Result<T, CoreError>;
