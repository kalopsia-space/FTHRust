//! The private directory a disk-backed replay buffer writes into:
//! `$XDG_CACHE_HOME/fthrust/replay/<pid>/` (`~/.cache/...` when unset).
//! Each engine owns the directory named after its pid, removes it on exit,
//! and on start removes directories whose engine is gone (a crash).

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The parent of every engine's replay directory.
pub fn replay_root() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    cache.join("fthrust").join("replay")
}

/// This engine's directory under `root`.
pub fn own_dir(root: &Path) -> PathBuf {
    root.join(std::process::id().to_string())
}

/// Creates this engine's directory under `root` (mode `0700`, as are the
/// `fthrust/replay` parents it creates) and returns it.
pub fn create_own_in(root: &Path) -> io::Result<PathBuf> {
    DirBuilder::new().recursive(true).mode(0o700).create(root)?;
    let dir = own_dir(root);
    DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// Removes the pid-named directories under `root` whose process is gone.
/// Entries that are not pid-named are left alone.
pub fn remove_stale(root: &Path, is_alive: impl Fn(u32) -> bool) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        if !is_alive(pid) && entry.path().is_dir() {
            match fs::remove_dir_all(entry.path()) {
                Ok(()) => tracing::info!(pid, "removed a replay buffer left by a stopped engine"),
                Err(e) => tracing::warn!(pid, error = %e, "could not remove a stale replay buffer"),
            }
        }
    }
}

/// Removes this engine's directory under `root`.
pub fn remove_own(root: &Path) {
    let dir = own_dir(root);
    if dir.exists()
        && let Err(e) = fs::remove_dir_all(&dir)
    {
        tracing::warn!(error = %e, "could not remove the replay buffer directory");
    }
}

/// Whether a process with `pid` exists.
pub fn pid_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

/// Whether `path` is on tmpfs (where a disk buffer saves no memory).
pub fn on_tmpfs(path: &Path) -> bool {
    nix::sys::statfs::statfs(path)
        .is_ok_and(|s| s.filesystem_type() == nix::sys::statfs::TMPFS_MAGIC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_replay_dirs_are_removed() {
        let root = tempfile::tempdir().unwrap();
        for name in ["1", "2", "notapid"] {
            fs::create_dir(root.path().join(name)).unwrap();
            fs::write(root.path().join(name).join("00.seg"), b"x").unwrap();
        }
        remove_stale(root.path(), |pid| pid == 2);
        assert!(!root.path().join("1").exists());
        assert!(root.path().join("2").exists());
        assert!(root.path().join("notapid").exists());
    }

    #[test]
    fn own_replay_dir_is_private() {
        let root = tempfile::tempdir().unwrap();
        let replay = root.path().join("fthrust").join("replay");
        let dir = create_own_in(&replay).unwrap();
        assert_eq!(dir, own_dir(&replay));
        for p in [&dir, &replay] {
            let mode = fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", p.display());
        }
        remove_own(&replay);
        assert!(!dir.exists());
    }

    #[test]
    fn this_process_is_alive() {
        assert!(pid_alive(std::process::id()));
    }
}
