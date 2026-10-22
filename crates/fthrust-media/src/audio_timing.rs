//! Audio block timing and window assembly for replay saves.
//!
//! Ports `reference/FTHRcapture_linux/src/audio_timing.{h,cpp}`: captured
//! blocks carry the monotonic time of their first frame; a save lays the
//! blocks covering its window back to back, inserting silence for real
//! gaps so later audio stays at its capture time.

/// Capture sample rate.
pub const SAMPLE_RATE: u32 = 48_000;
/// Capture channel count (interleaved stereo).
pub const CHANNELS: usize = 2;
/// Frames per 10 ms capture chunk.
pub const FRAMES_PER_CHUNK: usize = 480;
/// Gaps shorter than this are jitter and are joined seamlessly; longer ones
/// are filled with silence.
pub const GAP_FILL_THRESHOLD_NS: i64 = 50_000_000;

const NS_PER_S: i64 = 1_000_000_000;

/// Converts the delivery time of a block to its source-time start.
/// Unknown or negative latency counts as zero; a zero rate returns the
/// delivery time unchanged.
pub fn chunk_start_ns(
    delivery_end_ns: i64,
    latency_us: i64,
    frames: usize,
    sample_rate: u32,
) -> i64 {
    if sample_rate == 0 {
        return delivery_end_ns;
    }
    let latency_ns = latency_us.max(0) * 1_000;
    let block_ns = (frames as u64 * NS_PER_S as u64 / u64::from(sample_rate)) as i64;
    delivery_end_ns - latency_ns - block_ns
}

/// Audio ring size for a replay window: the window plus a ten-second margin.
pub fn ring_capacity_samples(keep_seconds: u32, sample_rate: u32, channels: usize) -> usize {
    (keep_seconds as usize + 10) * sample_rate as usize * channels
}

/// One captured block: interleaved samples and the time of its first frame.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioBlock {
    /// Monotonic nanoseconds of the first frame.
    pub start_ns: i64,
    /// Interleaved samples (`frames * channels` values).
    pub samples: Vec<f32>,
}

/// Interleaved PCM for the window `[end_ns - duration_ms, end_ns]`.
///
/// Blocks are laid back to back in capture order; a block starting more
/// than [`GAP_FILL_THRESHOLD_NS`] after the audio written so far is
/// preceded by silence. The result is exactly the window length (zero
/// padded) or empty when no block overlaps the window.
pub fn assemble_window(
    blocks: &[AudioBlock],
    channels: usize,
    sample_rate: u32,
    end_ns: i64,
    duration_ms: u32,
) -> Vec<f32> {
    if channels == 0 || sample_rate == 0 || duration_ms == 0 {
        return Vec::new();
    }
    let rate = i64::from(sample_rate);
    let total_frames = i64::from(duration_ms) * rate / 1000;
    let start_ns = end_ns - i64::from(duration_ms) * 1_000_000;
    let gap_frames = GAP_FILL_THRESHOLD_NS * rate / NS_PER_S;
    let ns_to_frames = |ns: i64| ns * rate / NS_PER_S;

    let mut out: Vec<f32> = Vec::with_capacity(total_frames as usize * channels);
    let mut cursor: i64 = 0;
    let mut any = false;
    for block in blocks {
        let frames = (block.samples.len() / channels) as i64;
        if frames == 0 {
            continue;
        }
        let block_end_ns = block.start_ns + frames * NS_PER_S / rate;
        if block_end_ns <= start_ns {
            continue;
        }
        if block.start_ns >= end_ns || cursor >= total_frames {
            break;
        }
        let mut skip = 0;
        let mut ideal = 0;
        if block.start_ns < start_ns {
            skip = frames.min(ns_to_frames(start_ns - block.start_ns));
        } else {
            ideal = total_frames.min(ns_to_frames(block.start_ns - start_ns));
        }
        if ideal - cursor > gap_frames {
            out.resize(ideal as usize * channels, 0.0);
            cursor = ideal;
        }
        let take = (frames - skip).min(total_frames - cursor);
        if take <= 0 {
            continue;
        }
        let first = skip as usize * channels;
        let last = first + take as usize * channels;
        out.extend_from_slice(&block.samples[first..last]);
        cursor += take;
        any = true;
    }
    if !any {
        return Vec::new();
    }
    out.resize(total_frames as usize * channels, 0.0);
    out
}
