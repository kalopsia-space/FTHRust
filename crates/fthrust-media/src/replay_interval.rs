//! Selection of the packet window for a replay save.
//!
//! Ports `reference/FTHRcapture_common/replay_interval.h`: given the capture
//! timestamps and keyframe flags of the packets in the ring, choose the
//! decode start (a keyframe at or before the requested window), the first
//! presented packet and the last packet at or before the target end, and
//! say whether the ring held the full requested duration.

/// One packet's capture timestamp (nanoseconds) and keyframe flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    /// Capture timestamp, monotonic nanoseconds.
    pub timestamp: i64,
    /// Whether the packet starts a decodable GOP.
    pub keyframe: bool,
}

/// Why no window could be selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectError {
    /// No samples, or a non-positive requested duration.
    EmptyInput,
    /// Every sample is newer than the target end.
    NoPacketAtTarget,
    /// No keyframe exists at or before the chosen end.
    MissingStartKeyframe,
}

/// A valid window: indexes into the input slice plus the timing facts the
/// save needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Whether the ring held the whole requested duration.
    pub full_history: bool,
    /// Index of the keyframe decoding starts from.
    pub decode_start: usize,
    /// Index of the first packet that is presented.
    pub presentation_start: usize,
    /// Index of the last packet (inclusive).
    pub end: usize,
    /// `target_end - requested_duration`.
    pub requested_start: i64,
    /// The requested end timestamp.
    pub target_end: i64,
    /// Timestamp of the last selected packet.
    pub media_end: i64,
}

/// Selects the packets covering `requested_duration` nanoseconds ending at
/// `target_end`. Partial history decodes from the first keyframe.
pub fn select(
    samples: &[Sample],
    target_end: i64,
    requested_duration: i64,
) -> Result<Selection, SelectError> {
    if samples.is_empty() || requested_duration <= 0 {
        return Err(SelectError::EmptyInput);
    }
    let mut end = samples.len();
    while end > 0 && samples[end - 1].timestamp > target_end {
        end -= 1;
    }
    if end == 0 {
        return Err(SelectError::NoPacketAtTarget);
    }
    let end = end - 1;

    let requested_start = target_end - requested_duration;
    let mut keyframe_before = None;
    let mut first_keyframe = None;
    for (i, sample) in samples.iter().enumerate().take(end + 1) {
        if !sample.keyframe {
            continue;
        }
        if first_keyframe.is_none() {
            first_keyframe = Some(i);
        }
        if sample.timestamp <= requested_start {
            keyframe_before = Some(i);
        }
    }
    let Some(first_keyframe) = first_keyframe else {
        return Err(SelectError::MissingStartKeyframe);
    };

    let full_history = samples[0].timestamp <= requested_start && keyframe_before.is_some();
    let media_end = samples[end].timestamp;
    if !full_history {
        return Ok(Selection {
            full_history,
            decode_start: first_keyframe,
            presentation_start: first_keyframe,
            end,
            requested_start,
            target_end,
            media_end,
        });
    }
    let decode_start = keyframe_before.expect("full history implies a keyframe before");
    let mut presentation_start = decode_start;
    while presentation_start < end && samples[presentation_start].timestamp < requested_start {
        presentation_start += 1;
    }
    Ok(Selection {
        full_history,
        decode_start,
        presentation_start,
        end,
        requested_start,
        target_end,
        media_end,
    })
}

/// The pts at which presentation starts: `newest_pts + 1 - duration_ticks`
/// for a full window, else the decode start's pts.
pub fn presentation_start_pts(
    newest_pts: i64,
    requested_duration_ticks: i64,
    full_history: bool,
    decode_start_pts: i64,
) -> i64 {
    if !full_history || requested_duration_ticks <= 0 {
        return decode_start_pts;
    }
    newest_pts + 1 - requested_duration_ticks
}

/// [`presentation_start_pts`] with the duration given in seconds at
/// `ticks_per_second`.
pub fn presentation_start_pts_seconds(
    newest_pts: i64,
    ticks_per_second: i64,
    requested_seconds: i64,
    full_history: bool,
    decode_start_pts: i64,
) -> i64 {
    presentation_start_pts(
        newest_pts,
        ticks_per_second * requested_seconds,
        full_history,
        decode_start_pts,
    )
}
