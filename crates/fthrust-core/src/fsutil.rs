//! Small filesystem helpers shared by the stores: private atomic writes,
//! private directories and setting a damaged file aside.

use std::ffi::OsString;
use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Mode of files that hold settings, credentials or history.
pub(crate) const PRIVATE_FILE_MODE: u32 = 0o600;
/// Mode of directories that hold them.
pub(crate) const PRIVATE_DIR_MODE: u32 = 0o700;

/// The mode a private file is rewritten with: `0600`, or the stricter mode
/// `path` already has (a file made read-only by hand stays read-only).
pub(crate) fn private_mode_for(path: &Path) -> u32 {
    match fs::metadata(path) {
        Ok(meta) => (meta.permissions().mode() & 0o600) | 0o400,
        Err(_) => PRIVATE_FILE_MODE,
    }
}

/// Writes `bytes` to `path` through `<path>.<pid>.<n>.tmp` in the same
/// directory: the temporary file is created exclusively with `mode`, fsynced
/// and renamed over `path`, so a reader sees the old or the new content,
/// never a mix, and the data is never visible with wider access than `mode`.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    // A name of its own per write: two writers (threads or processes) must
    // never truncate each other's temporary file.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut attempts = 0;
    let (mut file, tmp) = loop {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut name: OsString = path.as_os_str().to_owned();
        name.push(format!(".{}.{n}.tmp", std::process::id()));
        let tmp = PathBuf::from(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode & 0o777)
            .open(&tmp)
        {
            Ok(file) => break (file, tmp),
            // A leftover of a dead process that had our pid: take the next name.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempts < 16 => attempts += 1,
            Err(e) => return Err(e),
        }
    };
    let result = (|| -> io::Result<()> {
        // The umask may have narrowed the creation mode; the file gets
        // exactly the requested one.
        file.set_permissions(Permissions::from_mode(mode & 0o777))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Creates `dir` with mode `0700` (parents get the default mode), or removes
/// group and other access from an existing one. Creating can fail; tightening
/// an existing directory that is not ours only logs.
pub(crate) fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::DirBuilder::new().mode(PRIVATE_DIR_MODE).create(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            if !dir.is_dir() {
                return Err(e);
            }
            tighten_dir(dir);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Removes group and other access from an existing directory.
pub(crate) fn tighten_dir(dir: &Path) {
    let Ok(meta) = fs::metadata(dir) else {
        return;
    };
    let mode = meta.permissions().mode();
    if mode & 0o077 != 0
        && let Err(e) = fs::set_permissions(dir, Permissions::from_mode(mode & !0o077))
    {
        tracing::warn!(path = %dir.display(), error = %e, "could not restrict directory access");
    }
}

/// The path a damaged `path` is set aside to: `<path>.corrupt`.
pub(crate) fn corrupt_copy_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".corrupt");
    PathBuf::from(name)
}

/// Moves a damaged file to `<path>.corrupt` (replacing an older one) and
/// returns where it went.
pub(crate) fn quarantine(path: &Path) -> io::Result<PathBuf> {
    let target = corrupt_copy_path(path);
    fs::rename(path, &target)?;
    Ok(target)
}

/// Copies a damaged file to `<path>.corrupt` (replacing an older one) so the
/// original can then be overwritten, and returns where the copy went.
pub(crate) fn keep_copy(path: &Path) -> io::Result<PathBuf> {
    let target = corrupt_copy_path(path);
    fs::copy(path, &target)?;
    Ok(target)
}
