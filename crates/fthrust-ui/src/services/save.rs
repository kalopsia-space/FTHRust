//! The save-clip flow (`main.py` `_save_clip` and `_on_save_outcome`):
//! admission through capture health, the reference's clip paths, the
//! single-flight save state machine with its 1 s acknowledgement and 60 s
//! completion timeouts, and the outcomes.

use std::collections::HashMap;
use std::path::PathBuf;

use fthrust_core::capture_health::{HealthSnapshot, HealthState, evaluate_save_admission};
use fthrust_core::clip_files::{replay_clip_path, unique_path};
use fthrust_core::save_state::{EngineEvent, OutcomeKind, SaveStateMachine};
use fthrust_ipc::{Command, Event as IpcEvent, SavedClip};
use serde_json::json;
use time::OffsetDateTime;

use super::CardKind;
use crate::shell::error_bar::{Action, Level};
use crate::state::{AppState, StatusStyle, StatusText};

/// Presses closer than this are ignored (`_last_save_request`).
pub const SPAM_GUARD_S: f64 = 1.0;
/// How long `SAVE FAILED` / the busy `SAVING` stays before the engine text returns.
pub const FAILED_STATUS_S: f64 = 3.0;
/// The busy notice duration.
pub const BUSY_STATUS_S: f64 = 1.0;

/// What the save service wants done.
#[derive(Debug)]
pub enum SaveAction {
    /// Send a command to the engine.
    Send(Command),
    /// Show a card.
    Card(CardKind),
    /// Play a sound.
    Sound(String),
    /// Push an error-bar entry.
    Error {
        /// Title.
        title: String,
        /// Detail.
        detail: String,
        /// Level.
        level: Level,
        /// Actions.
        actions: Vec<Action>,
    },
    /// A diagnostics event.
    Diag {
        /// Event name.
        event: String,
        /// Fields.
        fields: serde_json::Value,
    },
}

/// The service.
pub struct SaveService {
    machine: SaveStateMachine,
    last_request: f64,
    next_request_id: u64,
    paths: HashMap<u64, PathBuf>,
    transient: Option<(StatusText, Option<f64>)>,
}

impl Default for SaveService {
    fn default() -> Self {
        Self::new()
    }
}

impl SaveService {
    /// Idle.
    pub fn new() -> Self {
        SaveService {
            machine: SaveStateMachine::with_defaults(),
            last_request: -10.0,
            next_request_id: 1,
            paths: HashMap::new(),
            transient: None,
        }
    }

    /// A save is in flight.
    pub fn is_busy(&self) -> bool {
        self.machine.is_busy()
    }

    fn show(&mut self, text: &str, style: StatusStyle, until: Option<f64>) {
        self.transient = Some((
            StatusText {
                text: text.into(),
                style,
            },
            until,
        ));
    }

    /// The transient status to show instead of the engine's, if any.
    pub fn transient_status(&mut self, now: f64) -> Option<StatusText> {
        if let Some((_, Some(until))) = &self.transient
            && now >= *until
        {
            self.transient = None;
        }
        self.transient.as_ref().map(|(t, _)| t.clone())
    }

    /// `_save_clip`: validates, names the file and submits the command.
    #[allow(clippy::too_many_arguments)]
    pub fn save_clip(
        &mut self,
        state: &mut AppState,
        seconds: u32,
        connected: bool,
        health: Option<&HealthSnapshot>,
        recording_transition: bool,
        now: f64,
        wall: OffsetDateTime,
    ) -> Vec<SaveAction> {
        let mut out = Vec::new();
        if recording_transition {
            self.show("SAVING", StatusStyle::Idle, Some(now + BUSY_STATUS_S));
            return out;
        }
        if !connected {
            out.push(SaveAction::Error {
                title: "CAPTURE UNAVAILABLE".into(),
                detail: "The capture engine is not connected, so there is no replay buffer available to save.".into(),
                level: Level::Error,
                actions: vec![],
            });
            return out;
        }
        let admission = evaluate_save_admission(health, seconds);
        if !admission.allowed {
            out.push(SaveAction::Card(CardKind::Error {
                detail: String::new(),
            }));
            out.push(SaveAction::Error {
                title: "CLIP NOT SAVED".into(),
                detail: admission.reason.into(),
                level: Level::Error,
                actions: vec![Action {
                    label: "RESTART CAPTURE".into(),
                    id: "restart-capture".into(),
                }],
            });
            tracing::warn!(
                state = ?health.map(|h| h.state).unwrap_or(HealthState::Stopped),
                reason = admission.reason,
                "save rejected by capture health"
            );
            return out;
        }
        let seconds = admission.duration_seconds;
        if now - self.last_request < SPAM_GUARD_S || self.machine.is_busy() {
            self.show("SAVING", StatusStyle::Idle, Some(now + BUSY_STATUS_S));
            return out;
        }
        self.last_request = now;
        let root = match state.settings.clips_directory() {
            Ok(r) => r,
            Err(e) => {
                out.push(SaveAction::Card(CardKind::Error {
                    detail: String::new(),
                }));
                out.push(SaveAction::Error {
                    title: "CLIP SAVE FAILED".into(),
                    detail: e.to_string(),
                    level: Level::Error,
                    actions: vec![],
                });
                return out;
            }
        };
        let game = if state.settings.get_str("capture_mode") == Some("window") {
            let name = state
                .settings
                .get_str("target_window_name")
                .unwrap_or("")
                .trim()
                .to_owned();
            Some(if name.is_empty() {
                "Unknown".to_owned()
            } else {
                name
            })
        } else {
            None
        };
        let path = unique_path(replay_clip_path(&root, game.as_deref(), wall));
        if let Some(folder) = path.parent()
            && let Err(e) = std::fs::create_dir_all(folder)
        {
            out.push(SaveAction::Card(CardKind::Error {
                detail: String::new(),
            }));
            out.push(SaveAction::Error {
                title: "CLIP SAVE FAILED".into(),
                detail: format!("Cannot create {}: {e}", folder.display()),
                level: Level::Error,
                actions: vec![],
            });
            return out;
        }
        out.push(SaveAction::Diag {
            event: "save_requested".into(),
            fields: json!({"requested_duration_seconds": seconds, "capture_mode": if game.is_some() { "window" } else { "desktop" }}),
        });
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        match self.machine.submit(&path, seconds, now) {
            Ok(_) => {}
            Err(_) => {
                self.show("SAVING", StatusStyle::Idle, Some(now + BUSY_STATUS_S));
                return out;
            }
        }
        self.paths.insert(request_id, path.clone());
        out.push(SaveAction::Send(Command::SaveClip {
            request_id,
            path,
            seconds,
        }));
        // Immediate feedback; a later failure replaces it.
        let (fps, res) = capture_summary(state);
        out.push(SaveAction::Card(CardKind::Clip {
            seconds,
            fps,
            resolution: res,
        }));
        out.push(SaveAction::Diag {
            event: "command_submitted".into(),
            fields: json!({"request_id": request_id}),
        });
        self.show("SAVING", StatusStyle::Idle, None);
        out
    }

    /// An engine event (`SaveStarted`, `SaveProgress`, `SaveResult`).
    pub fn on_engine_event(
        &mut self,
        state: &mut AppState,
        ev: &IpcEvent,
        now: f64,
    ) -> Vec<SaveAction> {
        let mut out = Vec::new();
        match ev {
            IpcEvent::SaveStarted { .. } => {
                if let Some(o) = self.machine.on_event(EngineEvent::SaveStarted, "", now) {
                    self.outcome(state, o.kind, &o.detail, o.late, None, &mut out, now);
                }
            }
            IpcEvent::SaveResult { request_id, result } => {
                let expected = self.paths.remove(request_id);
                let (event, detail, saved) = match result {
                    Ok(saved) => (EngineEvent::ClipSaved, String::new(), Some(saved.clone())),
                    Err(e) => (EngineEvent::ErrorOccurred, e.clone(), None),
                };
                if let Some(o) = self.machine.on_event(event, &detail, now) {
                    let _ = expected;
                    self.outcome(
                        state,
                        o.kind,
                        &o.detail,
                        o.late,
                        saved.as_ref(),
                        &mut out,
                        now,
                    );
                }
            }
            _ => {}
        }
        out
    }

    /// Timeouts (1 s acknowledgement, 60 s completion).
    pub fn on_tick(&mut self, state: &mut AppState, now: f64) -> Vec<SaveAction> {
        let mut out = Vec::new();
        if let Some(o) = self.machine.on_tick(now) {
            self.outcome(state, o.kind, &o.detail, o.late, None, &mut out, now);
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn outcome(
        &mut self,
        state: &mut AppState,
        kind: OutcomeKind,
        detail: &str,
        late: bool,
        saved: Option<&SavedClip>,
        out: &mut Vec<SaveAction>,
        now: f64,
    ) {
        let path = self
            .machine
            .current()
            .map(|op| op.output_path.clone())
            .unwrap_or_default();
        match kind {
            OutcomeKind::Accepted => {
                self.show("SAVING", StatusStyle::Idle, None);
                out.push(SaveAction::Diag {
                    event: "engine_accepted".into(),
                    fields: json!({}),
                });
            }
            OutcomeKind::TimeoutNotice => {
                self.show("SAVE SLOW…", StatusStyle::Warning, None);
                out.push(SaveAction::Diag {
                    event: "save_slow".into(),
                    fields: json!({}),
                });
            }
            OutcomeKind::Failed => {
                out.push(SaveAction::Card(CardKind::Error {
                    detail: String::new(),
                }));
                self.show(
                    "SAVE FAILED",
                    StatusStyle::Warning,
                    Some(now + FAILED_STATUS_S),
                );
                let detail = if detail.is_empty() {
                    "The capture engine failed while writing the clip. Check free disk space and the configured clips storage folder.".to_owned()
                } else {
                    detail.to_owned()
                };
                out.push(SaveAction::Error {
                    title: "CLIP WAS NOT SAVED".into(),
                    detail,
                    level: Level::Error,
                    actions: vec![Action {
                        label: "OPEN FOLDER".into(),
                        id: "open-folder".into(),
                    }],
                });
                out.push(SaveAction::Diag {
                    event: "save_failed".into(),
                    fields: json!({}),
                });
            }
            OutcomeKind::Completed => {
                let path = saved.map(|s| s.path.clone()).unwrap_or(path);
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                if !fthrust_core::clip_files::is_completed_video_path(&path) || size == 0 {
                    out.push(SaveAction::Card(CardKind::Error {
                        detail: String::new(),
                    }));
                    self.show(
                        "SAVE FAILED",
                        StatusStyle::Warning,
                        Some(now + FAILED_STATUS_S),
                    );
                    out.push(SaveAction::Error {
                        title: "CLIP WAS NOT SAVED".into(),
                        detail: "The capture engine reported success but no clip file was written. Check free disk space and the configured clips storage folder.".into(),
                        level: Level::Error,
                        actions: vec![Action {
                            label: "OPEN FOLDER".into(),
                            id: "open-folder".into(),
                        }],
                    });
                    return;
                }
                out.push(SaveAction::Diag {
                    event: "mux_completed".into(),
                    fields: json!({"late": late, "bytes": size}),
                });
                state.readiness.engine_committed(&path, false);
                state.readiness.ready(&path);
                if state.library.upsert(&path, false) {
                    state.library_dirty = true;
                }
                if let Some(saved) = saved
                    && let Some(err) = &saved.audio_error
                {
                    out.push(SaveAction::Error {
                        title: "CLIP SAVED WITHOUT AUDIO".into(),
                        detail: err.clone(),
                        level: Level::Warning,
                        actions: vec![],
                    });
                }
                self.transient = None;
            }
        }
    }
}

fn capture_summary(state: &AppState) -> (u32, String) {
    let fps = state.settings.get_i64("framerate").unwrap_or(60).max(1) as u32;
    let res = state
        .settings
        .get_str("resolution")
        .unwrap_or("source")
        .to_uppercase();
    (fps, res)
}
