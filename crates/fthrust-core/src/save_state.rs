//! Single-flight save state machine driven by the UI poller.
//!
//! Ports `reference/FTHR_UI/core/save_state.py`. The engine has one response
//! slot with no request id, so the poller interprets events and deadlines
//! here without any I/O. Timestamps are monotonic seconds supplied by the
//! caller.

use std::path::{Path, PathBuf};

/// Lifecycle of a single engine save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveState {
    /// Resting state: no operation exists.
    Idle,
    /// The command was written but not yet polled.
    Requested,
    /// Polled at least once with no response yet.
    WaitingForAck,
    /// `SAVE_STARTED` was seen.
    EngineAccepted,
    /// `CLIP_SAVED` was seen (terminal).
    EngineCompleted,
    /// `ERROR_OCCURRED` was seen or the completion deadline passed (terminal).
    EngineFailed,
    /// No acknowledgement in time; still watched for a late result.
    TimedOut,
}

impl SaveState {
    /// The reference's state name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Requested => "REQUESTED",
            Self::WaitingForAck => "WAITING_FOR_ACK",
            Self::EngineAccepted => "ENGINE_ACCEPTED",
            Self::EngineCompleted => "ENGINE_COMPLETED",
            Self::EngineFailed => "ENGINE_FAILED",
            Self::TimedOut => "TIMED_OUT",
        }
    }

    fn is_in_flight(self) -> bool {
        matches!(
            self,
            Self::Requested | Self::WaitingForAck | Self::EngineAccepted
        )
    }
}

/// What the poller read from the engine's response slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineEvent {
    /// The engine acknowledged the save.
    SaveStarted,
    /// The clip was written.
    ClipSaved,
    /// The save failed.
    ErrorOccurred,
}

/// Save results and non-terminal notices for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeKind {
    /// The engine acknowledged; the UI may show progress.
    Accepted,
    /// Result: the clip was written.
    Completed,
    /// Result: the clip was not written.
    Failed,
    /// Not a result: the save is slow and still watched.
    TimeoutNotice,
}

/// Everything known about one save.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveOperation {
    /// Monotonic operation id, starting at 1.
    pub op_id: u64,
    /// Path the engine was asked to write.
    pub output_path: PathBuf,
    /// Requested clip length in seconds.
    pub duration_seconds: u32,
    /// When the command was handed over.
    pub requested_at: f64,
    /// When a missing acknowledgement becomes a timeout notice.
    pub ack_deadline: f64,
    /// When a missing result becomes a failure.
    pub completion_deadline: f64,
    /// Current lifecycle state.
    pub state: SaveState,
    /// Detail of the failure, if any.
    pub error_detail: String,
    /// True once a `Completed` or `Failed` outcome has been handed out.
    pub result_emitted: bool,
    /// When `SAVE_STARTED` was seen.
    pub accepted_at: Option<f64>,
    /// When the operation finished (success, failure or give-up).
    pub finished_at: Option<f64>,
    /// When the acknowledgement timed out.
    pub timed_out_at: Option<f64>,
}

impl SaveOperation {
    /// Milliseconds from request to acknowledgement, when acknowledged.
    pub fn ack_latency_ms(&self) -> Option<f64> {
        self.accepted_at.map(|t| (t - self.requested_at) * 1000.0)
    }

    /// Milliseconds from request to finish, when finished.
    pub fn total_latency_ms(&self) -> Option<f64> {
        self.finished_at.map(|t| (t - self.requested_at) * 1000.0)
    }
}

/// One thing that happened, to be acted on exactly once.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// What happened.
    pub kind: OutcomeKind,
    /// The operation it belongs to.
    pub op_id: u64,
    /// The operation's output path.
    pub output_path: PathBuf,
    /// Engine detail text, if any.
    pub detail: String,
    /// True when the result arrived after a timeout notice was issued.
    pub late: bool,
}

/// Why a submit was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// Another save is in flight.
    Busy,
}

/// Default acknowledgement timeout in seconds.
pub const DEFAULT_ACK_TIMEOUT_S: f64 = 1.0;
/// Default completion timeout in seconds.
pub const DEFAULT_COMPLETION_TIMEOUT_S: f64 = 60.0;

/// Tracks one save at a time.
#[derive(Clone, Debug)]
pub struct SaveStateMachine {
    ack_timeout_s: f64,
    completion_timeout_s: f64,
    next_op_id: u64,
    current: Option<SaveOperation>,
}

impl SaveStateMachine {
    /// Creates a machine with explicit timeouts.
    pub fn new(ack_timeout_s: f64, completion_timeout_s: f64) -> Self {
        Self {
            ack_timeout_s,
            completion_timeout_s,
            next_op_id: 1,
            current: None,
        }
    }

    /// Creates a machine with the reference's default timeouts.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_ACK_TIMEOUT_S, DEFAULT_COMPLETION_TIMEOUT_S)
    }

    /// The current operation's state, or `Idle`.
    pub fn state(&self) -> SaveState {
        self.current.as_ref().map_or(SaveState::Idle, |op| op.state)
    }

    /// The current (or most recent) operation.
    pub fn current(&self) -> Option<&SaveOperation> {
        self.current.as_ref()
    }

    /// True while a save is in flight and a second one must be refused.
    /// A timed-out operation is not busy: it is still watched for a late
    /// result, but the user may try again.
    pub fn is_busy(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|op| op.state.is_in_flight() && !op.result_emitted)
    }

    /// Registers a save after its command was handed to the engine.
    pub fn submit(
        &mut self,
        output_path: impl AsRef<Path>,
        duration_seconds: u32,
        now: f64,
    ) -> std::result::Result<&SaveOperation, RejectReason> {
        if self.is_busy() {
            tracing::info!(
                state = self.state().as_str(),
                "rejected save (single-flight)"
            );
            return Err(RejectReason::Busy);
        }
        let op = SaveOperation {
            op_id: self.next_op_id,
            output_path: output_path.as_ref().to_path_buf(),
            duration_seconds,
            requested_at: now,
            ack_deadline: now + self.ack_timeout_s,
            completion_deadline: now + self.completion_timeout_s,
            state: SaveState::Requested,
            error_detail: String::new(),
            result_emitted: false,
            accepted_at: None,
            finished_at: None,
            timed_out_at: None,
        };
        self.next_op_id += 1;
        tracing::info!(op_id = op.op_id, duration_seconds, "save requested");
        self.current = Some(op);
        Ok(self.current.as_ref().expect("just set"))
    }

    /// Interprets one engine response. Returns an outcome to act on, if any.
    pub fn on_event(&mut self, event: EngineEvent, detail: &str, now: f64) -> Option<Outcome> {
        let Some(op) = self.current.as_mut().filter(|op| !op.result_emitted) else {
            tracing::warn!(
                ?event,
                detail,
                "unexpected engine response with no operation to own it"
            );
            return None;
        };
        match event {
            EngineEvent::SaveStarted => match op.state {
                SaveState::EngineAccepted => None,
                SaveState::TimedOut => {
                    tracing::info!(op_id = op.op_id, "late SAVE_STARTED after timeout");
                    None
                }
                _ => Some(transition(
                    op,
                    SaveState::EngineAccepted,
                    now,
                    OutcomeKind::Accepted,
                    "",
                    false,
                )),
            },
            EngineEvent::ClipSaved => {
                let late = op.state == SaveState::TimedOut;
                op.finished_at = Some(now);
                if late {
                    tracing::info!(op_id = op.op_id, "late success after timeout");
                }
                Some(transition(
                    op,
                    SaveState::EngineCompleted,
                    now,
                    OutcomeKind::Completed,
                    detail,
                    late,
                ))
            }
            EngineEvent::ErrorOccurred => {
                let late = op.state == SaveState::TimedOut;
                op.finished_at = Some(now);
                op.error_detail = detail.to_string();
                if late {
                    tracing::info!(op_id = op.op_id, "late failure after timeout");
                }
                Some(transition(
                    op,
                    SaveState::EngineFailed,
                    now,
                    OutcomeKind::Failed,
                    detail,
                    late,
                ))
            }
        }
    }

    /// Advances deadlines. Call once per poll when no response was read.
    pub fn on_tick(&mut self, now: f64) -> Option<Outcome> {
        let ack_timeout_s = self.ack_timeout_s;
        let completion_timeout_s = self.completion_timeout_s;
        let op = self.current.as_mut().filter(|op| !op.result_emitted)?;
        if op.state == SaveState::Requested {
            op.state = SaveState::WaitingForAck;
        }
        if op.state == SaveState::WaitingForAck && now >= op.ack_deadline {
            op.state = SaveState::TimedOut;
            op.timed_out_at = Some(now);
            tracing::warn!(
                op_id = op.op_id,
                "no acknowledgement within {ack_timeout_s:.1}s; still watching for a late result"
            );
            return Some(Outcome {
                kind: OutcomeKind::TimeoutNotice,
                op_id: op.op_id,
                output_path: op.output_path.clone(),
                detail: String::new(),
                late: false,
            });
        }
        if now >= op.completion_deadline {
            op.finished_at = Some(now);
            if op.error_detail.is_empty() {
                op.error_detail =
                    "The capture engine never reported whether the clip was written.".to_string();
            }
            tracing::warn!(
                op_id = op.op_id,
                "giving up after {completion_timeout_s:.0}s"
            );
            let detail = op.error_detail.clone();
            return Some(transition(
                op,
                SaveState::EngineFailed,
                now,
                OutcomeKind::Failed,
                &detail,
                false,
            ));
        }
        None
    }

    /// Abandons the in-flight operation without emitting a result.
    pub fn cancel_active(&mut self, now: f64) -> Option<Outcome> {
        if let Some(op) = self.current.as_mut().filter(|op| !op.result_emitted) {
            tracing::info!(op_id = op.op_id, "save abandoned");
            op.result_emitted = true;
            op.finished_at = Some(now);
        }
        None
    }
}

fn transition(
    op: &mut SaveOperation,
    new_state: SaveState,
    now: f64,
    kind: OutcomeKind,
    detail: &str,
    late: bool,
) -> Outcome {
    let old = op.state;
    op.state = new_state;
    if new_state == SaveState::EngineAccepted {
        op.accepted_at = Some(now);
    }
    tracing::info!(
        op_id = op.op_id,
        from = old.as_str(),
        to = new_state.as_str(),
        "save state"
    );
    if matches!(kind, OutcomeKind::Completed | OutcomeKind::Failed) {
        op.result_emitted = true;
    }
    Outcome {
        kind,
        op_id: op.op_id,
        output_path: op.output_path.clone(),
        detail: detail.to_string(),
        late,
    }
}
