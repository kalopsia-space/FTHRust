//! Interpretation of engine frame counters and health bits for the UI.
//!
//! Ports `reference/FTHR_UI/core/capture_health.py`: a pure state machine fed
//! from the status poll, plus the save-admission rule and the content
//! suspicion tracker shared with the engine.

use crate::error::{CoreError, Result};

/// Typed engine health bits.
pub mod flags {
    /// Capture is active.
    pub const ACTIVE: u32 = 1 << 0;
    /// The backend is recovering.
    pub const RECOVERING: u32 = 1 << 1;
    /// The backend failed.
    pub const BACKEND_FAILED: u32 = 1 << 2;
    /// Fresh frames look black-like or uniform.
    pub const CONTENT_SUSPECT: u32 = 1 << 3;
    /// Capture is intentionally paused (focus pause).
    pub const PAUSED: u32 = 1 << 4;
    /// A reconfiguration is required (Windows engine only; kept for parity).
    pub const RECONFIGURE_REQUIRED: u32 = 1 << 5;
}

/// User-facing capture health states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthState {
    /// Waiting for the first frame of a generation.
    Initializing,
    /// Frames are progressing.
    Healthy,
    /// Progress is delayed or capture is paused.
    Degraded,
    /// Fresh frames are persistently black-like or uniform.
    ContentSuspect,
    /// No new frame for a long time.
    Stalled,
    /// The backend failed.
    Failed,
    /// The backend is recovering.
    Recovering,
    /// The engine is not connected.
    Stopped,
}

impl HealthState {
    /// Every state, in the reference's order.
    pub const ALL: [HealthState; 8] = [
        Self::Initializing,
        Self::Healthy,
        Self::Degraded,
        Self::ContentSuspect,
        Self::Stalled,
        Self::Failed,
        Self::Recovering,
        Self::Stopped,
    ];

    /// The reference's lowercase state name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initializing => "initializing",
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::ContentSuspect => "content_suspect",
            Self::Stalled => "stalled",
            Self::Failed => "failed",
            Self::Recovering => "recovering",
            Self::Stopped => "stopped",
        }
    }
}

/// One deterministic result from [`CaptureHealthMonitor::observe`].
#[derive(Clone, Debug, PartialEq)]
pub struct HealthSnapshot {
    /// State after this poll.
    pub state: HealthState,
    /// State before this poll.
    pub previous_state: HealthState,
    /// Human-readable reason for the state.
    pub reason: String,
    /// Whether the state changed in this poll.
    pub changed: bool,
    /// Whether the UI should request one automatic recovery.
    pub request_recovery: bool,
    /// Seconds since the frame counter last advanced.
    pub seconds_since_progress: f64,
    /// Seconds of counter-advancing time accumulated since the last reset.
    pub fresh_buffer_seconds: f64,
}

impl HealthSnapshot {
    /// Whether replay data is recent enough to permit a save; the native
    /// ring validates the actual contents.
    pub fn save_allowed(&self) -> bool {
        !matches!(
            self.state,
            HealthState::Stopped | HealthState::Failed | HealthState::Recovering
        )
    }
}

/// Interprets frame progress from the status poll.
#[derive(Clone, Debug)]
pub struct CaptureHealthMonitor {
    warning_after_s: f64,
    stalled_after_s: f64,
    state: HealthState,
    last_count: Option<u64>,
    last_progress_at: Option<f64>,
    fresh_since: Option<f64>,
    fresh_duration_s: f64,
    last_observed_at: Option<f64>,
    generation: Option<u64>,
    auto_recoveries: u32,
    healthy_since: Option<f64>,
}

impl Default for CaptureHealthMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureHealthMonitor {
    /// Seconds without progress before the state degrades.
    pub const WARNING_AFTER_S: f64 = 3.0;
    /// Seconds without progress before a stall is declared.
    pub const STALLED_AFTER_S: f64 = 8.0;
    /// Healthy seconds that restore the automatic-recovery budget.
    pub const RECOVERY_BUDGET_RESET_S: f64 = 30.0;

    /// A monitor with the reference's thresholds.
    pub fn new() -> Self {
        Self::with_thresholds(Self::WARNING_AFTER_S, Self::STALLED_AFTER_S)
            .expect("default thresholds are valid")
    }

    /// A monitor with custom thresholds (`0 < warning < stalled`).
    pub fn with_thresholds(warning_after_s: f64, stalled_after_s: f64) -> Result<Self> {
        if !(0.0 < warning_after_s && warning_after_s < stalled_after_s) {
            return Err(CoreError::Invalid(
                "health thresholds must satisfy 0 < warning < stalled".into(),
            ));
        }
        Ok(Self {
            warning_after_s,
            stalled_after_s,
            state: HealthState::Stopped,
            last_count: None,
            last_progress_at: None,
            fresh_since: None,
            fresh_duration_s: 0.0,
            last_observed_at: None,
            generation: None,
            auto_recoveries: 0,
            healthy_since: None,
        })
    }

    /// The current state.
    pub fn state(&self) -> HealthState {
        self.state
    }

    /// Resets progress baselines for a new engine or backend generation.
    pub fn reset(&mut self, preserve_recovery_budget: bool) {
        self.state = HealthState::Initializing;
        self.last_count = None;
        self.last_progress_at = None;
        self.fresh_since = None;
        self.fresh_duration_s = 0.0;
        self.last_observed_at = None;
        self.generation = None;
        self.healthy_since = None;
        if !preserve_recovery_budget {
            self.auto_recoveries = 0;
        }
    }

    /// Advances the state machine from one status poll.
    pub fn observe(
        &mut self,
        connected: bool,
        frame_count: u64,
        engine_flags: u32,
        generation: u64,
        now: f64,
    ) -> HealthSnapshot {
        let poll_delta = self
            .last_observed_at
            .map_or(0.0, |last| (now - last).max(0.0));
        self.last_observed_at = Some(now);
        let old_state = self.state;
        let count = frame_count;

        if !connected {
            self.state = HealthState::Stopped;
            self.last_count = None;
            self.last_progress_at = None;
            self.fresh_since = None;
            self.fresh_duration_s = 0.0;
            self.healthy_since = None;
            return self.snapshot("engine disconnected".into(), old_state, false, now);
        }

        match self.generation {
            None => {
                self.generation = Some(generation);
                self.last_count = None;
                self.last_progress_at = Some(now);
                self.fresh_since = None;
                self.fresh_duration_s = 0.0;
                self.healthy_since = None;
                self.state = HealthState::Initializing;
            }
            Some(g) if g != generation => {
                self.generation = Some(generation);
                self.last_count = Some(count);
                self.last_progress_at = Some(now);
                self.fresh_since = None;
                self.fresh_duration_s = 0.0;
                self.healthy_since = None;
                self.state = HealthState::Initializing;
            }
            _ => {}
        }

        if engine_flags & flags::BACKEND_FAILED != 0 {
            self.state = HealthState::Failed;
            self.fresh_since = None;
            self.fresh_duration_s = 0.0;
            self.healthy_since = None;
            return self.snapshot("capture backend failed".into(), old_state, false, now);
        }
        if engine_flags & flags::RECOVERING != 0 {
            self.state = HealthState::Recovering;
            self.last_progress_at = Some(now);
            self.fresh_since = None;
            self.fresh_duration_s = 0.0;
            self.healthy_since = None;
            return self.snapshot(
                "capture backend is recovering".into(),
                old_state,
                false,
                now,
            );
        }
        if engine_flags & flags::PAUSED != 0 {
            self.state = HealthState::Degraded;
            self.last_progress_at = Some(now);
            self.healthy_since = None;
            return self.snapshot(
                "capture is intentionally paused".into(),
                old_state,
                false,
                now,
            );
        }

        let progressed = match self.last_count {
            None => count > 0,
            Some(last) => {
                if count < last {
                    self.fresh_since = None;
                    self.fresh_duration_s = 0.0;
                }
                count != last
            }
        };
        self.last_count = Some(count);

        if progressed {
            self.last_progress_at = Some(now);
            if self.fresh_since.is_none() {
                self.fresh_since = Some(now);
                self.fresh_duration_s = 0.0;
            } else {
                self.fresh_duration_s += poll_delta;
            }
            let reason = if engine_flags & flags::CONTENT_SUSPECT != 0 {
                self.state = HealthState::ContentSuspect;
                self.healthy_since = None;
                "fresh frames are persistently black-like or uniform"
            } else {
                self.state = HealthState::Healthy;
                match self.healthy_since {
                    None => self.healthy_since = Some(now),
                    Some(since) if now - since >= Self::RECOVERY_BUDGET_RESET_S => {
                        self.auto_recoveries = 0;
                    }
                    _ => {}
                }
                "frames progressing"
            };
            return self.snapshot(reason.into(), old_state, false, now);
        }

        if self.last_progress_at.is_none() {
            self.last_progress_at = Some(now);
        }
        let elapsed = now - self.last_progress_at.expect("set above");

        if elapsed >= self.stalled_after_s {
            self.state = HealthState::Stalled;
            self.healthy_since = None;
            let mut request_recovery = false;
            if old_state != HealthState::Stalled && self.auto_recoveries < 1 {
                self.auto_recoveries += 1;
                request_recovery = true;
            }
            return self.snapshot(
                format!("no new frame for {elapsed:.1} seconds"),
                old_state,
                request_recovery,
                now,
            );
        }
        if elapsed >= self.warning_after_s {
            self.state = HealthState::Degraded;
            self.healthy_since = None;
            return self.snapshot(
                format!("frame progress delayed for {elapsed:.1} seconds"),
                old_state,
                false,
                now,
            );
        }

        let reason = if self.fresh_since.is_some() && engine_flags & flags::CONTENT_SUSPECT != 0 {
            self.state = HealthState::ContentSuspect;
            "fresh content remains persistently black-like or uniform"
        } else if self.fresh_since.is_none() {
            self.state = HealthState::Initializing;
            "waiting for first frame"
        } else {
            self.state = HealthState::Healthy;
            "between frames"
        };
        self.snapshot(reason.into(), old_state, false, now)
    }

    fn snapshot(
        &self,
        reason: String,
        old_state: HealthState,
        request_recovery: bool,
        now: f64,
    ) -> HealthSnapshot {
        let since_progress = self.last_progress_at.map_or(0.0, |at| (now - at).max(0.0));
        HealthSnapshot {
            state: self.state,
            previous_state: old_state,
            reason,
            changed: self.state != old_state,
            request_recovery,
            seconds_since_progress: since_progress,
            fresh_buffer_seconds: self.fresh_duration_s,
        }
    }
}

/// Capture-health decision made before publishing a save command.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveAdmission {
    /// Whether the save may proceed.
    pub allowed: bool,
    /// Seconds to request (0 when refused).
    pub duration_seconds: u32,
    /// User-facing refusal reason, empty when allowed.
    pub reason: &'static str,
}

/// Rejects saves while the backend is unavailable; the native ring validates
/// retained footage itself.
pub fn evaluate_save_admission(
    snapshot: Option<&HealthSnapshot>,
    requested_seconds: u32,
) -> SaveAdmission {
    let requested = requested_seconds.max(1);
    let Some(snapshot) = snapshot else {
        return SaveAdmission {
            allowed: false,
            duration_seconds: 0,
            reason: "Capture health is not available yet.",
        };
    };
    if !snapshot.save_allowed() {
        let reason = if snapshot.state == HealthState::Recovering {
            "Capture is recovering. Try again in a moment."
        } else {
            "Capture is not receiving new frames. Restart capture before saving."
        };
        return SaveAdmission {
            allowed: false,
            duration_seconds: 0,
            reason,
        };
    }
    SaveAdmission {
        allowed: true,
        duration_seconds: requested,
        reason: "",
    }
}

/// Privacy-safe luma metrics of a sparse frame sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentSample {
    /// Mean luma of the sampled grid.
    pub luma_mean: f64,
    /// Luma variance of the sampled grid.
    pub luma_variance: f64,
    /// FNV-1a hash of the sampled lumas, for repeat detection.
    pub sample_hash: u32,
    /// Mean ≤ 8 and variance ≤ 6.
    pub black_like: bool,
    /// Variance ≤ 2.
    pub uniform: bool,
}

impl ContentSample {
    /// Builds a sample from precomputed luma statistics.
    pub fn from_luma(luma_mean: f64, luma_variance: f64, sample_hash: u32) -> Self {
        Self {
            luma_mean,
            luma_variance,
            sample_hash,
            black_like: luma_mean <= 8.0 && luma_variance <= 6.0,
            uniform: luma_variance <= 2.0,
        }
    }
}

/// Samples a sparse `columns`×`rows` grid of a BGRA frame without retaining
/// pixels, matching the engine's parity check.
pub fn sample_bgra_grid(
    frame: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    columns: usize,
    rows: usize,
) -> Result<ContentSample> {
    if width == 0 || height == 0 || stride < width * 4 {
        return Err(CoreError::Invalid("invalid BGRA frame geometry".into()));
    }
    if frame.len() < stride * height {
        return Err(CoreError::Invalid(
            "frame buffer is shorter than its declared geometry".into(),
        ));
    }
    let columns = columns.max(1);
    let rows = rows.max(1);
    let count = (columns * rows) as f64;
    let mut total: u64 = 0;
    let mut total_sq: u64 = 0;
    let mut digest: u32 = 2_166_136_261;
    for row in 0..rows {
        let y = (height - 1).min(((2 * row + 1) * height) / (2 * rows));
        for column in 0..columns {
            let x = (width - 1).min(((2 * column + 1) * width) / (2 * columns));
            let offset = y * stride + x * 4;
            let (blue, green, red) = (
                u64::from(frame[offset]),
                u64::from(frame[offset + 1]),
                u64::from(frame[offset + 2]),
            );
            let luma = (19 * blue + 183 * green + 54 * red) >> 8;
            total += luma;
            total_sq += luma * luma;
            digest = (digest ^ luma as u32).wrapping_mul(16_777_619);
        }
    }
    let mean = total as f64 / count;
    let variance = (total_sq as f64 / count - mean * mean).max(0.0);
    Ok(ContentSample::from_luma(mean, variance, digest))
}

/// Requires sustained suspicious samples before flagging content.
#[derive(Clone, Debug)]
pub struct ContentSuspicionTracker {
    suspect_after_samples: u32,
    suspicious_streak: u32,
    repeat_count: u32,
    last_hash: Option<u32>,
}

impl Default for ContentSuspicionTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ContentSuspicionTracker {
    /// Consecutive suspicious samples needed before flagging.
    pub const SUSPECT_AFTER_SAMPLES: u32 = 12;

    /// A tracker with the reference's threshold.
    pub fn new() -> Self {
        Self::with_threshold(Self::SUSPECT_AFTER_SAMPLES).expect("default threshold is valid")
    }

    /// A tracker with a custom threshold (at least 2).
    pub fn with_threshold(suspect_after_samples: u32) -> Result<Self> {
        if suspect_after_samples < 2 {
            return Err(CoreError::Invalid(
                "content suspicion requires temporal evidence".into(),
            ));
        }
        Ok(Self {
            suspect_after_samples,
            suspicious_streak: 0,
            repeat_count: 0,
            last_hash: None,
        })
    }

    /// Returns true only after sustained black-like or uniform samples.
    pub fn observe(&mut self, sample: &ContentSample) -> bool {
        let suspicious = sample.black_like || sample.uniform;
        self.suspicious_streak = if suspicious {
            self.suspicious_streak + 1
        } else {
            0
        };
        self.repeat_count = if self.last_hash == Some(sample.sample_hash) {
            self.repeat_count + 1
        } else {
            0
        };
        self.last_hash = Some(sample.sample_hash);
        self.suspicious_streak >= self.suspect_after_samples
    }

    /// Convenience for callers that only have luma statistics.
    pub fn observe_luma(&mut self, mean: f64, variance: f64) -> bool {
        let hash = (mean.to_bits() ^ variance.to_bits().rotate_left(17)) as u32;
        self.observe(&ContentSample::from_luma(mean, variance, hash))
    }

    /// How many consecutive identical samples have been seen.
    pub fn repeat_count(&self) -> u32 {
        self.repeat_count
    }
}
