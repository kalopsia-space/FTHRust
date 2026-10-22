//! Cheap identities for disposable, in-memory media inspection caches.

use crate::error::Result;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Replacement-aware identity of a file: path, size, timestamps and inode.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub struct FileIdentity {
    /// Canonicalised path.
    pub real_path: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// Modification time in nanoseconds since the epoch.
    pub mtime_ns: i128,
    /// Inode change time in nanoseconds since the epoch.
    pub ctime_ns: i128,
    /// Inode number.
    pub ino: u64,
}

/// Compute the [`FileIdentity`] of `p`, following symlinks.
pub fn file_identity(p: &Path) -> Result<FileIdentity> {
    let real_path = std::fs::canonicalize(p)?;
    let m = std::fs::metadata(&real_path)?;
    Ok(FileIdentity {
        real_path,
        size: m.size(),
        mtime_ns: i128::from(m.mtime()) * 1_000_000_000 + i128::from(m.mtime_nsec()),
        ctime_ns: i128::from(m.ctime()) * 1_000_000_000 + i128::from(m.ctime_nsec()),
        ino: m.ino(),
    })
}
