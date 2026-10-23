//! The diagnostics session (`~/.fthr/diagnostics/session-…/events.jsonl`).

use std::sync::Arc;

use fthrust_core::FthrPaths;
use fthrust_core::diagnostics::session::DiagnosticsSession;
use serde_json::Value;

use crate::state::AppState;

/// Starts a session; a failure logs and leaves diagnostics off.
pub fn start(paths: &FthrPaths) -> Arc<DiagnosticsSession> {
    match DiagnosticsSession::start(paths, crate::VERSION) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            tracing::warn!("diagnostics session not started: {e}");
            // A session that cannot write still answers; events are dropped.
            Arc::new(
                DiagnosticsSession::start(
                    &FthrPaths::new(std::env::temp_dir().join("fthrust-diag-fallback")),
                    crate::VERSION,
                )
                .expect("fallback diagnostics session"),
            )
        }
    }
}

/// Records one event on the live session (no-op without one).
pub fn emit(state: &AppState, subsystem: &str, event: &str, fields: Value) {
    if let Some(s) = &state.diagnostics {
        s.event(subsystem, event, fields);
    }
}
