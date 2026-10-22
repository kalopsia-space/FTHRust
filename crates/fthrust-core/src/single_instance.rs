//! One UI at a time (`core/single_instance.py`, `core/instance_activation.py`):
//! an exclusive `flock` on `~/.fthr/fthr.lock` (fail-open when the lock
//! cannot be taken for any reason but "held by another instance") and the
//! one-way `activate` message a second launch sends to the owner.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use nix::fcntl::{Flock, FlockArg};

/// The activation message.
pub const ACTIVATE: &[u8] = b"activate\n";

/// `<runtime>/instance.sock`.
pub fn activation_socket_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("instance.sock")
}

/// The held lock; dropping it releases the instance slot.
#[derive(Debug)]
pub struct InstanceLock {
    _lock: Flock<File>,
}

/// The result of trying to become the instance.
#[derive(Debug)]
pub enum LockResult {
    /// This process owns the slot.
    Acquired(InstanceLock),
    /// Another instance holds it.
    Held,
    /// The lock file could not be used (fail-open: start anyway).
    Unavailable,
}

impl InstanceLock {
    /// Takes the exclusive lock on `lock_file`, creating it (mode 0600).
    pub fn acquire(lock_file: &Path) -> LockResult {
        if let Some(parent) = lock_file.parent()
            && std::fs::create_dir_all(parent).is_err()
        {
            return LockResult::Unavailable;
        }
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_file)
        {
            Ok(f) => f,
            Err(_) => return LockResult::Unavailable,
        };
        let _ = std::fs::set_permissions(
            lock_file,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        );
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(mut lock) => {
                // Diagnostic only; the flock is the lock.
                let _ = lock.set_len(0);
                let _ = lock.write_all(std::process::id().to_string().as_bytes());
                LockResult::Acquired(InstanceLock { _lock: lock })
            }
            Err((_, nix::errno::Errno::EWOULDBLOCK)) => LockResult::Held,
            Err(_) => LockResult::Unavailable,
        }
    }
}
