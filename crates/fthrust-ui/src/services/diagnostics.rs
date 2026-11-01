//! The diagnostics session (`~/.fthr/diagnostics/session-…/events.jsonl`).

use std::sync::Arc;

use fthrust_core::FthrPaths;
use fthrust_core::diagnostics::session::DiagnosticsSession;
use serde_json::{Value, json};

use crate::shell::error_bar::Level;
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

/// Tells the log, the diagnostics session and the user what the settings and
/// theme stores could not do while loading (an unreadable or damaged file, a
/// migration or folder that could not be written). The app carries on with
/// what it has: a full disk must not keep the user from opening it to make
/// room.
///
/// The notice goes into the error bar whatever `error_notifications_enabled`
/// says (it is `push`, not `push_error`, which that setting gates): these say
/// that something was not read or will not be saved, and a preference about
/// error pop-ups must not hide that. Titled `STARTUP PROBLEM`, the detail
/// bounded to the bar's 240 characters.
pub fn report_startup_issues(state: &mut AppState) {
    let issues: Vec<String> = state
        .settings
        .startup_issues()
        .iter()
        .chain(state.theme.startup_issues())
        .cloned()
        .chain(state.hotkeys.startup_issue())
        .collect();
    for (i, issue) in issues.iter().enumerate() {
        tracing::error!("{issue}");
        emit(
            state,
            "startup",
            "store_problem",
            json!({ "detail": issue }),
        );
        state.error_bar.push(
            "STARTUP PROBLEM",
            &issue.chars().take(240).collect::<String>(),
            Level::Warning,
            Vec::new(),
            Some(format!("startup-store-{i}")),
            false,
        );
    }
}

/// Records one event on the live session (no-op without one).
pub fn emit(state: &AppState, subsystem: &str, event: &str, fields: Value) {
    if let Some(s) = &state.diagnostics {
        s.event(subsystem, event, fields);
    }
}
