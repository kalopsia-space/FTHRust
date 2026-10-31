//! Atomic JSON file writing and tolerant reading.

use crate::error::{CoreError, Result};
use crate::fsutil;
use std::path::Path;

/// Writes `value` to `path` atomically.
///
/// Data goes to `<path>.<pid>.<n>.tmp`, is fsynced, then renamed over
/// `path`. Pretty output matches Python's `json.dump(indent=2)` plus a
/// trailing newline; compact output matches `separators=(',', ':')`.
///
/// The files are private: new ones get mode `0600`, and an existing file
/// keeps its mode when that is stricter (the stores hold upload credentials).
///
/// An existing file that cannot be read is never replaced: the write fails
/// with [`CoreError::Protected`], because the new content was built without
/// it and would silently destroy whatever it held. An existing file that is
/// not JSON is copied to `<path>.corrupt` first.
pub fn write_json_atomic(path: &Path, value: &serde_json::Value, pretty: bool) -> Result<()> {
    let mut bytes = Vec::new();
    let json_err = |source| CoreError::Json {
        path: path.to_path_buf(),
        source,
    };
    if pretty {
        serde_json::to_writer_pretty(&mut bytes, value).map_err(json_err)?;
        bytes.push(b'\n');
    } else {
        bytes = serde_json::to_string(value).map_err(json_err)?.into_bytes();
    }
    guard_existing(path)?;
    fsutil::write_atomic(path, &bytes, fsutil::private_mode_for(path))?;
    Ok(())
}

/// Checks that replacing `path` cannot destroy data that was never read.
fn guard_existing(path: &Path) -> Result<()> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(CoreError::Protected {
                path: path.to_path_buf(),
                reason: format!("it could not be read ({e})"),
            });
        }
    };
    if bytes.iter().all(u8::is_ascii_whitespace)
        || serde_json::from_slice::<serde::de::IgnoredAny>(&bytes).is_ok()
    {
        return Ok(());
    }
    match fsutil::keep_copy(path) {
        Ok(copy) => {
            tracing::warn!(
                "{} is not valid JSON; a copy was kept as {}",
                path.display(),
                copy.display()
            );
            Ok(())
        }
        Err(e) => Err(CoreError::Protected {
            path: path.to_path_buf(),
            reason: format!("it is not valid JSON and a copy could not be kept ({e})"),
        }),
    }
}

/// Reads and parses a JSON file.
///
/// Returns `Ok(None)` when the file does not exist and `Err(CoreError::Json)`
/// when it cannot be parsed.
pub fn read_json(path: &Path) -> Result<Option<serde_json::Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|source| CoreError::Json {
            path: path.to_path_buf(),
            source,
        })
}

/// What loading a store file found.
#[derive(Debug)]
pub(crate) enum Loaded {
    /// There is no file: start from defaults.
    Missing,
    /// The parsed document.
    Value(serde_json::Value),
    /// The file is there but is not JSON (or not UTF-8).
    Corrupt,
    /// The file could not be read at all (permissions, I/O error). Whatever
    /// it holds is unknown, so it must not be replaced.
    Unreadable(std::io::Error),
}

/// Reads a store file, telling a damaged file from one that could not be read.
pub(crate) fn load_json(path: &Path) -> Loaded {
    match read_json(path) {
        Ok(None) => Loaded::Missing,
        Ok(Some(v)) => Loaded::Value(v),
        Err(CoreError::Json { .. }) => Loaded::Corrupt,
        // Invalid UTF-8 comes back as InvalidData: damaged, not unreadable.
        Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::InvalidData => Loaded::Corrupt,
        Err(CoreError::Io(e)) => Loaded::Unreadable(e),
        Err(other) => Loaded::Unreadable(std::io::Error::other(other.to_string())),
    }
}
