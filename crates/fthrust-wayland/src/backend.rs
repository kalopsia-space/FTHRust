//! Error vocabulary shared by the protocol backends.

/// Why a backend could not be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// A genuine error: counts toward the retry budget.
    Error,
    /// The display could not be connected or did not answer: waited out.
    CompositorUnreachable,
    /// The selected output is not advertised right now: waited out.
    OutputAbsent,
    /// The cancel flag was set.
    Cancelled,
}

/// An open failure with words for the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendError {
    /// Classification.
    pub kind: FailureKind,
    /// Human-readable reason.
    pub reason: String,
}

impl BackendError {
    pub(crate) fn new(kind: FailureKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for BackendError {}

/// Why a capture call produced no frame; every variant ends the generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureFailure {
    /// The compositor did not answer within the frame deadline.
    Timeout,
    /// The cancel flag was set.
    Cancelled,
    /// The connection is gone.
    Disconnected,
    /// The compositor reported a failure, or a protocol error occurred.
    Failed(String),
}

impl std::fmt::Display for CaptureFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureFailure::Timeout => {
                f.write_str("the compositor did not deliver a frame in time")
            }
            CaptureFailure::Cancelled => f.write_str("capture cancelled"),
            CaptureFailure::Disconnected => f.write_str("the Wayland connection was lost"),
            CaptureFailure::Failed(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for CaptureFailure {}

/// Maps a bounded-wait outcome that was not `EventReceived`.
pub(crate) fn wait_failure(result: crate::dispatch::WaitResult) -> CaptureFailure {
    use crate::dispatch::WaitResult;
    match result {
        WaitResult::Timeout => CaptureFailure::Timeout,
        WaitResult::Cancelled => CaptureFailure::Cancelled,
        WaitResult::Disconnected => CaptureFailure::Disconnected,
        WaitResult::Error | WaitResult::EventReceived => {
            CaptureFailure::Failed("Wayland protocol error".into())
        }
    }
}

/// Maps a capture failure during open to an open failure.
pub(crate) fn open_failure(step: &str, failure: CaptureFailure) -> BackendError {
    match failure {
        CaptureFailure::Cancelled => BackendError::new(FailureKind::Cancelled, "capture cancelled"),
        CaptureFailure::Disconnected => BackendError::new(
            FailureKind::CompositorUnreachable,
            format!("{step}: the Wayland connection was lost"),
        ),
        CaptureFailure::Timeout => BackendError::new(
            FailureKind::CompositorUnreachable,
            format!("{step}: the compositor did not answer"),
        ),
        CaptureFailure::Failed(reason) => {
            BackendError::new(FailureKind::Error, format!("{step}: {reason}"))
        }
    }
}

/// `CLOCK_MONOTONIC` in nanoseconds.
pub fn monotonic_ns() -> i64 {
    let ts =
        nix::time::clock_gettime(nix::time::ClockId::CLOCK_MONOTONIC).expect("CLOCK_MONOTONIC");
    ts.tv_sec() * 1_000_000_000 + ts.tv_nsec()
}
