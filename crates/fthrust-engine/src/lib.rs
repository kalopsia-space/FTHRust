//! The FTHRust capture daemon: policies, audio capture, the generation
//! loop, save and record workers, and the IPC service.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Crate version, printed by `fthrust-engine --version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
