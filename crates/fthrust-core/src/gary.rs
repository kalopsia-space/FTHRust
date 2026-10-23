//! Gary mode maths (`core/gary_mode.py`): thresholds on the microphone
//! level, the target intensity and the per-tick attack/release step.

/// `DEFAULT_MIN_LEVEL`.
pub const DEFAULT_MIN: i64 = 55;
/// `DEFAULT_MAX_LEVEL`.
pub const DEFAULT_MAX: i64 = 85;
/// `MIN_THRESHOLD_GAP`.
pub const MIN_GAP: i64 = 5;
/// Rise per 33 ms tick.
pub const RISE: f64 = 0.18;
/// Fall per 33 ms tick.
pub const FALL: f64 = 0.065;

/// `clamp_thresholds`: `low` 1–95, `high` at least `low + 5` and at most 100.
pub fn clamp_thresholds(low: i64, high: i64) -> (i64, i64) {
    let low = low.clamp(1, 95);
    (low, high.min(100).max(low + MIN_GAP))
}

/// `intensity_for_level`: 0 at or below `low`%, 1 at or above `high`%,
/// linear between. `level` is 0–1.
pub fn intensity_for_level(level: f64, low: i64, high: i64) -> f64 {
    if !level.is_finite() {
        return 0.0;
    }
    let (low, high) = clamp_thresholds(low, high);
    let (low, high) = (low as f64, high as f64);
    let percent = level.clamp(0.0, 1.0) * 100.0;
    if percent <= low + 1e-9 {
        0.0
    } else if percent >= high - 1e-9 {
        1.0
    } else {
        (percent - low) / (high - low)
    }
}

/// `step_intensity`: towards `target` by [`RISE`] or [`FALL`], landing
/// exactly when within one step.
pub fn step_intensity(current: f64, target: f64) -> f64 {
    let (current, target) = (current.clamp(0.0, 1.0), target.clamp(0.0, 1.0));
    let delta = target - current;
    let step = if delta > 0.0 { RISE } else { FALL }.max(0.001);
    if delta.abs() <= step {
        target
    } else {
        current + step.copysign(delta)
    }
}
