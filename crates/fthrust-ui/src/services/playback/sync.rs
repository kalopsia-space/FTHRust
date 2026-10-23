//! The reference's video-to-audio sync rule (`sync_to_video_position`):
//! audio is the clock; when it runs ahead of the video by more than the
//! drift limit it is held until the video catches up to within the resume
//! margin; when it falls behind by more than the limit it reseeks.

/// Drift beyond this holds or reseeks (ms).
pub const DRIFT_LIMIT_MS: i64 = 120;
/// A hold ends once the drift is within this (ms).
pub const RESUME_MARGIN_MS: i64 = 40;
/// No sync decisions for this long after a seek or rate change (s).
pub const RECOVERY_S: f64 = 0.75;

/// What to do this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncDecision {
    /// Suspend the audio output.
    Hold,
    /// Resume the audio output.
    Resume,
    /// Reseek audio to this video position (ms).
    Reseek(u64),
    /// Nothing.
    None,
}

/// `audio_ms` is the estimated output position, `video_ms` the frame on
/// screen; `holding` whether audio is currently suspended by this rule.
pub fn decide(audio_ms: i64, video_ms: i64, holding: bool, guard_active: bool) -> SyncDecision {
    if guard_active {
        return SyncDecision::None;
    }
    let drift = audio_ms - video_ms.max(0);
    if holding {
        if drift > RESUME_MARGIN_MS {
            return SyncDecision::None;
        }
        if drift < -DRIFT_LIMIT_MS {
            return SyncDecision::Reseek(video_ms.max(0) as u64);
        }
        return SyncDecision::Resume;
    }
    if drift > DRIFT_LIMIT_MS {
        return SyncDecision::Hold;
    }
    if drift < -DRIFT_LIMIT_MS {
        return SyncDecision::Reseek(video_ms.max(0) as u64);
    }
    SyncDecision::None
}
