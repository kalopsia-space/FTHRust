//! Hotkey delivery on Linux (`hotkey_manager.py` + `linux_runtime.py`): the
//! private `hotkey.sock` the compositor binds call through, and the managed
//! Hyprland configuration that is rewritten and reloaded whenever the
//! bindings change.

use std::io::Read;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use fthrust_core::hotkeys::{HotkeyAction, HotkeyBindings, hyprland};

use super::{Event, Outbox};

/// A client must send its action within this long.
pub const CLIENT_TIMEOUT: Duration = Duration::from_millis(500);
/// The longest action name accepted.
pub const MAX_ACTION_LEN: usize = 64;

/// `prepare_socket_path`: remove only a stale socket of ours; refuse
/// symlinks, foreign files, other users' sockets and live sockets.
pub fn prepare_socket_path(path: &Path) -> Result<(), String> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("cannot inspect {}: {e}", path.display())),
    };
    let shown = path.display();
    if meta.file_type().is_symlink() {
        return Err(format!(
            "{shown} is a symlink. Refusing to touch it — remove it by hand after checking where it points."
        ));
    }
    if !meta.file_type().is_socket() {
        return Err(format!(
            "{shown} exists and is not a socket. Refusing to delete a file FTHR did not create."
        ));
    }
    if meta.uid() != nix::unistd::getuid().as_raw() {
        return Err(format!(
            "{shown} is owned by uid {}, not by you. Refusing to remove another user's socket.",
            meta.uid()
        ));
    }
    if UnixStream::connect(path).is_ok() {
        return Err(format!(
            "{shown} is already in use by a running FTHR Clips instance."
        ));
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot remove stale socket {shown}: {e}")),
    }
}

/// Parses one client message into an action the bindings know.
pub fn parse_message(bytes: &[u8], bindings: &HotkeyBindings) -> Option<HotkeyAction> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    if text.is_empty() || text.len() > MAX_ACTION_LEN {
        return None;
    }
    let action = HotkeyAction::parse(text)?;
    if let HotkeyAction::SaveCustomClip(n) = action
        && !bindings.custom_clip_lengths().contains(&n)
    {
        return None;
    }
    Some(action)
}

/// The listening socket.
pub struct HotkeyServer {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    bindings: Arc<Mutex<HotkeyBindings>>,
    thread: Option<JoinHandle<()>>,
}

impl HotkeyServer {
    /// Binds `<runtime_dir>/hotkey.sock` (mode 0600) and serves it on a thread.
    pub fn start(
        runtime_dir: &Path,
        bindings: HotkeyBindings,
        outbox: Outbox,
    ) -> Result<Self, String> {
        let path = runtime_dir.join("hotkey.sock");
        prepare_socket_path(&path)?;
        let listener = UnixListener::bind(&path)
            .map_err(|e| format!("cannot bind {}: {e}", path.display()))?;
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            let _ = std::fs::remove_file(&path);
            return Err(format!("cannot restrict {}: {e}", path.display()));
        }
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("cannot configure {}: {e}", path.display()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let bindings = Arc::new(Mutex::new(bindings));
        let thread = {
            let stop = stop.clone();
            let bindings = bindings.clone();
            std::thread::Builder::new()
                .name("fthrust-hotkey-sock".into())
                .spawn(move || serve(listener, stop, bindings, outbox))
                .map_err(|e| e.to_string())?
        };
        Ok(HotkeyServer {
            path,
            stop,
            bindings,
            thread: Some(thread),
        })
    }

    /// The socket path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// New bindings (custom clip lengths gate what is accepted).
    pub fn update_bindings(&self, bindings: HotkeyBindings) {
        if let Ok(mut b) = self.bindings.lock() {
            *b = bindings;
        }
    }

    /// Stops serving and unlinks the socket.
    pub fn close(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for HotkeyServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn serve(
    listener: UnixListener,
    stop: Arc<AtomicBool>,
    bindings: Arc<Mutex<HotkeyBindings>>,
    outbox: Outbox,
) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let bindings = bindings.clone();
                let outbox = outbox.clone();
                let _ = std::thread::Builder::new()
                    .name("fthrust-hotkey-client".into())
                    .spawn(move || handle(stream, &bindings, &outbox));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) => {
                tracing::warn!(error = %e, "hotkey socket accept failed");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn handle(mut stream: UnixStream, bindings: &Mutex<HotkeyBindings>, outbox: &Outbox) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(CLIENT_TIMEOUT));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_ACTION_LEN + 2 {
                    return;
                }
                if buf.contains(&b'\n') {
                    break;
                }
            }
            Err(_) => return,
        }
    }
    let action = match bindings.lock() {
        Ok(b) => parse_message(&buf, &b),
        Err(_) => None,
    };
    match action {
        Some(a) => {
            tracing::info!(action = %a.id(), "hotkey socket");
            outbox.send(Event::Hotkey(a));
        }
        None => tracing::info!(
            message = %String::from_utf8_lossy(&buf[..buf.len().min(MAX_ACTION_LEN)]).trim(),
            "hotkey socket: unknown command"
        ),
    }
}

/// What applying the compositor configuration did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    /// The managed file changed.
    pub changed: bool,
    /// `hyprctl reload` ran.
    pub reloaded: bool,
}

/// `_hyprland_apply`: on Hyprland, rewrite the managed binds file and
/// reload the running instance when something changed.
#[allow(clippy::too_many_arguments)]
pub fn apply_compositor_config(
    home: &Path,
    bindings: &HotkeyBindings,
    client_exe: &Path,
    sock: &Path,
    compositor: &str,
    has_instance: bool,
    hyprctl: Option<&Path>,
) -> Result<Applied, String> {
    if compositor != "hyprland" {
        return Ok(Applied {
            changed: false,
            reloaded: false,
        });
    }
    let applied = hyprland::apply(home, bindings, client_exe, sock).map_err(|e| e.to_string())?;
    let mut reloaded = false;
    if applied.changed && has_instance {
        let exe = hyprctl
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("hyprctl"));
        match std::process::Command::new(&exe)
            .arg("reload")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                reloaded = true;
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => return Err(format!("hyprctl reload failed: {e}")),
        }
    }
    Ok(Applied {
        changed: applied.changed,
        reloaded,
    })
}

/// Whether a Hyprland instance is reachable (`HYPRLAND_INSTANCE_SIGNATURE`).
pub fn hyprland_instance_present() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|s| !s.is_empty())
}
