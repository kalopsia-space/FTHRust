//! One instance at a time: the lock plus the activation socket a second
//! launch writes `activate` to.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fthrust_core::single_instance::{ACTIVATE, InstanceLock, LockResult, activation_socket_path};

/// The timeout for the whole activation request.
pub const ACTIVATE_TIMEOUT: Duration = Duration::from_millis(350);

/// The outcome of [`become_primary`].
pub struct Instance {
    lock: Option<InstanceLock>,
    activation: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    primary: bool,
}

impl Instance {
    /// This process owns the window.
    pub fn is_primary(&self) -> bool {
        self.primary
    }

    /// Whether a second launch asked for the window since the last call.
    pub fn activation_requested(&self) -> bool {
        self.activation.swap(false, Ordering::Relaxed)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.lock.take();
    }
}

/// Takes the lock and listens for activation requests; when another
/// instance holds the lock, sends it `activate` and returns a secondary.
pub fn become_primary(
    lock_file: &Path,
    runtime_dir: &Path,
    ctx: &egui::Context,
) -> Result<Instance, String> {
    match InstanceLock::acquire(lock_file) {
        LockResult::Held => {
            let sent = request_activation(runtime_dir);
            tracing::info!(sent, "another FTHR Clips instance owns the lock");
            Ok(Instance {
                lock: None,
                activation: Arc::new(AtomicBool::new(false)),
                stop: Arc::new(AtomicBool::new(false)),
                thread: None,
                primary: false,
            })
        }
        LockResult::Acquired(lock) => Ok(listen(Some(lock), runtime_dir, ctx)),
        LockResult::Unavailable => {
            tracing::warn!("instance lock unavailable; starting anyway");
            Ok(listen(None, runtime_dir, ctx))
        }
    }
}

fn listen(lock: Option<InstanceLock>, runtime_dir: &Path, ctx: &egui::Context) -> Instance {
    let sock = activation_socket_path(runtime_dir);
    let activation = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let _ = std::fs::remove_file(&sock);
    let thread = match UnixListener::bind(&sock) {
        Ok(listener) => {
            let _ = listener.set_nonblocking(true);
            let (flag, stop_t, ctx) = (activation.clone(), stop.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("fthrust-activation".into())
                .spawn(move || {
                    while !stop_t.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((mut conn, _)) => {
                                let _ = conn.set_read_timeout(Some(ACTIVATE_TIMEOUT));
                                let mut buf = [0u8; 32];
                                if let Ok(n) = conn.read(&mut buf)
                                    && buf[..n].trim_ascii() == ACTIVATE.trim_ascii()
                                {
                                    flag.store(true, Ordering::Relaxed);
                                    ctx.request_repaint();
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(25));
                            }
                            Err(_) => break,
                        }
                    }
                    let _ = std::fs::remove_file(&sock);
                })
                .ok()
        }
        Err(e) => {
            tracing::warn!("activation socket unavailable: {e}");
            None
        }
    };
    Instance {
        lock,
        activation,
        stop,
        thread,
        primary: true,
    }
}

/// Asks the running instance to show its window; never blocks longer
/// than [`ACTIVATE_TIMEOUT`].
pub fn request_activation(runtime_dir: &Path) -> bool {
    let sock = activation_socket_path(runtime_dir);
    let Ok(mut stream) = UnixStream::connect(&sock) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(ACTIVATE_TIMEOUT));
    stream.write_all(ACTIVATE).is_ok() && stream.flush().is_ok()
}
