//! The optional clip uploader: a one-shot program that reads one JSON
//! request on stdin and writes one JSON line on stdout, so the main app
//! never links provider network code (spec 6.5).
//!
//! | module | owns |
//! |---|---|
//! | `protocol` | request gating and dispatch (`uploader_service.py`) |
//! | `http` | Catbox and your own server (`upload_runtime.py`) |
//! | `sftp` | SFTP uploads and host keys (`sftp_provider.py`) |
//! | `vault` | saved SFTP passwords |
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fthrust_core::FthrPaths;
use serde_json::{Map, Value};

pub mod http;
mod protocol;
pub mod sftp;
pub mod vault;

pub use protocol::handle;

/// Catbox's upload API.
pub const CATBOX_URL: &str = "https://catbox.moe/user/api.php";
/// Waits between upload attempts (`_RETRY_DELAYS`).
pub const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(45),
];
/// No retry starts once an upload has taken this long: a transfer that long
/// is not repeated on its own, the user is offered RETRY NOW instead.
pub const DEADLINE: Duration = Duration::from_secs(540);
/// How long an HTTP upload may go without moving a byte, and how long a
/// server may take to answer (`timeout=120` in the reference).
pub const IO_TIMEOUT: Duration = Duration::from_secs(120);
/// How often an upload that is moving says so on stderr (see [`Pulse`]).
pub const PULSE_EVERY: Duration = Duration::from_secs(10);

/// What an upload tells the app while it runs: a line on stderr now and then
/// (`[Uploader] Sent 12.0 of 456.7 MB`) for as long as bytes move. The app
/// stops an uploader that has gone quiet for ten minutes, and no other, so a
/// big clip on a slow link is never cut off for taking long.
#[derive(Clone)]
pub struct Pulse {
    every: Duration,
    sink: Arc<dyn Fn(&str) + Send + Sync>,
}

impl Pulse {
    /// Lines on stderr, one every [`PULSE_EVERY`].
    pub fn stderr() -> Pulse {
        Pulse::new(PULSE_EVERY, |line| {
            let _ = writeln!(std::io::stderr(), "{line}");
        })
    }

    /// Lines to `sink`, one every `every`.
    pub fn new(every: Duration, sink: impl Fn(&str) + Send + Sync + 'static) -> Pulse {
        Pulse {
            every,
            sink: Arc::new(sink),
        }
    }

    /// The count of one transfer of `total` bytes.
    pub fn count(&self, total: u64) -> Count {
        Count {
            pulse: self.clone(),
            total,
            sent: 0,
            next: Instant::now() + self.every,
        }
    }
}

/// The bytes of one transfer that have gone out.
pub struct Count {
    pulse: Pulse,
    total: u64,
    sent: u64,
    next: Instant,
}

impl Count {
    /// `bytes` more have gone out; says so when a line is due.
    pub fn add(&mut self, bytes: u64) {
        self.sent += bytes;
        let now = Instant::now();
        if now >= self.next {
            self.next = now + self.pulse.every;
            let mb = |n: u64| format!("{:.1}", n as f64 / (1024.0 * 1024.0));
            (self.pulse.sink)(&format!(
                "[Uploader] Sent {} of {} MB",
                mb(self.sent),
                mb(self.total)
            ));
        }
    }
}

/// Where the uploader talks to and keeps its state.
pub struct Env {
    /// `~/.fthr` paths.
    pub paths: FthrPaths,
    /// Catbox endpoint.
    pub catbox_url: String,
    /// Waits between attempts.
    pub retry_delays: Vec<Duration>,
    /// Retry budget.
    pub deadline: Duration,
    /// The stall limit of an HTTP upload (see [`IO_TIMEOUT`]).
    pub io_timeout: Duration,
    /// Where an upload says it is still moving.
    pub progress: Pulse,
    /// Saved passwords.
    pub vault: Box<dyn vault::Vault>,
}

impl Env {
    /// The real environment: `$HOME` and the desktop keyring. Debug builds
    /// (the tests, the UI bench) also read the hidden overrides
    /// `FTHRUST_UPLOADER_CATBOX_URL` and `FTHRUST_UPLOADER_NO_RETRY_DELAY`;
    /// a release build has no such thing, so no environment variable can
    /// send its uploads somewhere else.
    pub fn from_process() -> Result<Env, String> {
        let paths = FthrPaths::from_env().map_err(|e| e.to_string())?;
        #[allow(unused_mut)]
        let mut env = Env {
            paths,
            catbox_url: CATBOX_URL.into(),
            retry_delays: RETRY_DELAYS.to_vec(),
            deadline: DEADLINE,
            io_timeout: IO_TIMEOUT,
            progress: Pulse::stderr(),
            vault: Box::new(vault::SecretService),
        };
        #[cfg(debug_assertions)]
        {
            if let Ok(url) = std::env::var("FTHRUST_UPLOADER_CATBOX_URL") {
                env.catbox_url = url;
            }
            if std::env::var_os("FTHRUST_UPLOADER_NO_RETRY_DELAY").is_some() {
                env.retry_delays = vec![Duration::ZERO; RETRY_DELAYS.len()];
            }
        }
        Ok(env)
    }

    /// A test environment: no retry waits, no progress lines, an in-memory
    /// vault.
    pub fn for_tests(paths: FthrPaths) -> Env {
        Env {
            paths,
            catbox_url: CATBOX_URL.into(),
            retry_delays: vec![Duration::ZERO; RETRY_DELAYS.len()],
            deadline: DEADLINE,
            io_timeout: IO_TIMEOUT,
            progress: Pulse::new(PULSE_EVERY, |_| {}),
            vault: Box::new(vault::Memory::default()),
        }
    }
}

/// One response line.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    /// Success.
    pub ok: bool,
    /// The message shown to the user.
    pub message: String,
    /// The history entry of an upload (`{}` when it failed).
    pub upload_info: Option<Map<String, Value>>,
    /// The failure is in the upload settings (URL, token): retrying the
    /// same request cannot help. Written only when true.
    pub settings_problem: bool,
    /// What the user should know about an upload that worked: no link came
    /// back, the history could not be saved. Written only when there is one.
    pub warning: Option<String>,
    /// The clip may be deleted now: the upload is confirmed (a link the app
    /// can offer, saved in the history), the settings ask for auto-delete and
    /// the clip is inside the clips folder. The uploader never deletes a clip
    /// itself; the app does it by the library's own rules, which know about
    /// the exports, shares and editors of a clip. Written only when true.
    pub delete: bool,
}

impl Response {
    /// A failure.
    pub fn fail(message: impl Into<String>) -> Response {
        Response {
            ok: false,
            message: message.into(),
            upload_info: None,
            settings_problem: false,
            warning: None,
            delete: false,
        }
    }

    /// A success without upload info.
    pub fn done(message: impl Into<String>) -> Response {
        Response {
            ok: true,
            message: message.into(),
            upload_info: None,
            settings_problem: false,
            warning: None,
            delete: false,
        }
    }

    /// The compact JSON line (`separators=(',', ':')`), without the newline.
    pub fn to_line(&self) -> String {
        let mut m = Map::new();
        m.insert("ok".into(), Value::Bool(self.ok));
        m.insert("message".into(), Value::String(self.message.clone()));
        if let Some(info) = &self.upload_info {
            m.insert("upload_info".into(), Value::Object(info.clone()));
        }
        if self.settings_problem {
            m.insert("settings_problem".into(), Value::Bool(true));
        }
        if let Some(warning) = &self.warning {
            m.insert("warning".into(), Value::String(warning.clone()));
        }
        if self.delete {
            m.insert("delete".into(), Value::Bool(true));
        }
        Value::Object(m).to_string()
    }
}
