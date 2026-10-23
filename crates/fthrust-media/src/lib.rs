//! FFmpeg-backed media layer for FTHRust: encoding, the encoded replay ring,
//! transactional clip saves, continuous recording, probing and thumbnails.
//!
//! | Module | Ports | Purpose |
//! |---|---|---|
//! | [`replay_interval`] | `replay_interval.cpp` | Which packets a replay window needs and where presentation starts |
//! | [`audio_timing`] | `audio_timing.cpp` | 48 kHz chunk clock, gap fill, ring capacity, window assembly |
//! | [`ring`] | `ring_buffer.cpp` | The encoded GOP ring: time and byte-budget trims, snapshots, publish waits |
//! | [`transactional_save`] | `transactional_save.cpp` | `.partial` write, fsync, no-replace rename with link fallback |
//! | [`encoder_policy`] | `encoder_policy.cpp` | Candidate encoder order per codec and backend, preset option tables |
//! | [`encoder`] | `encoder.cpp` | BGRA in, packets out: hardware candidates, BT.709, wall-clock pts, forced IDR |
//! | [`clip_writer`] | `save_clip.cpp` | Plain MP4 with negative pre-roll pts and native AAC, saved transactionally |
//! | [`recorder`] | `continuous_recording_writer.cpp` | Recoverable fragmented MP4 that commits a fragment at every keyframe |
//! | [`probe`] | `core/media_metadata.py` | ffprobe-rule parsing, configured-tag overrides, CFR evidence |
//! | [`thumbnail`] | `ui/clip_grid.py` | Poster frames fitted into 640×360 |
//! | [`capabilities`] | `core/encoder_capabilities.py` | Working-backend discovery by a one-frame encode, and its cache |
//! | [`combined_audio`] | `core/combined_audio_policy.py` | Which native streams a combined track mixes, and the `amix` filter |
//! | [`vaapi`] | `encoder.cpp` | VA-API render-node discovery |
//! | `ffi` (private) | — | The only `unsafe` in the crate: thin wrappers over FFmpeg APIs ffmpeg-next lacks |
//!
//! Every module above the `ffi` row is safe Rust. Call [`init`] once (any
//! entry point does it for you) before touching FFmpeg.
#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod audio_decode;
pub mod audio_timing;
pub mod camera;
pub mod capabilities;
pub mod clip_writer;
pub mod combined_audio;
pub mod encoder;
pub mod encoder_policy;
pub mod export;
pub(crate) mod ffi;
pub mod playback;
pub mod probe;
pub mod recorder;
pub mod replay_interval;
pub mod ring;
pub mod thumbnail;
pub mod transactional_save;

/// VA-API device discovery (the safe surface of the private `ffi::hw`).
pub mod vaapi {
    pub use crate::ffi::hw::{DEFAULT_DEVICE, candidate_devices, vaapi_available};
}

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
