//! Engine <-> UI IPC for FTHRust: message types, length-prefixed JSON
//! framing and the Unix-socket transport.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod config;
pub mod framing;
mod messages;
pub mod transport;

use std::path::PathBuf;

pub use messages::{
    AudioStatus, BackendKind, Command, EngineConfig, Event, RecordingState, SavedClip,
    SourceStatus, StatusSnapshot,
};

/// Wire protocol version carried in `Hello`.
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest frame either side accepts.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// IPC failures.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// Socket or stream I/O failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A message did not (de)serialise.
    #[error("ipc json: {0}")]
    Json(#[from] serde_json::Error),
    /// A frame header announced more than [`MAX_FRAME_BYTES`].
    #[error("ipc frame of {0} bytes exceeds the limit")]
    FrameTooLarge(usize),
    /// The stream ended inside a frame.
    #[error("ipc stream ended inside a frame")]
    Truncated,
    /// The peer speaks another protocol version.
    #[error("ipc protocol mismatch: expected {expected}, got {got}")]
    ProtocolMismatch {
        /// Ours.
        expected: u32,
        /// Theirs.
        got: u32,
    },
    /// The runtime directory could not be resolved.
    #[error(transparent)]
    Core(#[from] fthrust_core::CoreError),
}

/// `$XDG_RUNTIME_DIR/fthrust/engine.sock` (or the `~/.fthr/run` fallback).
pub fn socket_path() -> Result<PathBuf, IpcError> {
    Ok(fthrust_core::runtime_dir()?.join("engine.sock"))
}
