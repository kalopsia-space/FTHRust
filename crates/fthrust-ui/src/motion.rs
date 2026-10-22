//! Easing curves and timed transitions (`ui/motion.py`, section 2.5).
//! `reduce_ui_animations` jumps every transition to its end.

use egui::{Context, Id};

use crate::tokens::motion;

/// Qt easing curves the reference uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Ease {
    Linear,
    InOutSine,
    OutCubic,
    InCubic,
    OutBack,
}

impl Ease {
    /// Maps linear progress `t` in `0..=1`.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::Linear => t,
            Ease::InOutSine => -(((std::f32::consts::PI * t).cos()) - 1.0) / 2.0,
            Ease::OutCubic => 1.0 - (1.0 - t).powi(3),
            Ease::InCubic => t * t * t,
            Ease::OutBack => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            }
        }
    }
}

fn in_out_sine(t: f32) -> f32 {
    Ease::InOutSine.apply(t)
}

/// Eased 0..1 blend toward `target`, over `duration_ms`, with the hover
/// curve (InOutSine). Repaints are requested while the blend runs.
pub fn blend_bool(
    ctx: &Context,
    id: Id,
    target: bool,
    duration_ms: u64,
    reduce_motion: bool,
) -> f32 {
    let seconds = if reduce_motion {
        0.0
    } else {
        duration_ms as f32 / 1000.0
    };
    ctx.animate_bool_with_time_and_easing(id, target, seconds, in_out_sine)
}

/// The 120 ms hover blend.
pub fn hover(ctx: &Context, id: Id, hovered: bool, reduce_motion: bool) -> f32 {
    blend_bool(ctx, id, hovered, motion::HOVER_MS, reduce_motion)
}

/// A one-shot timed transition started at `start` (seconds, `Input::time`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    /// Start time.
    pub start: f64,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Curve.
    pub ease: Ease,
}

impl Transition {
    /// Starts now.
    pub fn new(now: f64, duration_ms: u64, ease: Ease) -> Transition {
        Transition {
            start: now,
            duration_ms,
            ease,
        }
    }

    /// Eased progress at `now`; `1.0` when reduced or finished.
    pub fn progress(&self, now: f64, reduce_motion: bool) -> f32 {
        if reduce_motion || self.duration_ms == 0 {
            return 1.0;
        }
        let t = ((now - self.start) * 1000.0 / self.duration_ms as f64) as f32;
        self.ease.apply(t)
    }

    /// Whether the transition still runs at `now`.
    pub fn running(&self, now: f64, reduce_motion: bool) -> bool {
        !reduce_motion && (now - self.start) * 1000.0 < self.duration_ms as f64
    }
}

/// Smooth wheel scrolling: `pos += (target − pos)·(1 − e^(−dt/τ))`,
/// τ = 35 ms, stop when |target − pos| < 0.5.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SmoothScroll {
    /// Where the wheel asked to be.
    pub target: f32,
    /// Where the view is.
    pub pos: f32,
}

impl SmoothScroll {
    /// Adds a wheel step (`−Δ/120 · single_step · wheel_lines`).
    pub fn wheel(&mut self, angle_delta: f32, single_step: f32, wheel_lines: f32) {
        self.target += -angle_delta / 120.0 * single_step * wheel_lines;
    }

    /// Advances by `dt` seconds; returns whether it is still moving.
    pub fn tick(&mut self, dt: f32) -> bool {
        let gap = self.target - self.pos;
        if gap.abs() < 0.5 {
            self.pos = self.target;
            return false;
        }
        let tau = motion::SCROLL_TAU_MS as f32 / 1000.0;
        self.pos += gap * (1.0 - (-dt / tau).exp());
        true
    }

    /// Snaps to the target.
    pub fn settle(&mut self) {
        self.pos = self.target;
    }
}
