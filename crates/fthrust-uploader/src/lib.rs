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

use std::time::Duration;

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
/// No retry starts once this much of the caller's 600 s budget is gone.
pub const DEADLINE: Duration = Duration::from_secs(540);

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
    /// Saved passwords.
    pub vault: Box<dyn vault::Vault>,
}

impl Env {
    /// The real environment: `$HOME`, the desktop keyring, and the hidden
    /// test overrides `FTHRUST_UPLOADER_CATBOX_URL` and
    /// `FTHRUST_UPLOADER_NO_RETRY_DELAY`.
    pub fn from_process() -> Result<Env, String> {
        let paths = FthrPaths::from_env().map_err(|e| e.to_string())?;
        let catbox_url =
            std::env::var("FTHRUST_UPLOADER_CATBOX_URL").unwrap_or_else(|_| CATBOX_URL.into());
        let retry_delays = if std::env::var_os("FTHRUST_UPLOADER_NO_RETRY_DELAY").is_some() {
            vec![Duration::ZERO; RETRY_DELAYS.len()]
        } else {
            RETRY_DELAYS.to_vec()
        };
        Ok(Env {
            paths,
            catbox_url,
            retry_delays,
            deadline: DEADLINE,
            vault: Box::new(vault::SecretService),
        })
    }

    /// A test environment: no retry waits, an in-memory vault.
    pub fn for_tests(paths: FthrPaths) -> Env {
        Env {
            paths,
            catbox_url: CATBOX_URL.into(),
            retry_delays: vec![Duration::ZERO; RETRY_DELAYS.len()],
            deadline: DEADLINE,
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
}

impl Response {
    /// A failure.
    pub fn fail(message: impl Into<String>) -> Response {
        Response {
            ok: false,
            message: message.into(),
            upload_info: None,
        }
    }

    /// A success without upload info.
    pub fn done(message: impl Into<String>) -> Response {
        Response {
            ok: true,
            message: message.into(),
            upload_info: None,
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
        Value::Object(m).to_string()
    }
}
