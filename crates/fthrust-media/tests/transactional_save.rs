use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fthrust_media::transactional_save::{
    Failure, RealFs, SaveFs, run, run_with, temporary_path_for,
};

#[derive(Default)]
struct Fake {
    final_exists: bool,
    temporary_exists: bool,
    rename_fails: bool,
    cleanup_fails: bool,
    sync_file_fails: bool,
    einval_once: bool,
    /// Only renames onto the final name fail.
    final_rename_fails: bool,
    /// Where a rename to another name than the final one put the clip.
    published_as: Option<String>,
    ops: Vec<String>,
}

struct FakeFs(RefCell<Fake>);

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

impl SaveFs for FakeFs {
    fn exists(&self, path: &Path) -> io::Result<bool> {
        let mut f = self.0.borrow_mut();
        f.ops.push(format!("exists:{}", name(path)));
        Ok(if path.to_string_lossy().ends_with(".partial") {
            f.temporary_exists
        } else {
            f.final_exists
        })
    }
    fn rename_no_replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        let mut f = self.0.borrow_mut();
        f.ops.push(format!("rename:{}->{}", name(from), name(to)));
        if f.einval_once {
            f.einval_once = false;
            return Err(io::Error::from_raw_os_error(22));
        }
        let to_final = name(to) == "clip.mp4";
        let taken = to_final && f.final_exists;
        if f.rename_fails || taken || (to_final && f.final_rename_fails) || !f.temporary_exists {
            return Err(io::Error::new(
                if taken {
                    io::ErrorKind::AlreadyExists
                } else {
                    io::ErrorKind::Other
                },
                "rename failed",
            ));
        }
        f.temporary_exists = false;
        if to_final {
            f.final_exists = true;
        } else {
            f.published_as = Some(name(to));
        }
        Ok(())
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        let mut f = self.0.borrow_mut();
        f.ops.push(format!("remove:{}", name(path)));
        if f.cleanup_fails {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "permission denied",
            ));
        }
        f.temporary_exists = false;
        Ok(())
    }
    fn sync_file(&self, path: &Path) -> io::Result<()> {
        let mut f = self.0.borrow_mut();
        f.ops.push(format!("sync_file:{}", name(path)));
        if f.sync_file_fails {
            return Err(io::Error::other("io error"));
        }
        Ok(())
    }
    fn sync_directory(&self, path: &Path) -> io::Result<()> {
        self.0
            .borrow_mut()
            .ops
            .push(format!("sync_dir:{}", path.display()));
        Ok(())
    }
}

fn fake(setup: impl FnOnce(&mut Fake)) -> FakeFs {
    let mut f = Fake::default();
    setup(&mut f);
    FakeFs(RefCell::new(f))
}

#[test]
fn temporary_path_appends_partial() {
    assert_eq!(
        temporary_path_for(Path::new("/a/clip.mp4")),
        PathBuf::from("/a/clip.mp4.partial")
    );
}

#[test]
fn success_path() {
    let fs = fake(|_| {});
    let result = run_with(&fs, Path::new("clip.mp4"), |tmp| {
        assert_eq!(name(tmp), "clip.mp4.partial");
        let mut f = fs.0.borrow_mut();
        assert!(!f.final_exists);
        f.ops.push("write:clip.mp4.partial".into());
        f.temporary_exists = true;
        Ok(())
    });
    assert_eq!(result.unwrap(), PathBuf::from("clip.mp4"));
    let f = fs.0.borrow();
    assert!(f.final_exists && !f.temporary_exists);
    let n = f.ops.len();
    assert_eq!(f.ops[n - 3], "sync_file:clip.mp4.partial");
    assert_eq!(f.ops[n - 2], "rename:clip.mp4.partial->clip.mp4");
    assert!(f.ops[n - 1].starts_with("sync_dir:"));
}

#[test]
fn writer_failure_cleans_temp() {
    let fs = fake(|_| {});
    let err = run_with(&fs, Path::new("clip.mp4"), |_| {
        fs.0.borrow_mut().temporary_exists = true;
        Err("packet write failed".into())
    })
    .unwrap_err();
    assert_eq!(err.failure, Failure::Writer);
    assert_eq!(err.to_string(), "packet write failed");
    let f = fs.0.borrow();
    assert!(!f.final_exists && !f.temporary_exists);
}

#[test]
fn rename_failure_keeps_the_finished_clip() {
    let fs = fake(|f| f.rename_fails = true);
    let err = run_with(&fs, Path::new("clip.mp4"), |_| {
        fs.0.borrow_mut().temporary_exists = true;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(err.failure, Failure::Rename);
    let text = err.to_string();
    assert!(
        text.starts_with("Could not atomically finalize clip 'clip.mp4' from 'clip.mp4.partial': "),
        "{text}"
    );
    assert!(
        text.ends_with("The finished clip was kept as 'clip.mp4.partial'."),
        "{text}"
    );
    let f = fs.0.borrow();
    assert!(
        f.temporary_exists,
        "the written and flushed partial is the only copy"
    );
    assert!(!f.final_exists);
    assert!(
        !f.ops.iter().any(|op| op.starts_with("remove:")),
        "{:?}",
        f.ops
    );
}

#[test]
fn cleanup_failure_preserves_original_error() {
    let fs = fake(|f| f.cleanup_fails = true);
    let err = run_with(&fs, Path::new("clip.mp4"), |_| {
        fs.0.borrow_mut().temporary_exists = true;
        Err("disk full".into())
    })
    .unwrap_err();
    assert_eq!(err.to_string(), "disk full");
    assert_eq!(
        err.cleanup_error.as_ref().unwrap().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert!(fs.0.borrow().temporary_exists);
}

#[test]
fn existing_destination_untouched() {
    let fs = fake(|f| f.final_exists = true);
    let mut called = false;
    let err = run_with(&fs, Path::new("clip.mp4"), |_| {
        called = true;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(err.failure, Failure::DestinationExists);
    assert_eq!(
        err.to_string(),
        "Final clip already exists and was not overwritten: 'clip.mp4'"
    );
    assert!(!called);
    assert!(fs.0.borrow().final_exists);
}

#[test]
fn existing_partial_refused() {
    let fs = fake(|f| f.temporary_exists = true);
    let mut called = false;
    let err = run_with(&fs, Path::new("clip.mp4"), |_| {
        called = true;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(err.failure, Failure::TemporaryExists);
    assert_eq!(
        err.to_string(),
        "Temporary clip already exists and was not overwritten: 'clip.mp4.partial'"
    );
    assert!(
        !called,
        "a leftover partial from a crashed save is never overwritten"
    );
}

#[test]
fn missing_temporary_cannot_commit() {
    let fs = fake(|_| {});
    let err = run_with(&fs, Path::new("clip.mp4"), |_| Ok(())).unwrap_err();
    assert_eq!(err.failure, Failure::Rename);
    assert!(!err.to_string().contains("kept"), "{err}");
    assert!(!err.to_string().contains("saved as"), "{err}");
    assert!(!fs.0.borrow().final_exists);
}

#[test]
fn a_failed_publication_gives_the_clip_a_visible_name() {
    let fs = fake(|f| f.final_rename_fails = true);
    let err = run_with(&fs, Path::new("clip.mp4"), |_| {
        fs.0.borrow_mut().temporary_exists = true;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(err.failure, Failure::Rename);
    let text = err.to_string();
    assert!(
        text.ends_with("The finished clip was saved as 'clip_unpublished.mp4' instead."),
        "{text}"
    );
    let f = fs.0.borrow();
    assert_eq!(f.published_as.as_deref(), Some("clip_unpublished.mp4"));
    assert!(!f.temporary_exists);
    assert!(
        !f.ops.iter().any(|op| op.starts_with("remove:")),
        "{:?}",
        f.ops
    );
}

#[test]
fn a_clip_whose_name_was_taken_meanwhile_is_saved_beside_it() {
    let tmp = tempfile::tempdir().unwrap();
    let final_path = tmp.path().join("clip.mp4");
    let err = run(&final_path, |partial| {
        fs::write(partial, b"the clip").map_err(|e| e.to_string())?;
        // Something else takes the name while the clip is written.
        fs::write(&final_path, b"someone else").map_err(|e| e.to_string())
    })
    .unwrap_err();
    assert_eq!(err.failure, Failure::Rename);
    assert_eq!(fs::read(&final_path).unwrap(), b"someone else");
    let beside = tmp.path().join("clip_unpublished.mp4");
    assert_eq!(fs::read(&beside).unwrap(), b"the clip");
    assert!(!temporary_path_for(&final_path).exists());
    assert!(err.to_string().contains("clip_unpublished.mp4"), "{err}");
}

#[test]
fn without_noreplace_or_hard_links_the_clip_is_still_published() {
    // Some FUSE mounts (gvfs, rclone, exfat-fuse) have neither
    // RENAME_NOREPLACE nor hard links.
    let tmp = tempfile::tempdir().unwrap();
    let from = tmp.path().join("a.mp4.partial");
    let to = tmp.path().join("a.mp4");
    fs::write(&from, b"data").unwrap();
    let no_links = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(libc::EPERM));
    RealFs::publish_without_noreplace(&from, &to, no_links).unwrap();
    assert!(!from.exists() && fs::read(&to).unwrap() == b"data");

    fs::write(&from, b"other").unwrap();
    let err = RealFs::publish_without_noreplace(&from, &to, no_links).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&to).unwrap(), b"data", "never replaced");
    assert_eq!(fs::read(&from).unwrap(), b"other", "never removed");

    // Any other link error is the answer: nothing is renamed.
    fs::remove_file(&to).unwrap();
    let broken = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(libc::EIO));
    let err = RealFs::publish_without_noreplace(&from, &to, broken).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::EIO));
    assert!(from.exists() && !to.exists());
}

#[test]
fn file_fsync_before_rename_dir_fsync_after() {
    let fs = fake(|_| {});
    run_with(&fs, Path::new("dir/clip.mp4"), |_| {
        fs.0.borrow_mut().temporary_exists = true;
        Ok(())
    })
    .unwrap();
    let f = fs.0.borrow();
    let n = f.ops.len();
    assert_eq!(f.ops[n - 3], "sync_file:clip.mp4.partial");
    assert_eq!(f.ops[n - 2], "rename:clip.mp4.partial->clip.mp4");
    assert_eq!(f.ops[n - 1], "sync_dir:dir");
}

#[test]
fn sync_failure_reported_as_writer_failure() {
    let fs = fake(|f| f.sync_file_fails = true);
    let err = run_with(&fs, Path::new("clip.mp4"), |_| {
        fs.0.borrow_mut().temporary_exists = true;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(err.failure, Failure::Writer);
    assert!(
        err.to_string()
            .starts_with("Could not flush the temporary clip to disk: ")
    );
    let f = fs.0.borrow();
    assert!(!f.final_exists && !f.temporary_exists);
}

#[test]
fn noreplace_fallback_on_einval() {
    // The real filesystem adapter falls back to link + unlink when the
    // kernel or filesystem rejects RENAME_NOREPLACE; prove the fallback is
    // itself no-replace and leaves exactly one name behind.
    let tmp = tempfile::tempdir().unwrap();
    let from = tmp.path().join("a.partial");
    let to = tmp.path().join("a.mp4");
    fs::write(&from, b"data").unwrap();
    RealFs::link_then_unlink(&from, &to).unwrap();
    assert!(!from.exists() && fs::read(&to).unwrap() == b"data");
    fs::write(&from, b"other").unwrap();
    let err = RealFs::link_then_unlink(&from, &to).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(
        fs::read(&to).unwrap(),
        b"data",
        "existing target is never replaced"
    );
    assert!(from.exists(), "source stays when the fallback refuses");
}

#[test]
fn default_filesystem_commit_and_preserve_existing() {
    let tmp = tempfile::tempdir().unwrap();
    let final_path = tmp.path().join("clip.mp4");
    let written = run(&final_path, |tmp_path| {
        fs::write(tmp_path, b"complete").map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(written, final_path);
    assert_eq!(fs::read(&final_path).unwrap(), b"complete");
    assert!(!temporary_path_for(&final_path).exists());

    let mut called = false;
    let err = run(&final_path, |_| {
        called = true;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(err.failure, Failure::DestinationExists);
    assert!(!called);
    assert_eq!(fs::read(&final_path).unwrap(), b"complete");
}
