//! One instance at a time: the lock plus the activation socket a second
//! launch writes `activate` to.
//!
//! The running instance answers `ok` when it shows its window, or `exiting`
//! once it has started to quit (an older build closes without an answer,
//! which counts as `ok`). A launch that finds the lock held by an instance
//! on its way out waits for the lock instead of reporting a window that is
//! never shown: the quitting instance keeps the lock until it has finished
//! writing (a save in flight can take a minute).

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fthrust_core::single_instance::{
    ACTIVATE, ACTIVATED, EXITING, InstanceLock, LockResult, activation_socket_path,
};

/// The timeout for the whole activation request.
pub const ACTIVATE_TIMEOUT: Duration = Duration::from_millis(350);

/// How long a launch waits for an instance that is shutting down. Its
/// longest shutdown (exports cancelled, a recording finished, a save in
/// flight) is about a minute and a half.
pub const PREVIOUS_INSTANCE_WAIT: Duration = Duration::from_secs(120);

/// How often a waiting launch tries the lock again.
const LOCK_POLL: Duration = Duration::from_millis(100);

/// What the running instance said to `activate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    /// It shows its window.
    Shown,
    /// It is shutting down.
    Exiting,
    /// Nothing listens.
    NoInstance,
}

/// The outcome of [`become_primary`].
pub struct Instance {
    lock: Option<InstanceLock>,
    activation: Arc<AtomicBool>,
    exiting: Arc<AtomicBool>,
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

    /// The app is quitting: a launch from now on is told so and waits for
    /// the lock, which goes when this is dropped.
    pub fn set_exiting(&self) {
        self.exiting.store(true, Ordering::Relaxed);
    }

    fn secondary() -> Self {
        Instance {
            lock: None,
            activation: Arc::new(AtomicBool::new(false)),
            exiting: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            primary: false,
            sock: None,
            waker: Arc::new(Mutex::new(None)),
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

/// Takes the lock and listens for activation requests. When a running
/// instance holds the lock it is asked for its window and a secondary is
/// returned; when the holder is shutting down, this waits up to
/// [`PREVIOUS_INSTANCE_WAIT`] for the lock, and fails after that.
pub fn become_primary(
    lock_file: &Path,
    runtime_dir: &Path,
    ctx: &egui::Context,
) -> Result<Instance, String> {
    become_primary_within(lock_file, runtime_dir, ctx, PREVIOUS_INSTANCE_WAIT)
}

/// [`become_primary`] with the wait for a quitting instance given.
pub fn become_primary_within(
    lock_file: &Path,
    runtime_dir: &Path,
    ctx: &egui::Context,
    wait: Duration,
) -> Result<Instance, String> {
    let deadline = Instant::now() + wait;
    let mut told = false;
    loop {
        match InstanceLock::acquire(lock_file) {
            LockResult::Acquired(lock) => return Ok(listen(Some(lock), runtime_dir, ctx)),
            LockResult::Unavailable => {
                tracing::warn!("instance lock unavailable; starting anyway");
                return Ok(listen(None, runtime_dir, ctx));
            }
            LockResult::Held => {}
        }
        // Asked again each turn: an instance that has only just taken the
        // lock may not listen yet.
        let answer = request_activation(runtime_dir);
        if answer == Activation::Shown {
            tracing::info!("another FTHR Clips instance owns the lock");
            return Ok(Instance::secondary());
        }
        if Instant::now() >= deadline {
            return Err(match answer {
                Activation::Exiting => {
                    "FTHR Clips is still shutting down; start it again once it has finished".into()
                }
                _ => "another FTHR Clips holds the instance lock but does not answer".into(),
            });
        }
        if !told && answer == Activation::Exiting {
            told = true;
            eprintln!("FTHR Clips is shutting down; starting once it has finished");
        }
        std::thread::sleep(LOCK_POLL);
    }
}

fn listen(lock: Option<InstanceLock>, runtime_dir: &Path, ctx: &egui::Context) -> Instance {
    let sock = activation_socket_path(runtime_dir);
    let activation = Arc::new(AtomicBool::new(false));
    let exiting = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let waker: Waker = Arc::new(Mutex::new(None));
    let _ = std::fs::remove_file(&sock);
    let thread = match UnixListener::bind(&sock) {
        Ok(listener) => {
            let _ = listener.set_nonblocking(true);
            let (flag, exiting_t, stop_t, ctx, wake) = (
                activation.clone(),
                exiting.clone(),
                stop.clone(),
                ctx.clone(),
                waker.clone(),
            );
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
                                let _ = conn.set_nonblocking(false);
                                let _ = conn.set_read_timeout(Some(ACTIVATE_TIMEOUT));
                                let _ = conn.set_write_timeout(Some(ACTIVATE_TIMEOUT));
                                let mut buf = [0u8; 32];
                                if let Ok(n) = conn.read(&mut buf)
                                    && buf[..n].trim_ascii() == ACTIVATE.trim_ascii()
                                {
                                    // Quitting (the window may already be
                                    // gone): the launch waits for the lock.
                                    if exiting_t.load(Ordering::Relaxed) || super::quitting() {
                                        let _ = conn.write_all(EXITING);
                                        continue;
                                    }
                                    flag.store(true, Ordering::Relaxed);
                                    ctx.request_repaint();
                                    let wake = wake.lock().ok().and_then(|w| w.clone());
                                    if let Some(wake) = wake {
                                        wake();
                                    }
                                    let _ = conn.write_all(ACTIVATED);
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
        exiting,
        stop,
        thread,
        primary: true,
        sock: Some(sock),
        waker,
    }
}

/// Asks the running instance to show its window; never blocks longer
/// than [`ACTIVATE_TIMEOUT`].
pub fn request_activation(runtime_dir: &Path) -> Activation {
    let sock = activation_socket_path(runtime_dir);
    let started = Instant::now();
    let Ok(mut stream) = UnixStream::connect(&sock) else {
        return Activation::NoInstance;
    };
    let _ = stream.set_write_timeout(Some(ACTIVATE_TIMEOUT));
    if stream.write_all(ACTIVATE).is_err() || stream.flush().is_err() {
        return Activation::NoInstance;
    }
    let left = ACTIVATE_TIMEOUT
        .saturating_sub(started.elapsed())
        .max(Duration::from_millis(1));
    let _ = stream.set_read_timeout(Some(left));
    let mut buf = [0u8; 16];
    match stream.read(&mut buf) {
        Ok(n) if buf[..n].trim_ascii() == EXITING.trim_ascii() => Activation::Exiting,
        // `ok`, no answer from an older build, or a slow one: the request
        // was delivered.
        _ => Activation::Shown,
    }
}
