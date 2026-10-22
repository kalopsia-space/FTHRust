//! FFmpeg-backed media layer for FTHRust: encoding, the encoded replay ring,
//! transactional clip saves, continuous recording, probing and thumbnails.
#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod audio_timing;
pub mod ffi;
pub mod replay_interval;

use std::sync::OnceLock;

/// Errors from the media layer.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    /// An FFmpeg call failed.
    #[error("ffmpeg: {0}")]
    Ffmpeg(#[from] ffmpeg_next::Error),
    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A `fthrust-core` operation failed.
    #[error(transparent)]
    Core(#[from] fthrust_core::CoreError),
    /// Invalid input or configuration.
    #[error("{0}")]
    Invalid(String),
    /// None of the candidate encoders could be opened.
    #[error("no usable encoder: {0}")]
    EncoderUnavailable(String),
}

/// Result alias for the media layer.
pub type Result<T> = std::result::Result<T, MediaError>;

static INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();

/// Initialises FFmpeg once per process. Safe to call repeatedly.
pub fn init() -> Result<()> {
    INIT.get_or_init(|| ffmpeg_next::init().map_err(|e| e.to_string()))
        .clone()
        .map_err(MediaError::Invalid)
}
