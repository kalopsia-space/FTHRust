//! ScreenCast portal session (ashpd) and its PipeWire video stream.
#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod capture;
pub mod screencast;
pub mod stream;
pub mod token;

use std::sync::OnceLock;

static PW_INIT: OnceLock<()> = OnceLock::new();

/// Initialises libpipewire once per process.
pub fn init_pipewire() {
    PW_INIT.get_or_init(pipewire::init);
}

static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();

/// The runtime every portal session runs on, alive for the process. zbus
/// runs a connection's reader on the runtime that opened it, so a runtime
/// dropped after one capture would leave the next one talking to a dead
/// connection.
pub(crate) fn runtime() -> Result<&'static tokio::runtime::Runtime, screencast::PortalFailure> {
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("fthrust-portal")
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| screencast::PortalFailure::Failed(format!("tokio: {e}")))
}
