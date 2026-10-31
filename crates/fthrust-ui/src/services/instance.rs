//! One instance at a time: the lock plus the activation socket a second
//! launch writes `activate` to.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
    sock: Option<PathBuf>,
    waker: Waker,
}

/// What an activation request calls to wake the app's event loop.
type Waker = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

impl Instance {
    /// This process owns the window.
    pub fn is_primary(&self) -> bool {
        self.primary
    }

    /// Whether a second launch asked for the window since the last call.
    pub fn activation_requested(&self) -> bool {
        self.activation.swap(false, Ordering::Relaxed)
    }

    /// Wakes the app's event loop on an activation request. The listener
    /// starts before the window exists, so the context it was given is a
    /// stand-in that nothing paints; the services hand in the real wake-up.
    pub fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut w) = self.waker.lock() {
            *w = Some(wake);
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if self.thread.is_some()
            && let Some(sock) = &self.sock
        {
            let _ = UnixStream::connect(sock);
        }
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
                sock: None,
                waker: Arc::new(Mutex::new(None)),
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
    let waker: Waker = Arc::new(Mutex::new(None));
    let _ = std::fs::remove_file(&sock);
    let thread = match UnixListener::bind(&sock) {
        Ok(listener) => {
            let _ = listener.set_nonblocking(true);
            let (flag, stop_t, ctx, wake) =
                (activation.clone(), stop.clone(), ctx.clone(), waker.clone());
            let path = sock.clone();
            std::thread::Builder::new()
                .name("fthrust-activation".into())
                .spawn(move || {
                    while !stop_t.load(Ordering::Relaxed) {
                        if !super::wait_for_connection(&listener, super::ACCEPT_POLL)
                            || stop_t.load(Ordering::Relaxed)
                        {
                            continue;
                        }
                        match listener.accept() {
                            Ok((mut conn, _)) => {
                                let _ = conn.set_read_timeout(Some(ACTIVATE_TIMEOUT));
                                let mut buf = [0u8; 32];
                                if let Ok(n) = conn.read(&mut buf)
                                    && buf[..n].trim_ascii() == ACTIVATE.trim_ascii()
                                {
                                    flag.store(true, Ordering::Relaxed);
                                    ctx.request_repaint();
                                    let wake = wake.lock().ok().and_then(|w| w.clone());
                                    if let Some(wake) = wake {
                                        wake();
                                    }
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(_) => break,
                        }
                    }
                    let _ = std::fs::remove_file(&path);
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
        sock: Some(sock),
        waker,
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
