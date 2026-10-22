//! Stat-based, cancellable media discovery for the clip library.
//!
//! There is no persisted index: every scan walks the owned clips root and the
//! user-imported roots, never following symlinks, and publishes records in
//! batches so a large library becomes visible before the walk finishes.

use crate::clip_files::{
    IMAGE_SUFFIXES, VIDEO_SUFFIXES, is_fthr_post_processing_dir, is_partial_name,
};
use crate::error::Result;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Number of records published per `on_batch` call.
pub const DEFAULT_BATCH_SIZE: usize = 128;
/// Maximum number of directories reported by [`watched_dirs`].
pub const DEFAULT_MAX_WATCHED_DIRECTORIES: usize = 256;

/// Kind of a library item, derived from its suffix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    /// A finished video clip.
    Video,
    /// A screenshot or other image.
    Image,
}

/// Lightweight metadata for one library item.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryRecord {
    /// Path as discovered under its scan root.
    pub path: PathBuf,
    /// Canonical (symlink-resolved) path used to de-duplicate overlapping roots.
    pub identity: PathBuf,
    /// Indices of the roots containing this file: 0 is the owned root,
    /// `n + 1` is `imported[n]`. Ascending.
    pub root_ids: Vec<usize>,
    /// Whether the file lives under any imported root.
    pub imported: bool,
    /// Video or image.
    pub kind: MediaKind,
    /// Size in bytes.
    pub size: u64,
    /// Modification time in nanoseconds since the epoch.
    pub mtime_ns: i128,
}

/// Roots to scan.
#[derive(Clone, Debug)]
pub struct ScanRoots {
    /// The FTHR-owned clips root (root id 0).
    pub owned: PathBuf,
    /// User-imported roots (root ids 1..).
    pub imported: Vec<PathBuf>,
}

/// Which records to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// Everything.
    All,
    /// Videos only.
    Clips,
    /// Images only.
    Screenshots,
    /// Items under imported roots.
    Imported,
}

/// Ordering of shown records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    /// Most recently modified first.
    Newest,
    /// Least recently modified first.
    Oldest,
    /// Largest first, ties by newest.
    Longest,
}

fn kind_of(name: &str) -> Option<MediaKind> {
    if is_partial_name(name) {
        return None;
    }
    let ext = Path::new(name).extension()?.to_str()?;
    if VIDEO_SUFFIXES.iter().any(|s| ext.eq_ignore_ascii_case(s)) {
        Some(MediaKind::Video)
    } else if IMAGE_SUFFIXES.iter().any(|s| ext.eq_ignore_ascii_case(s)) {
        Some(MediaKind::Image)
    } else {
        None
    }
}

enum Event<'a> {
    Dir(&'a Path),
    File {
        root_id: usize,
        path: &'a Path,
        meta: &'a std::fs::Metadata,
        kind: MediaKind,
    },
}

/// Roots with their original indices and canonical paths. A root whose
/// canonical path equals an earlier one (the owned root always wins) is dropped.
fn effective_roots(roots: &ScanRoots) -> Vec<(usize, &Path, PathBuf)> {
    let mut seen: Vec<PathBuf> = vec![];
    let mut out = vec![];
    for (id, root) in std::iter::once(roots.owned.as_path())
        .chain(roots.imported.iter().map(PathBuf::as_path))
        .enumerate()
    {
        let canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        if seen.contains(&canon) {
            continue;
        }
        seen.push(canon.clone());
        out.push((id, root, canon));
    }
    out
}

/// Depth-first walk of every root without following symlinks. `visit`
/// returns `false` to stop the whole walk.
fn walk(roots: &ScanRoots, visit: &mut dyn FnMut(Event<'_>) -> bool) {
    for (root_id, root, _) in effective_roots(roots) {
        if !root.is_dir() {
            continue;
        }
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if !visit(Event::Dir(&dir)) {
                return;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<_> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            let mut subdirs = vec![];
            for entry in entries {
                let path = entry.path();
                let Ok(meta) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if meta.file_type().is_symlink() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if meta.is_dir() {
                    if !is_fthr_post_processing_dir(&name) {
                        subdirs.push(path);
                    }
                } else if meta.is_file()
                    && let Some(kind) = kind_of(&name)
                    && !visit(Event::File {
                        root_id,
                        path: &path,
                        meta: &meta,
                        kind,
                    })
                {
                    return;
                }
            }
            stack.extend(subdirs.into_iter().rev());
        }
    }
}

fn mtime_ns(meta: &std::fs::Metadata) -> i128 {
    i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec())
}

/// Scan all roots, publishing batches of [`DEFAULT_BATCH_SIZE`] new records
/// through `on_batch` as they are found, and return the complete de-duplicated
/// list. Symlinks, partial files and FTHR post-processing directories are
/// skipped. If `cancel` becomes set, the pending partial batch is dropped,
/// nothing further is published and `Ok(vec![])` is returned.
pub fn scan(
    roots: &ScanRoots,
    cancel: &AtomicBool,
    mut on_batch: impl FnMut(Vec<LibraryRecord>),
) -> Result<Vec<LibraryRecord>> {
    let imported_canon: Vec<PathBuf> = effective_roots(roots)
        .into_iter()
        .filter(|(id, _, _)| *id > 0)
        .map(|(_, _, canon)| canon)
        .collect();
    let mut records: Vec<LibraryRecord> = vec![];
    let mut index: HashMap<PathBuf, usize> = HashMap::new();
    let mut pending: Vec<LibraryRecord> = vec![];
    let cancelled = || cancel.load(Ordering::SeqCst);

    walk(roots, &mut |ev| {
        if cancelled() {
            return false;
        }
        let Event::File {
            root_id,
            path,
            meta,
            kind,
        } = ev
        else {
            return true;
        };
        let Ok(identity) = std::fs::canonicalize(path) else {
            return true;
        };
        if let Some(&i) = index.get(&identity) {
            let rec = &mut records[i];
            if let Err(pos) = rec.root_ids.binary_search(&root_id) {
                rec.root_ids.insert(pos, root_id);
            }
            rec.imported |= root_id > 0;
            return true;
        }
        let imported = root_id > 0 || imported_canon.iter().any(|r| identity.starts_with(r));
        let rec = LibraryRecord {
            path: path.to_path_buf(),
            identity: identity.clone(),
            root_ids: vec![root_id],
            imported,
            kind,
            size: meta.len(),
            mtime_ns: mtime_ns(meta),
        };
        index.insert(identity, records.len());
        records.push(rec.clone());
        pending.push(rec);
        if pending.len() >= DEFAULT_BATCH_SIZE {
            on_batch(std::mem::take(&mut pending));
        }
        true
    });

    if cancelled() {
        return Ok(vec![]);
    }
    if !pending.is_empty() {
        on_batch(pending);
    }
    Ok(records)
}

/// Directories visited by a scan of `roots`, in scan order, capped at
/// [`DEFAULT_MAX_WATCHED_DIRECTORIES`].
pub fn watched_dirs(roots: &ScanRoots) -> Vec<PathBuf> {
    let mut dirs = vec![];
    walk(roots, &mut |ev| {
        if let Event::Dir(d) = ev {
            if dirs.iter().any(|x| x == d) {
                return true;
            }
            dirs.push(d.to_path_buf());
            return dirs.len() < DEFAULT_MAX_WATCHED_DIRECTORIES;
        }
        true
    });
    dirs
}

/// Filter and sort `records` for display.
pub fn apply(records: &[LibraryRecord], f: Filter, s: Sort) -> Vec<LibraryRecord> {
    let mut out: Vec<LibraryRecord> = records
        .iter()
        .filter(|r| match f {
            Filter::All => true,
            Filter::Clips => r.kind == MediaKind::Video,
            Filter::Screenshots => r.kind == MediaKind::Image,
            Filter::Imported => r.imported,
        })
        .cloned()
        .collect();
    match s {
        Sort::Newest => out.sort_by_key(|a| std::cmp::Reverse(a.mtime_ns)),
        Sort::Oldest => out.sort_by_key(|a| a.mtime_ns),
        Sort::Longest => out.sort_by(|a, b| {
            b.size
                .cmp(&a.size)
                .then_with(|| b.mtime_ns.cmp(&a.mtime_ns))
        }),
    }
    out
}
