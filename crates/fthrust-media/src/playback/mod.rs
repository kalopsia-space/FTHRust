//! Playback: the editor's audio mixer and video decoder (plan 6a). The
//! reference's `ffmpeg_playback_bridge.cpp` and `ffmpeg_playback.py`.

pub mod mixer;

/// Output sample rate.
pub const CANONICAL_SAMPLE_RATE: u32 = 48_000;
/// Output channels.
pub const CANONICAL_CHANNELS: usize = 2;
/// The audio worker's block size in frames.
pub const FRAMES_PER_BLOCK: usize = 1024;
/// The decoded queue's capacity in frames (`48000 * 3 / 20`).
pub const MAX_QUEUED_FRAMES: usize = (CANONICAL_SAMPLE_RATE as usize) * 3 / 20;

/// One audio track to mix: the manifest's id and its container stream index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSpec {
    /// The manifest source id (`system`, `microphone`, …).
    pub source_id: String,
    /// The stream index inside the container.
    pub container_index: usize,
}
