//! Backend selection and recovery rules, kept free of compositors and
//! FFmpeg so they are tested with plain values. Ports
//! `capture_backend_policy.cpp` and `recovery_policy.h`.

use std::time::Duration;

use fthrust_ipc::BackendKind;
use fthrust_wayland::dispatch::WaitResult;

/// First-time selection order on Wayland: ext-image-copy-capture (the spec's
/// preference), then wlr-screencopy, then the portal because it needs a
/// dialog. Without Wayland there is nothing (no X11 fallback).
pub fn backend_candidates(has_wayland: bool) -> Vec<BackendKind> {
    if has_wayland {
        vec![
            BackendKind::ExtImageCopy,
            BackendKind::WlrScreencopy,
            BackendKind::PortalScreenCast,
        ]
    } else {
        Vec::new()
    }
}

/// Recovery order: the backend that last delivered frames first, then the
/// remaining candidates (the spec's fall-through fix), never the portal
/// unless it was the last good one (its dialog must not reopen on its own).
pub fn recovery_order(last_good: BackendKind, candidates: &[BackendKind]) -> Vec<BackendKind> {
    let mut out = vec![last_good];
    out.extend(
        candidates
            .iter()
            .copied()
            .filter(|k| *k != last_good && *k != BackendKind::PortalScreenCast),
    );
    out
}

/// What to do after a generation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryDecision {
    /// Start another generation after `backoff`.
    pub retry: bool,
    /// The budget is spent; report `BACKEND_FAILED`.
    pub exhausted: bool,
    /// Attempts counted so far (transient waits do not count).
    pub attempt: u32,
    /// How long to wait first.
    pub backoff: Duration,
}

/// Bounded retry budget with a healthy-generation reset.
#[derive(Clone, Debug)]
pub struct CaptureRecoveryPolicy {
    healthy_reset_frames: u64,
    max_attempts: u32,
    attempts: u32,
}

impl CaptureRecoveryPolicy {
    /// Wait between transient retries.
    pub const TRANSIENT_BACKOFF: Duration = Duration::from_millis(2000);
    /// Backoff per counted attempt (the last repeats).
    pub const BACKOFFS: [Duration; 3] = [
        Duration::from_millis(250),
        Duration::from_millis(750),
        Duration::from_millis(1500),
    ];

    /// `healthy_reset_frames` produced frames reset the budget;
    /// `max_attempts` counted failures exhaust it.
    pub fn new(healthy_reset_frames: u64, max_attempts: u32) -> Self {
        Self {
            healthy_reset_frames: healthy_reset_frames.max(1),
            max_attempts: max_attempts.max(1),
            attempts: 0,
        }
    }

    /// A generation ended after `produced_frames`. Transient failures (the
    /// compositor is not answering, the selected output is absent) are
    /// waited out without consuming the budget.
    pub fn on_generation_failed(
        &mut self,
        produced_frames: u64,
        transient: bool,
    ) -> RecoveryDecision {
        if transient {
            return RecoveryDecision {
                retry: true,
                exhausted: false,
                attempt: self.attempts,
                backoff: Self::TRANSIENT_BACKOFF,
            };
        }
        if produced_frames >= self.healthy_reset_frames {
            self.attempts = 0;
        }
        self.attempts += 1;
        if self.attempts > self.max_attempts {
            return RecoveryDecision {
                retry: false,
                exhausted: true,
                attempt: self.attempts,
                backoff: Duration::ZERO,
            };
        }
        let index = (self.attempts as usize - 1).min(Self::BACKOFFS.len() - 1);
        RecoveryDecision {
            retry: true,
            exhausted: false,
            attempt: self.attempts,
            backoff: Self::BACKOFFS[index],
        }
    }

    /// Counted attempts so far.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

/// Half a second of consecutive encode failures (never fewer than five)
/// ends the generation so the encoder is reopened.
#[derive(Clone, Debug)]
pub struct EncodeFailurePolicy {
    threshold: u32,
    consecutive: u32,
}

impl EncodeFailurePolicy {
    /// Threshold `max(5, fps / 2)`.
    pub fn new(fps: u32) -> Self {
        Self {
            threshold: (fps / 2).max(5),
            consecutive: 0,
        }
    }

    /// Returns true when the generation must give up on this encoder.
    pub fn on_encode_result(&mut self, ok: bool) -> bool {
        if ok {
            self.consecutive = 0;
            return false;
        }
        self.consecutive += 1;
        self.consecutive >= self.threshold
    }

    /// Consecutive failures so far.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive
    }
}

/// The capture loop encodes at most one frame per `frame_ns`. After a
/// stall the next deadline would lie far in the past; re-anchor on the
/// first frame that is more than two periods late instead of bursting.
pub fn next_encode_deadline(next_encode_ns: i64, frame_ns: i64, frame_timestamp_ns: i64) -> i64 {
    if frame_timestamp_ns > next_encode_ns + 2 * frame_ns {
        frame_timestamp_ns
    } else {
        next_encode_ns
    }
}

/// Sleeps `remaining` in slices of at most 50 ms while `is_running`;
/// returns whether the wait completed without cancellation.
pub fn wait_for_recovery_backoff(
    mut remaining: Duration,
    is_running: impl Fn() -> bool,
    mut sleep: impl FnMut(Duration),
) -> bool {
    const SLICE: Duration = Duration::from_millis(50);
    while is_running() && remaining > Duration::ZERO {
        let slice = remaining.min(SLICE);
        sleep(slice);
        remaining -= slice;
    }
    is_running()
}

/// Only a plain timeout may repeat the previous frame: the compositor may
/// hold a request until the screen changes, whereas any other result means
/// the connection or session is gone.
pub fn should_repeat_last_frame(result: WaitResult, have_last_frame: bool) -> bool {
    result == WaitResult::Timeout && have_last_frame
}
