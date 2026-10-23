//! Atomic JSON file writing and tolerant reading.

use crate::error::{CoreError, Result};
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

/// Writes `value` to `path` atomically.
///
/// Data goes to `<path>.<pid>.<n>.tmp`, is fsynced, then renamed
/// over `path`. Pretty output matches Python's `json.dump(indent=2)` plus a
/// trailing newline; compact output matches `separators=(',', ':')`.
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

    // A name of its own per write: two writers (threads or processes) must
    // never truncate each other's temporary file.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp_name: OsString = path.as_os_str().to_owned();
    tmp_name.push(format!(".{}.{n}.tmp", std::process::id()));
    let tmp = Path::new(&tmp_name);

    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(tmp, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(tmp);
        return Err(e.into());
    }
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
