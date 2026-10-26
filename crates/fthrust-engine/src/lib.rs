//! The FTHRust capture daemon: policies, audio capture, the generation
//! loop, save and record workers, and the IPC service.
// deny, not forbid: alloc.rs wraps libc and opts in locally.
#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod alloc;
pub mod audio;
pub mod backend;
pub mod cli;
pub mod daemon;
pub mod devices;
pub mod engine;
pub mod policy;
pub mod portal_backend;
pub mod save;
pub mod watchdog;
pub mod zero_copy;

/// Crate version, printed by `fthrust-engine --version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
