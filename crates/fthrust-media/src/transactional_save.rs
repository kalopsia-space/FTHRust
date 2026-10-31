//! Crash-safe publication of a finished clip.
//!
//! Ports `reference/FTHRcapture_common/transactional_save.h`: the writer
//! produces `<final>.partial`, the file is fsynced, renamed into place with
//! no-replace semantics, and the directory is fsynced afterwards. A save is
//! refused when the final name or a leftover partial already exists.
//! `renameat2(RENAME_NOREPLACE)` falls back to `link` + `unlink` on
//! filesystems that reject the flag (the reference failed there outright),
//! and to a checked plain rename where hard links are missing too
//! ([`fthrust_core::clip_files::rename_no_replace`]). A clip that is written
//! but cannot take its name is never deleted: it gets a visible
//! `<stem>_unpublished` name, or stays as its `.partial`, which the startup
//! sweep publishes later.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Suffix of the temporary file a save writes before publication.
pub const PARTIAL_SUFFIX: &str = ".partial";

/// `<final>.partial`.
pub fn temporary_path_for(final_path: &Path) -> PathBuf {
    let mut name = final_path.as_os_str().to_owned();
    name.push(PARTIAL_SUFFIX);
    PathBuf::from(name)
}

/// `<stem>_unpublished.<ext>`, then `_unpublished_2` …: the visible name a
/// finished clip takes when its own could not be had.
pub fn unpublished_path_for(final_path: &Path, attempt: u32) -> PathBuf {
    let stem = final_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = final_path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let tag = if attempt <= 1 {
        "_unpublished".to_owned()
    } else {
        format!("_unpublished_{attempt}")
    };
    final_path.with_file_name(format!("{stem}{tag}{ext}"))
}

/// Which step of a save failed.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Failure {
    /// The final path could not be inspected.
    InspectDestination,
    /// The final file already exists.
    DestinationExists,
    /// The temporary path could not be inspected.
    InspectTemporary,
    /// A leftover temporary file exists.
    TemporaryExists,
    /// The writer failed, or the file could not be flushed to disk.
    Writer,
    /// The no-replace rename failed.
    Rename,
}

/// A failed save, with the reference's description as its `Display`.
#[derive(Debug)]
pub struct SaveError {
    /// Which step failed.
    pub failure: Failure,
    /// The intended final path.
    pub final_path: PathBuf,
    /// The temporary path.
    pub temporary_path: PathBuf,
    /// The writer's message, or the flush error text.
    pub writer_error: String,
    /// The I/O error of the failed step, when there was one.
    pub operation_error: Option<io::Error>,
    /// An error removing the temporary file after the failure.
    pub cleanup_error: Option<io::Error>,
    /// Where a finished clip that could not take its name is: an
    /// `_unpublished` name next to it, or the temporary path. A written and
    /// flushed clip is never deleted.
    pub kept_as: Option<PathBuf>,
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let final_path = self.final_path.display();
        let temporary = self.temporary_path.display();
        let op = || {
            self.operation_error
                .as_ref()
                .map_or_else(|| "io error".to_string(), ToString::to_string)
        };
        match self.failure {
            Failure::InspectDestination => {
                write!(
                    f,
                    "Could not inspect final clip path '{final_path}': {}",
                    op()
                )
            }
            Failure::DestinationExists => {
                write!(
                    f,
                    "Final clip already exists and was not overwritten: '{final_path}'"
                )
            }
            Failure::InspectTemporary => {
                write!(
                    f,
                    "Could not inspect temporary clip path '{temporary}': {}",
                    op()
                )
            }
            Failure::TemporaryExists => {
                write!(
                    f,
                    "Temporary clip already exists and was not overwritten: '{temporary}'"
                )
            }
            Failure::Writer => {
                if self.writer_error.is_empty() {
                    write!(
                        f,
                        "Media writer failed while creating temporary clip '{temporary}'"
                    )
                } else {
                    f.write_str(&self.writer_error)
                }
            }
            Failure::Rename => {
                write!(
                    f,
                    "Could not atomically finalize clip '{final_path}' from '{temporary}': {}",
                    op()
                )?;
                match &self.kept_as {
                    Some(kept) if *kept == self.temporary_path => {
                        write!(f, ". The finished clip was kept as '{temporary}'.")
                    }
                    Some(kept) => write!(
                        f,
                        ". The finished clip was saved as '{}' instead.",
                        kept.display()
                    ),
                    None => Ok(()),
                }
            }
        }
    }
}

impl std::error::Error for SaveError {}

/// Filesystem operations a save needs; injectable for tests.
pub trait SaveFs {
    /// Whether `path` exists.
    fn exists(&self, path: &Path) -> io::Result<bool>;
    /// Renames `from` to `to`, failing if `to` exists.
    fn rename_no_replace(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Removes a file.
    fn remove(&self, path: &Path) -> io::Result<()>;
    /// Flushes a file's data to disk.
    fn sync_file(&self, path: &Path) -> io::Result<()>;
    /// Flushes a directory's entries to disk.
    fn sync_directory(&self, path: &Path) -> io::Result<()>;
}

/// The real filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealFs;

impl RealFs {
    /// No-replace move by hard link plus unlink, for filesystems that reject
    /// `RENAME_NOREPLACE` ([`fthrust_core::clip_files::link_then_unlink`]).
    pub fn link_then_unlink(from: &Path, to: &Path) -> io::Result<()> {
        fthrust_core::clip_files::link_then_unlink(from, to)
    }

    /// The move for filesystems without `RENAME_NOREPLACE`, with `link` in
    /// place of the hard link (tests):
    /// [`fthrust_core::clip_files::publish_without_noreplace`].
    #[doc(hidden)]
    pub fn publish_without_noreplace(
        from: &Path,
        to: &Path,
        link: impl FnOnce(&Path, &Path) -> io::Result<()>,
    ) -> io::Result<()> {
        fthrust_core::clip_files::publish_without_noreplace(from, to, link)
    }
}

impl SaveFs for RealFs {
    fn exists(&self, path: &Path) -> io::Result<bool> {
        match fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn rename_no_replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        fthrust_core::clip_files::rename_no_replace(from, to)
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    fn sync_file(&self, path: &Path) -> io::Result<()> {
        File::open(path)?.sync_all()
    }

    fn sync_directory(&self, path: &Path) -> io::Result<()> {
        let dir = if path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            path
        };
        OpenOptions::new().read(true).open(dir)?.sync_all()
    }
}

/// Runs a save on the real filesystem: see [`run_with`].
pub fn run<W>(final_path: &Path, writer: W) -> Result<PathBuf, SaveError>
where
    W: FnOnce(&Path) -> Result<(), String>,
{
    run_with(&RealFs, final_path, writer)
}

/// Runs a save: refuses existing final or partial files, calls `writer`
/// with the temporary path, fsyncs it, renames it into place with
/// no-replace semantics and fsyncs the directory (a directory sync failure
/// is tolerated). A writer or flush failure removes the temporary file. A
/// failed rename never does, since it is then the finished clip: it is
/// moved to a visible `<stem>_unpublished` name if one can be had, else
/// kept, and the error says which name it has.
pub fn run_with<W>(fs: &dyn SaveFs, final_path: &Path, writer: W) -> Result<PathBuf, SaveError>
where
    W: FnOnce(&Path) -> Result<(), String>,
{
    let temporary_path = temporary_path_for(final_path);
    let fail =
        |failure: Failure, writer_error: String, operation_error: Option<io::Error>| SaveError {
            failure,
            final_path: final_path.to_path_buf(),
            temporary_path: temporary_path.clone(),
            writer_error,
            operation_error,
            cleanup_error: None,
            kept_as: None,
        };
    match fs.exists(final_path) {
        Ok(true) => return Err(fail(Failure::DestinationExists, String::new(), None)),
        Ok(false) => {}
        Err(e) => return Err(fail(Failure::InspectDestination, String::new(), Some(e))),
    }
    match fs.exists(&temporary_path) {
        Ok(true) => return Err(fail(Failure::TemporaryExists, String::new(), None)),
        Ok(false) => {}
        Err(e) => return Err(fail(Failure::InspectTemporary, String::new(), Some(e))),
    }

    let cleanup = |mut err: SaveError| {
        if let Err(e) = fs.remove(&temporary_path) {
            err.cleanup_error = Some(e);
        }
        err
    };

    if let Err(message) = writer(&temporary_path) {
        return Err(cleanup(fail(Failure::Writer, message, None)));
    }
    if let Err(e) = fs.sync_file(&temporary_path) {
        return Err(cleanup(fail(
            Failure::Writer,
            format!("Could not flush the temporary clip to disk: {e}"),
            Some(e),
        )));
    }
    if let Err(e) = fs.rename_no_replace(&temporary_path, final_path) {
        // The temporary file is the finished, flushed clip and may be the
        // only copy: never delete it.
        let mut err = fail(Failure::Rename, String::new(), Some(e));
        err.kept_as = keep_finished(fs, &temporary_path, final_path);
        return Err(err);
    }
    sync_directory_of(fs, final_path);
    Ok(final_path.to_path_buf())
}

/// Where a finished clip that could not take its own name goes: a free
/// visible `_unpublished` name next to it, else it stays at `temporary`
/// (`None` when that is gone too).
fn keep_finished(fs: &dyn SaveFs, temporary: &Path, final_path: &Path) -> Option<PathBuf> {
    for attempt in 1..=3 {
        let beside = unpublished_path_for(final_path, attempt);
        match fs.rename_no_replace(temporary, &beside) {
            Ok(()) => {
                sync_directory_of(fs, &beside);
                return Some(beside);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => break,
        }
    }
    fs.exists(temporary)
        .unwrap_or(true)
        .then(|| temporary.to_path_buf())
}

/// Fsyncs the directory holding `path`; a failure is only logged.
fn sync_directory_of(fs: &dyn SaveFs, path: &Path) {
    let parent = path.parent().unwrap_or(Path::new(""));
    if let Err(e) = fs.sync_directory(parent) {
        tracing::warn!(
            "directory sync after publishing {} failed: {e}",
            path.display()
        );
    }
}
