//! Bounded, cancellable Wayland event dispatch (ports `wayland_dispatch.cpp`).

/// How a bounded wait ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitResult {
    /// At least one event was dispatched.
    EventReceived,
    /// The deadline passed with nothing to dispatch.
    Timeout,
    /// `keep_running` returned false.
    Cancelled,
    /// The compositor connection is gone.
    Disconnected,
    /// A protocol or I/O error.
    Error,
}

impl WaitResult {
    /// The reference's name for the result.
    pub fn name(self) -> &'static str {
        match self {
            WaitResult::EventReceived => "event",
            WaitResult::Timeout => "timeout",
            WaitResult::Cancelled => "cancelled",
            WaitResult::Disconnected => "disconnected",
            WaitResult::Error => "error",
        }
    }
}
