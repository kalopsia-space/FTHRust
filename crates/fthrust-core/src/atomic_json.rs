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
    fsutil::write_atomic(path, &bytes, fsutil::private_mode_for(path))?;
    Ok(())
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
