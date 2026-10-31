//! The diagnostics session (`~/.fthr/diagnostics/session-…/events.jsonl`).

use std::sync::Arc;

use fthrust_core::FthrPaths;
use fthrust_core::diagnostics::session::DiagnosticsSession;
use serde_json::Value;

use crate::state::AppState;

/// Starts a session. When it cannot start (a full or read-only disk, a file
/// where the directory should be) the failure is logged and there is no
/// session: [`AppState::diagnostics`] is `None`, [`emit`] does nothing and the
/// app opens all the same. Diagnostics are never worth stopping the window
/// for, least of all on a disk that is full: the user may need the app to
/// delete clips and make room.
pub fn start(paths: &FthrPaths) -> Option<Arc<DiagnosticsSession>> {
    match DiagnosticsSession::start(paths, crate::VERSION) {
        Ok(session) => Some(Arc::new(session)),
        Err(e) => {
            tracing::warn!("diagnostics session not started: {e}");
            None
        }
    }
}

/// Records one event on the live session (no-op without one).
pub fn emit(state: &AppState, subsystem: &str, event: &str, fields: Value) {
    if let Some(s) = &state.diagnostics {
        s.event(subsystem, event, fields);
    }
}
