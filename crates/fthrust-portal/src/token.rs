//! The ScreenCast restore token at `~/.fthr/portal_screencast_token`
//! (mode 0600, written through a temporary file). Ports the token helpers
//! of `portal_screencast.cpp`.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use fthrust_core::FthrPaths;

/// Where the token lives.
pub fn path(paths: &FthrPaths) -> PathBuf {
    paths.root().join("portal_screencast_token")
}

/// Why a token could not be loaded.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    /// The file exists but holds whitespace, control characters or nothing.
    #[error("the portal restore token file is damaged")]
    Damaged,
    /// The file could not be read.
    #[error("could not read the portal restore token: {0}")]
    Io(String),
}

/// Loads the token; `Ok(None)` when there is no file.
pub fn load(path: &Path) -> Result<Option<String>, TokenError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(TokenError::Io(e.to_string())),
    };
    if text.is_empty() || text.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(TokenError::Damaged);
    }
    Ok(Some(text))
}

/// Saves the token (temporary file + fsync + rename, mode 0600); an empty
/// token deletes the file.
pub fn save(path: &Path, token: &str) -> std::io::Result<()> {
    if token.is_empty() {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)
}
