//! Conservative ownership rules for destructive library actions.

use crate::clip_files::audio_manifest_sidecar;
use crate::error::{CoreError, Result};
use std::path::{Component, Path, PathBuf};

/// Who owns a media file for deletion purposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ownership {
    /// Inside the FTHR clips root: may be deleted.
    FthrOwned,
    /// Inside a user-imported folder: protected.
    LinkedImported,
    /// Anywhere else: protected.
    External,
}

fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn resolve(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| lexical(p))
}

/// Classify `p` after resolving symlinks; imported roots win over the owned root.
pub fn classify_media_path(p: &Path, owned_root: &Path, imported_roots: &[PathBuf]) -> Ownership {
    let path = resolve(p);
    if imported_roots.iter().any(|r| path.starts_with(resolve(r))) {
        Ownership::LinkedImported
    } else if path.starts_with(resolve(owned_root)) {
        Ownership::FthrOwned
    } else {
        Ownership::External
    }
}

/// Delete `p` and its audio manifest sidecar, but only if it is FTHR-owned.
pub fn delete_owned_media(p: &Path, owned_root: &Path, imported_roots: &[PathBuf]) -> Result<()> {
    if classify_media_path(p, owned_root, imported_roots) != Ownership::FthrOwned {
        return Err(CoreError::Invalid(
            "Linked or external media is protected".into(),
        ));
    }
    std::fs::remove_file(p)?;
    match std::fs::remove_file(audio_manifest_sidecar(p)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Resolve an import root to its canonical absolute form; if resolution
/// fails the lexically normalised path is returned.
pub fn normalize_import_root(p: &Path) -> Result<PathBuf> {
    Ok(resolve(p))
}

/// Add `p` to `list` unless an equivalent resolved path is present.
/// Returns whether it was added.
pub fn add_import_root(list: &mut Vec<PathBuf>, p: &Path) -> bool {
    let resolved = resolve(p);
    if list.iter().any(|r| resolve(r) == resolved) {
        return false;
    }
    list.push(resolved);
    true
}

/// Remove the entry equal to `p` (by resolved path) from `list`. Returns
/// whether something was removed. Never touches files.
pub fn remove_import_root(list: &mut Vec<PathBuf>, p: &Path) -> bool {
    let target = resolve(p);
    let before = list.len();
    list.retain(|r| resolve(r) != target);
    list.len() != before
}
