//! The reference's status strings for the capture health states.

use fthrust_core::capture_health::{HealthState, flags};

use crate::state::StatusStyle;

/// `PAUSED — GAME UNFOCUSED`.
pub const FOCUS_PAUSED: &str = "PAUSED — GAME UNFOCUSED";

/// The status text for a health state (`main.py` `state_text`).
pub fn text_for(state: HealthState) -> &'static str {
    match state {
        HealthState::Initializing => "STARTING CAPTURE",
        HealthState::Healthy => "CAPTURING",
        HealthState::Degraded => "CAPTURE DEGRADED",
        HealthState::ContentSuspect => "CAPTURE DEGRADED — DARK / UNIFORM",
        HealthState::Stalled => "CAPTURE STALLED",
        HealthState::Failed => "CAPTURE FAILED",
        HealthState::Recovering => "RECOVERING CAPTURE",
        HealthState::Stopped => "CAPTURE STOPPED",
    }
}

/// The status style: idle while applying, active for the healthy family,
/// idle for the transitional states, warning otherwise.
pub fn style_for(state: HealthState, applying: bool) -> StatusStyle {
    if applying {
        return StatusStyle::Idle;
    }
    match state {
        HealthState::Healthy | HealthState::Degraded | HealthState::ContentSuspect => {
            StatusStyle::Active
        }
        HealthState::Initializing | HealthState::Recovering => StatusStyle::Idle,
        _ => StatusStyle::Warning,
    }
}

/// `_is_focus_pause`: the engine answered the focus pause with
/// ACTIVE|PAUSED, which health maps to DEGRADED.
pub fn focus_paused(state: HealthState, engine_flags: u32) -> bool {
    state == HealthState::Degraded && engine_flags & flags::PAUSED != 0
}
