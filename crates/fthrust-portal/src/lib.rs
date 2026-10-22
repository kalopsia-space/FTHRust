//! ScreenCast portal session (ashpd) and its PipeWire video stream.
#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod screencast;
pub mod stream;
pub mod token;

use std::sync::OnceLock;

static PW_INIT: OnceLock<()> = OnceLock::new();

/// Initialises libpipewire once per process.
pub fn init_pipewire() {
    PW_INIT.get_or_init(pipewire::init);
}
