//! The engine supervisor: a pure state machine (`Supervisor`) fed with
//! inputs (requests, IPC events, process exits, ticks) that emits UI
//! events and driver effects, and the driver that spawns `fthrust-engine`,
//! connects over the socket and turns effects into actions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fthrust_core::capture_config::{CaptureConfig, CaptureConfigTracker, linux_ring_budget_mb};
use fthrust_core::capture_health::{CaptureHealthMonitor, HealthSnapshot, HealthState, flags};
use fthrust_core::recovery_tracker::{
    RecoverableIncident, RecoveryKind, RecoveryTracker, advance_recoverable_incidents,
};
use fthrust_ipc::transport::EngineClient;
use fthrust_ipc::{BackendKind, Command, EngineConfig, Event as IpcEvent, StatusSnapshot};

use super::status::{FOCUS_PAUSED, focus_paused, style_for, text_for};
use super::{Event, Outbox};
use crate::shell::error_bar::{Action, Level};
use crate::state::{AppState, StatusStyle};

/// Connection attempts after spawning (60 × 150 ms).
pub const CONNECT_ATTEMPTS: u32 = 60;
/// Pause between attempts.
pub const CONNECT_INTERVAL: Duration = Duration::from_millis(150);
/// The handshake deadline (9 s).
pub const HANDSHAKE_S: f64 = 9.0;
/// Settings apply deadline.
pub const APPLY_TIMEOUT_S: f64 = 30.0;
/// No event for this long means the engine is gone.
pub const SILENCE_S: f64 = 3.0;
/// Pause between stop and start on a restart.
pub const RESTART_GAP_S: f64 = 1.0;
/// Retry a deferred restart this often while a save runs.
pub const RESTART_DEFER_S: f64 = 0.5;
/// A crashed or hung engine is restarted after this long...
pub const AUTO_RESTART_DELAY_S: f64 = 2.0;
/// ...at most this many times...
pub const AUTO_RESTART_LIMIT: usize = 3;
/// ...within this window; after that it stays stopped until the user
/// restarts it.
pub const AUTO_RESTART_WINDOW_S: f64 = 600.0;
/// A connected engine silent this long is taken as hung and restarted.
pub const HANG_RESTART_S: f64 = 15.0;
/// How long a graceful stop waits for the engine to exit before SIGTERM.
pub const KILL_GRACE_S: f64 = 2.0;
/// The same with a clip save in flight: the engine finishes the save first
/// (it waits up to 60 s for it), so it gets that long plus a margin.
pub const SAVE_EXIT_GRACE_S: f64 = 65.0;

/// What drives the supervisor.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// Launch with this configuration.
    Start(EngineConfig),
    /// Stop the engine.
    Stop,
    /// Restart with a new configuration.
    Restart {
        /// The configuration.
        config: EngineConfig,
        /// Allowed while the recording service is preparing a profile.
        allow_recording_prepare: bool,
        /// An explicit restart (shortcut, error bar) shows `RESTARTING
        /// ENGINE`; a settings apply shows `APPLYING SETTINGS`.
        explicit: bool,
    },
    /// Focus pause.
    Pause,
    /// Focus resume.
    Resume,
    /// The process started.
    Spawned {
        /// Its pid.
        pid: u32,
    },
    /// The process could not start.
    SpawnFailed {
        /// Why.
        detail: String,
        /// The binary is missing (Error 003) rather than failing (Error 005).
        missing: bool,
    },
    /// The IPC handshake completed.
    Hello {
        /// Engine pid.
        pid: u32,
        /// Engine version.
        version: String,
    },
    /// `StartupWarning` from the engine.
    StartupWarning {
        /// Code.
        code: String,
        /// Detail.
        detail: String,
    },
    /// `StartupError` from the engine (or parsed from its stderr).
    StartupError {
        /// Code.
        code: String,
        /// Detail.
        detail: String,
    },
    /// `Configured`.
    Configured {
        /// Clamped fields.
        warnings: Vec<String>,
    },
    /// A status snapshot.
    Status(StatusSnapshot),
    /// A heartbeat.
    Heartbeat,
    /// The IPC connection closed.
    Disconnected,
    /// The process exited.
    ProcessExited {
        /// Exit code (`None` on a signal).
        code: Option<i32>,
    },
    /// Send a command when connected (save / recording services).
    Send(Command),
    /// Time passed (≈100 ms).
    Tick,
}

/// What the supervisor asks the driver or the UI to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Spawn the engine process.
    Spawn,
    /// Start connecting to the socket.
    Connect,
    /// Send a command.
    Send(Command),
    /// Stop the process (`Shutdown`, then SIGTERM, then SIGKILL).
    Kill {
        /// Ask nicely first.
        graceful: bool,
        /// Seconds to wait for a graceful exit before SIGTERM.
        grace_s: f64,
    },
    /// Spawn again after this many seconds.
    StartAfter(f64),
    /// An event for the UI.
    Ui(Event),
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Stopped,
    Spawning,
    Connecting { since: f64 },
    Connected,
    Stopping,
}

/// The engine supervisor.
pub struct Supervisor {
    generation: u64,
    process_generation: Option<u64>,
    phase: Phase,
    config: Option<EngineConfig>,
    pending_launch: Option<(EngineConfig, u64)>,
    tracker: CaptureConfigTracker,
    health: CaptureHealthMonitor,
    snapshot: Option<HealthSnapshot>,
    recovery: RecoveryTracker,
    incidents: BTreeMap<String, RecoverableIncident>,
    restart_pending: bool,
    restart_requested: bool,
    restart_at: Option<f64>,
    defer_restart_until: Option<f64>,
    deferred_restart: Option<(EngineConfig, bool, bool)>,
    /// A restart asked for after the running one already configured its
    /// process; it runs once that apply settles.
    queued_restart: Option<(EngineConfig, bool, bool)>,
    /// When the engine was last restarted on its own (crash or hang).
    auto_restarts: Vec<f64>,
    /// The app is quitting: nothing starts the engine again.
    final_stop: bool,
    applying: bool,
    apply_deadline: Option<f64>,
    last_event_at: f64,
    last_status: Option<(String, StatusStyle)>,
    last_flags: u32,
    last_backend_generation: u64,
    active_codec: Option<String>,
    exit_code: Option<Option<i32>>,
    save_busy: bool,
    recording: bool,
    recording_preparing: bool,
    disconnected_reported: bool,
    silence_reported: bool,
    startup_error: Option<(String, String)>,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Supervisor {
    /// A stopped supervisor.
    pub fn new() -> Self {
        Supervisor {
            generation: 0,
            process_generation: None,
            phase: Phase::Stopped,
            config: None,
            pending_launch: None,
            tracker: CaptureConfigTracker::new(None),
            health: CaptureHealthMonitor::new(),
            snapshot: None,
            recovery: RecoveryTracker::new(),
            incidents: BTreeMap::new(),
            restart_pending: false,
            restart_requested: false,
            restart_at: None,
            defer_restart_until: None,
            deferred_restart: None,
            queued_restart: None,
            auto_restarts: Vec::new(),
            final_stop: false,
            applying: false,
            apply_deadline: None,
            last_event_at: 0.0,
            last_status: None,
            last_flags: 0,
            last_backend_generation: 0,
            active_codec: None,
            exit_code: None,
            save_busy: false,
            recording: false,
            recording_preparing: false,
            disconnected_reported: false,
            silence_reported: false,
            startup_error: None,
        }
    }

    /// The save service reports whether a save is in flight.
    pub fn set_save_busy(&mut self, busy: bool) {
        self.save_busy = busy;
    }

    /// Whether a clip save is in flight.
    pub fn save_busy(&self) -> bool {
        self.save_busy
    }

    /// The recording service reports whether a recording is not idle.
    pub fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
    }

    /// The recording service is preparing a quality profile.
    pub fn set_recording_preparing(&mut self, preparing: bool) {
        self.recording_preparing = preparing;
    }

    /// The IPC connection is up.
    pub fn is_connected(&self) -> bool {
        self.phase == Phase::Connected
    }

    /// The active (applied) configuration.
    pub fn active_config(&self) -> Option<&CaptureConfig> {
        self.tracker.active()
    }

    /// The engine's encoder name.
    pub fn active_codec(&self) -> Option<&str> {
        self.active_codec.as_deref()
    }

    /// The last status shown.
    pub fn status_text(&self) -> Option<(&str, StatusStyle)> {
        self.last_status.as_ref().map(|(t, s)| (t.as_str(), *s))
    }

    /// The latest health snapshot.
    pub fn snapshot(&self) -> Option<&HealthSnapshot> {
        self.snapshot.as_ref()
    }

    /// Whether a save may be submitted now (connected and health allows).
    pub fn save_allowed(&self) -> bool {
        self.is_connected()
            && self
                .snapshot
                .as_ref()
                .is_some_and(HealthSnapshot::save_allowed)
    }

    /// Whether settings are being applied (restart in progress).
    pub fn applying(&self) -> bool {
        self.applying
    }

    fn pending_text(&self) -> &'static str {
        if self.restart_requested {
            "RESTARTING ENGINE"
        } else {
            "APPLYING SETTINGS"
        }
    }

    fn status(&mut self, out: &mut Vec<Effect>, text: &str, style: StatusStyle) {
        let next = (text.to_owned(), style);
        if self.last_status.as_ref() != Some(&next) {
            self.last_status = Some(next.clone());
            out.push(Effect::Ui(Event::Status {
                text: next.0,
                style: next.1,
            }));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn error(
        &mut self,
        out: &mut Vec<Effect>,
        title: &str,
        detail: String,
        level: Level,
        actions: Vec<(&str, &str)>,
        key: Option<&str>,
        dismiss_on_action: bool,
        recovery: Option<(RecoveryKind, &str)>,
    ) {
        if let (Some(key), Some((kind, code))) = (key, recovery) {
            self.recovery.reset(Some(key));
            self.incidents.insert(
                key.to_owned(),
                RecoverableIncident {
                    kind,
                    process_generation: self.process_generation.unwrap_or(0),
                    public_error_code: code.to_owned(),
                    title: title.to_owned(),
                },
            );
        } else if let Some(key) = key {
            self.incidents.remove(key);
            self.recovery.reset(Some(key));
        }
        out.push(Effect::Ui(Event::ErrorBar {
            title: title.to_owned(),
            detail,
            level,
            actions: actions
                .into_iter()
                .map(|(label, id)| Action {
                    label: label.into(),
                    id: id.into(),
                })
                .collect(),
            key: key.map(str::to_owned),
            dismiss_on_action,
        }));
    }

    fn begin_start(&mut self, config: EngineConfig, out: &mut Vec<Effect>, now: f64) {
        if self.final_stop {
            return;
        }
        self.generation += 1;
        self.process_generation = Some(self.generation);
        self.exit_code = None;
        self.startup_error = None;
        self.disconnected_reported = false;
        self.silence_reported = false;
        self.tracker.request(config.capture.clone());
        let _ = self.tracker.begin_apply();
        self.config = Some(config);
        self.phase = Phase::Spawning;
        self.last_event_at = now;
        self.health.reset(true);
        self.snapshot = None;
        if !self.applying {
            self.status(out, "CONNECTING", StatusStyle::Idle);
        }
        out.push(Effect::Spawn);
    }

    /// Enters `Stopping` and asks the driver to stop the process. Saves and
    /// recordings asked for from here on would reach an engine that is
    /// going away, so the pages learn at once that it is gone (they used to
    /// admit a save, show its success card, then fail it).
    fn begin_stop(&mut self, out: &mut Vec<Effect>, graceful: bool, grace_s: f64) {
        let was_connected = self.is_connected();
        self.phase = Phase::Stopping;
        self.generation += 1;
        out.push(Effect::Kill { graceful, grace_s });
        if was_connected {
            out.push(Effect::Ui(Event::EngineConnected(false)));
        }
    }

    fn finish_apply(&mut self, out: &mut Vec<Effect>) {
        self.applying = false;
        self.apply_deadline = None;
        self.restart_pending = false;
        self.restart_requested = false;
        let _ = out;
    }

    fn engine_gone(&mut self, out: &mut Vec<Effect>, detail: String, now: f64) {
        self.phase = Phase::Stopped;
        self.pending_launch = None;
        self.tracker
            .deactivate("capture engine exited unexpectedly");
        self.finish_apply(out);
        out.push(Effect::Ui(Event::ActiveConfig(None)));
        self.recovery.reset(None);
        self.snapshot = Some(self.health.observe(false, 0, 0, 0, now));
        self.status(out, "ENGINE STOPPED", StatusStyle::Warning);
        out.push(Effect::Ui(Event::EngineConnected(false)));
        out.push(Effect::Ui(Event::Health(
            self.health.observe(false, 0, 0, 0, now),
        )));
        self.error(
            out,
            "ENGINE STOPPED",
            detail,
            Level::Error,
            vec![("RESTART ENGINE", "restart")],
            Some("engine-process-exit"),
            false,
            Some((RecoveryKind::EngineLineage, "Error 006")),
        );
        self.schedule_auto_restart(out, now);
    }

    /// The reference left a crashed engine stopped until the user pressed
    /// RESTART ENGINE, so capture silently ended mid-game. It now comes back
    /// on its own a few times; the error bar still says what happened.
    fn schedule_auto_restart(&mut self, out: &mut Vec<Effect>, now: f64) {
        if !self.auto_restart_allowed(now) {
            return;
        }
        self.auto_restarts.push(now);
        self.restart_at = Some(now + AUTO_RESTART_DELAY_S);
        out.push(Effect::StartAfter(AUTO_RESTART_DELAY_S));
    }

    /// Whether an automatic restart is still in the budget.
    fn auto_restart_allowed(&mut self, now: f64) -> bool {
        self.auto_restarts
            .retain(|t| now - t < AUTO_RESTART_WINDOW_S);
        !self.final_stop && self.config.is_some() && self.auto_restarts.len() < AUTO_RESTART_LIMIT
    }

    /// When the supervisor next has something to do on its own (a timer);
    /// `None` when only inputs can change anything.
    pub fn next_deadline(&self) -> Option<f64> {
        let mut at: Vec<f64> = Vec::new();
        // A restart waits for the old process to be gone: due while it is
        // still stopping, it must not make the loop spin.
        if self.phase == Phase::Stopped {
            at.extend(self.restart_at);
        }
        at.extend(self.defer_restart_until);
        at.extend(self.apply_deadline);
        if let Phase::Connecting { since } = self.phase {
            at.push(since + HANDSHAKE_S);
        }
        if self.is_connected() {
            at.push(
                self.last_event_at
                    + if self.silence_reported {
                        HANG_RESTART_S
                    } else {
                        SILENCE_S
                    },
            );
        }
        at.into_iter().reduce(f64::min)
    }

    /// Feeds one input; returns the effects.
    pub fn handle(&mut self, input: Input, now: f64) -> Vec<Effect> {
        let mut out = Vec::new();
        match input {
            Input::Start(config) => {
                if matches!(self.phase, Phase::Stopped) {
                    self.begin_start(config, &mut out, now);
                }
            }
            Input::Stop => {
                // Quitting: nothing may start the engine again (an armed
                // restart used to spawn a new engine after the stop).
                self.final_stop = true;
                self.restart_at = None;
                self.defer_restart_until = None;
                self.deferred_restart = None;
                self.queued_restart = None;
                self.finish_apply(&mut out);
                if !matches!(self.phase, Phase::Stopped) {
                    // Quitting mid-save: let the engine finish the clip.
                    let grace_s = if self.save_busy {
                        SAVE_EXIT_GRACE_S
                    } else {
                        KILL_GRACE_S
                    };
                    self.begin_stop(&mut out, true, grace_s);
                }
            }
            Input::Restart {
                config,
                allow_recording_prepare,
                explicit,
            } => {
                if self.final_stop {
                    return out;
                }
                if self.recording && !(allow_recording_prepare && self.recording_preparing) {
                    self.error(
                        &mut out,
                        "STOP RECORDING FIRST",
                        "Finish the manual recording before restarting capture or applying encoder settings.".into(),
                        Level::Warning,
                        vec![],
                        None,
                        true,
                        None,
                    );
                    return out;
                }
                if explicit && !self.recording {
                    self.restart_requested = true;
                }
                self.applying = true;
                if self.save_busy {
                    self.status(&mut out, self.pending_text(), StatusStyle::Idle);
                    // The newest settings win; an explicit ask stays explicit.
                    let explicit = explicit || self.deferred_restart.as_ref().is_some_and(|d| d.2);
                    if self.deferred_restart.is_none() {
                        self.defer_restart_until = Some(now + RESTART_DEFER_S);
                    }
                    self.deferred_restart = Some((config, allow_recording_prepare, explicit));
                    return out;
                }
                if self.restart_pending {
                    // A restart is under way. Until the new process has been
                    // configured, it simply starts with the newest settings;
                    // after that, they need one more restart once this one
                    // settles (both used to be dropped).
                    if self.pending_launch.is_some() {
                        let explicit =
                            explicit || self.queued_restart.as_ref().is_some_and(|q| q.2);
                        self.queued_restart = Some((config, allow_recording_prepare, explicit));
                    } else {
                        // The tracker must hold what Configure will send, or
                        // the old settings are reported as active.
                        self.tracker.request(config.capture.clone());
                        let _ = self.tracker.begin_apply();
                        self.config = Some(config);
                    }
                    return out;
                }
                self.restart_pending = true;
                self.apply_deadline = Some(now + APPLY_TIMEOUT_S);
                self.status(&mut out, self.pending_text(), StatusStyle::Idle);
                self.config = Some(config);
                match self.phase {
                    Phase::Stopped => {
                        self.restart_at = Some(now + RESTART_GAP_S);
                        out.push(Effect::StartAfter(RESTART_GAP_S));
                    }
                    _ => {
                        self.begin_stop(&mut out, true, KILL_GRACE_S);
                        self.restart_at = Some(now + RESTART_GAP_S);
                        out.push(Effect::StartAfter(RESTART_GAP_S));
                    }
                }
            }
            Input::Pause => {
                if self.is_connected() {
                    out.push(Effect::Send(Command::Pause));
                    self.status(&mut out, FOCUS_PAUSED, StatusStyle::Warning);
                }
            }
            Input::Resume => {
                if self.is_connected() {
                    out.push(Effect::Send(Command::Resume));
                    self.status(&mut out, "STARTING CAPTURE", StatusStyle::Idle);
                }
            }
            Input::Spawned { pid } => {
                let _ = pid;
                if self.phase == Phase::Spawning {
                    self.phase = Phase::Connecting { since: now };
                    self.last_event_at = now;
                    out.push(Effect::Connect);
                } else if matches!(self.phase, Phase::Stopping | Phase::Stopped) {
                    // A stop arrived while the process was still being
                    // spawned: the kill found no child then, so the process
                    // that just appeared would run unseen.
                    self.phase = Phase::Stopping;
                    out.push(Effect::Kill {
                        graceful: false,
                        grace_s: 0.0,
                    });
                }
            }
            Input::SpawnFailed { detail, missing } => {
                self.phase = Phase::Stopped;
                self.tracker.fail(&detail);
                self.finish_apply(&mut out);
                if missing {
                    self.status(&mut out, "NO ENGINE", StatusStyle::Warning);
                    self.error(
                        &mut out,
                        "ENGINE NOT FOUND",
                        "fthrust-engine was not found next to the application (or at $FTHRUST_ENGINE). Reinstall FTHRust.".into(),
                        Level::Error,
                        vec![("OPEN SETTINGS", "open-settings")],
                        Some("engine-launch"),
                        true,
                        Some((RecoveryKind::EngineLineage, "Error 003")),
                    );
                } else {
                    self.status(&mut out, "ENGINE STOPPED", StatusStyle::Warning);
                    self.error(
                        &mut out,
                        "ENGINE COULD NOT START",
                        detail,
                        Level::Error,
                        vec![("RESTART ENGINE", "restart")],
                        Some("engine-launch"),
                        false,
                        Some((RecoveryKind::EngineLineage, "Error 005")),
                    );
                }
            }
            Input::Hello { .. } => {
                if matches!(self.phase, Phase::Connecting { .. } | Phase::Spawning) {
                    self.phase = Phase::Connected;
                    self.last_event_at = now;
                    self.disconnected_reported = false;
                    if let Some(config) = self.config.clone() {
                        self.pending_launch = Some((config.clone(), self.generation));
                        out.push(Effect::Send(Command::Configure(config)));
                    }
                    self.status(&mut out, "STARTING CAPTURE", StatusStyle::Idle);
                    out.push(Effect::Ui(Event::EngineConnected(true)));
                }
            }
            Input::StartupWarning { code, detail } => {
                let action = if code.starts_with("CROP_") {
                    ("EDIT GAME CROP", "edit-game-crop")
                } else {
                    ("OPEN AUDIO SETTINGS", "open-audio-settings")
                };
                self.error(
                    &mut out,
                    &startup_warning_title(&code),
                    detail,
                    Level::Warning,
                    vec![action],
                    None,
                    true,
                    None,
                );
            }
            Input::StartupError { code, detail } => {
                self.startup_error = Some((code.clone(), detail.clone()));
                let explicit = code != "ENGINE_START_FAILED";
                self.tracker.fail(&detail);
                self.finish_apply(&mut out);
                let (title, public) = if explicit {
                    ("CAPTURE FAILED", "Error 001")
                } else {
                    ("ENGINE NOT RESPONDING", "Error 004")
                };
                self.status(&mut out, "CAPTURE FAILED", StatusStyle::Warning);
                self.error(
                    &mut out,
                    title,
                    format!("{code}: {detail}"),
                    Level::Error,
                    vec![("RESTART ENGINE", "restart")],
                    Some("engine-launch"),
                    false,
                    Some((RecoveryKind::EngineLineage, public)),
                );
                if !matches!(self.phase, Phase::Stopped) {
                    self.begin_stop(&mut out, true, KILL_GRACE_S);
                }
            }
            Input::Configured { warnings } => {
                self.last_event_at = now;
                if !warnings.is_empty() {
                    self.error(
                        &mut out,
                        "CAPTURE SETTINGS ADJUSTED",
                        warnings.join(" "),
                        Level::Warning,
                        vec![("OPEN SETTINGS", "open-settings")],
                        None,
                        true,
                        None,
                    );
                }
            }
            Input::Status(s) => {
                self.last_event_at = now;
                self.silence_reported = false;
                if !self.is_connected() {
                    return out;
                }
                if !s.active_codec.is_empty()
                    && self.active_codec.as_deref() != Some(s.active_codec.as_str())
                {
                    self.active_codec = Some(s.active_codec.clone());
                    out.push(Effect::Ui(Event::EncoderActive(s.active_codec.clone())));
                }
                self.last_flags = s.health_flags;
                self.last_backend_generation = u64::from(s.capture_generation);
                let snapshot = self.health.observe(
                    true,
                    s.frames_captured,
                    s.health_flags,
                    u64::from(s.capture_generation),
                    now,
                );
                let state = snapshot.state;
                self.snapshot = Some(snapshot.clone());
                // Save admission and the recording guard read the latest snapshot.
                out.push(Effect::Ui(Event::Health(snapshot.clone())));
                // Activate or fail the pending launch.
                if let Some((_, launch_gen)) = &self.pending_launch
                    && *launch_gen == self.generation
                {
                    let paused_ok = state == HealthState::Degraded
                        && s.health_flags & flags::PAUSED != 0
                        && s.health_flags & flags::ACTIVE != 0;
                    if matches!(state, HealthState::Healthy | HealthState::ContentSuspect)
                        || paused_ok
                    {
                        let _ = self.tracker.succeed();
                        self.pending_launch = None;
                        self.finish_apply(&mut out);
                        out.push(Effect::Ui(Event::ActiveConfig(
                            self.tracker.active().cloned(),
                        )));
                    } else if matches!(state, HealthState::Stalled | HealthState::Failed) {
                        self.tracker.fail(&snapshot.reason);
                        self.pending_launch = None;
                        self.finish_apply(&mut out);
                    }
                }
                if snapshot.changed && state == HealthState::Failed {
                    let detail = if s.capture_failure_detail.is_empty() {
                        if snapshot.reason.is_empty() {
                            "The capture backend failed.".to_owned()
                        } else {
                            snapshot.reason.clone()
                        }
                    } else {
                        s.capture_failure_detail.clone()
                    };
                    self.error(
                        &mut out,
                        "CAPTURE FAILED",
                        detail,
                        Level::Error,
                        vec![("RESTART ENGINE", "restart")],
                        Some("capture-backend"),
                        false,
                        Some((RecoveryKind::CaptureBackend, "Error 001")),
                    );
                }
                let text = if self.applying {
                    self.pending_text().to_owned()
                } else if focus_paused(state, s.health_flags) {
                    FOCUS_PAUSED.to_owned()
                } else {
                    text_for(state).to_owned()
                };
                let style = if !self.applying && focus_paused(state, s.health_flags) {
                    StatusStyle::Warning
                } else {
                    style_for(state, self.applying)
                };
                self.status(&mut out, &text, style);
                self.advance_incidents(&mut out, true, s.frames_captured, state, now);
            }
            Input::Heartbeat => {
                self.last_event_at = now;
                self.silence_reported = false;
            }
            Input::Disconnected => {
                if self.is_connected() && !self.disconnected_reported {
                    self.disconnected_reported = true;
                    self.snapshot = Some(self.health.observe(false, 0, 0, 0, now));
                    self.status(&mut out, "DISCONNECTED", StatusStyle::Warning);
                    out.push(Effect::Ui(Event::EngineConnected(false)));
                }
            }
            Input::ProcessExited { code } => {
                if matches!(self.phase, Phase::Stopping | Phase::Stopped) {
                    // Expected: a stop or restart in progress.
                    self.phase = Phase::Stopped;
                    self.exit_code = Some(code);
                    out.push(Effect::Ui(Event::EngineConnected(false)));
                    return out;
                }
                let previous = self.snapshot.as_ref().map(|s| s.state);
                self.exit_code = Some(code);
                let detail = engine_stopped_detail(code, previous);
                self.engine_gone(&mut out, detail, now);
            }
            Input::Send(cmd) => {
                if self.is_connected() {
                    out.push(Effect::Send(cmd));
                }
            }
            Input::Tick => {}
        }
        // Timers.
        if let Some(at) = self.restart_at
            && now >= at
            && matches!(self.phase, Phase::Stopped)
            && let Some(config) = self.config.clone()
        {
            self.restart_at = None;
            self.begin_start(config, &mut out, now);
        }
        if let Some(until) = self.defer_restart_until
            && now >= until
        {
            self.defer_restart_until = None;
            if self.save_busy {
                self.defer_restart_until = Some(now + RESTART_DEFER_S);
            } else if let Some((config, allow, explicit)) = self.deferred_restart.take() {
                let mut more = self.handle(
                    Input::Restart {
                        config,
                        allow_recording_prepare: allow,
                        explicit,
                    },
                    now,
                );
                out.append(&mut more);
            }
        }
        if !self.restart_pending
            && let Some((config, allow, explicit)) = self.queued_restart.take()
        {
            let mut more = self.handle(
                Input::Restart {
                    config,
                    allow_recording_prepare: allow,
                    explicit,
                },
                now,
            );
            out.append(&mut more);
        }
        if let Phase::Connecting { since } = self.phase
            && now - since >= HANDSHAKE_S
        {
            let (code, detail) = self.startup_error.clone().unwrap_or_else(|| {
                (
                    "ENGINE_START_FAILED".into(),
                    "the engine did not answer within 9 seconds".into(),
                )
            });
            self.phase = Phase::Stopping;
            self.generation += 1;
            let mut more = self.handle(Input::StartupError { code, detail }, now);
            out.append(&mut more);
            if !out.iter().any(|e| matches!(e, Effect::Kill { .. })) {
                out.push(Effect::Kill {
                    graceful: false,
                    grace_s: KILL_GRACE_S,
                });
            }
        }
        if let Some(deadline) = self.apply_deadline
            && now >= deadline
            && self.applying
        {
            self.tracker.fail("settings apply timed out");
            self.finish_apply(&mut out);
            self.status(&mut out, "SETTINGS APPLY TIMED OUT", StatusStyle::Warning);
            self.error(
                &mut out,
                "SETTINGS APPLY TIMED OUT",
                "The capture engine did not reach a healthy state within 30 seconds. Restart capture or revert the settings.".into(),
                Level::Error,
                vec![("RESTART ENGINE", "restart")],
                Some("capture-apply-timeout"),
                true,
                None,
            );
        }
        if self.is_connected() && now - self.last_event_at >= SILENCE_S && !self.silence_reported {
            self.silence_reported = true;
            let snapshot = self.health.observe(false, 0, 0, 0, now);
            self.snapshot = Some(snapshot.clone());
            // Save admission reads the health the UI was last sent: without
            // this, saves were still admitted to a silent engine.
            out.push(Effect::Ui(Event::Health(snapshot)));
            self.status(&mut out, "DISCONNECTED", StatusStyle::Warning);
            self.advance_incidents(&mut out, false, 0, HealthState::Stopped, now);
        }
        if self.is_connected()
            && self.silence_reported
            && now - self.last_event_at >= HANG_RESTART_S
        {
            tracing::warn!("engine silent for {HANG_RESTART_S} s; stopping it");
            let detail = if self.auto_restart_allowed(now) {
                "The capture engine stopped answering; it is being restarted."
            } else {
                "The capture engine stopped answering and was stopped. Clips cannot be saved until it is restarted."
            };
            self.engine_gone(&mut out, detail.into(), now);
            // engine_gone marked it stopped; the process is still there.
            self.phase = Phase::Stopping;
            self.generation += 1;
            out.push(Effect::Kill {
                graceful: false,
                grace_s: KILL_GRACE_S,
            });
        }
        out
    }

    fn advance_incidents(
        &mut self,
        out: &mut Vec<Effect>,
        connected: bool,
        frames: u64,
        state: HealthState,
        now: f64,
    ) {
        let recovered = advance_recoverable_incidents(
            &mut self.recovery,
            &mut self.incidents,
            self.process_generation,
            connected,
            frames,
            state,
            self.last_backend_generation,
            false,
            now,
        );
        for (key, _incident) in recovered {
            self.incidents.remove(&key);
            out.push(Effect::Ui(Event::IncidentCleared(key)));
        }
    }
}

fn startup_warning_title(code: &str) -> String {
    match code {
        "DESKTOP_AUDIO_UNAVAILABLE" => "DESKTOP AUDIO UNAVAILABLE".into(),
        "MICROPHONE_UNAVAILABLE" => "MIC NOT FOUND".into(),
        c if c.starts_with("CROP_") => "GAME CROP ADJUSTED".into(),
        other => other.replace('_', " "),
    }
}

/// `_engine_stopped_detail`.
pub fn engine_stopped_detail(code: Option<i32>, previous: Option<HealthState>) -> String {
    if code == Some(0) && previous == Some(HealthState::Failed) {
        return "The capture engine stopped after the capture failure above. Clips cannot be saved until it is restarted.".into();
    }
    let code = code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".into());
    format!(
        "The capture engine exited unexpectedly (code {code}). Clips cannot be saved until it is restarted."
    )
}

/// Builds the engine configuration from the settings.
pub fn engine_config(state: &AppState) -> Result<EngineConfig, String> {
    let capture = CaptureConfig::from_settings(&state.settings).map_err(|e| e.to_string())?;
    let max_buffer_mb = linux_ring_budget_mb(capture.bitrate_kbps, capture.buffer_seconds, None)
        .min(u64::from(u32::MAX)) as u32;
    let backend_override = std::env::var("FTHR_CAPTURE_BACKEND")
        .ok()
        .and_then(|v| BackendKind::parse_override(&v));
    Ok(EngineConfig {
        capture,
        max_buffer_mb,
        backend_override,
    })
}

/// Finds `fthrust-engine`: `$FTHRUST_ENGINE`, next to the executable, then
/// the cargo target directory in development.
pub fn discover_engine(exe_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FTHRUST_ENGINE") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = exe_dir {
        candidates.push(dir.join("fthrust-engine"));
    }
    if let Some(exe) = &exe {
        let mut dir = exe.parent();
        // target/debug/deps/<test> → target/debug; target/debug/fthrust → target/debug.
        for _ in 0..3 {
            if let Some(d) = dir {
                candidates.push(d.join("fthrust-engine"));
                dir = d.parent();
            }
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Runs the supervisor on the services runtime: spawns the process,
/// connects, forwards IPC events and executes effects.
pub struct EngineDriver {
    inputs: Sender<Input>,
    pid: Arc<Mutex<Option<u32>>>,
    supervisor: Arc<Mutex<Supervisor>>,
}

struct DriverCtx {
    handle: tokio::runtime::Handle,
    outbox: Outbox,
    engine: PathBuf,
    socket: PathBuf,
    env: Vec<(String, String)>,
    log: PathBuf,
    child: Arc<Mutex<Option<tokio::process::Child>>>,
    pid: Arc<Mutex<Option<u32>>>,
    /// The connected client's command queue (ordered; `None` when not connected).
    commands: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<Command>>>>,
    inputs: Sender<Input>,
    start_epoch: Instant,
}

impl EngineDriver {
    /// Starts the driver thread.
    pub fn start(
        handle: tokio::runtime::Handle,
        outbox: Outbox,
        engine: PathBuf,
        socket: PathBuf,
        env: Vec<(String, String)>,
        log: PathBuf,
    ) -> EngineDriver {
        let (tx, rx) = channel::<Input>();
        let supervisor = Arc::new(Mutex::new(Supervisor::new()));
        let child = Arc::new(Mutex::new(None));
        let pid = Arc::new(Mutex::new(None));
        let ctx = Arc::new(DriverCtx {
            handle: handle.clone(),
            outbox,
            engine,
            socket,
            env,
            log,
            child: child.clone(),
            pid: pid.clone(),
            commands: Arc::new(Mutex::new(None)),
            inputs: tx.clone(),
            start_epoch: Instant::now(),
        });
        let sv = supervisor.clone();
        std::thread::Builder::new()
            .name("fthrust-engine-supervisor".into())
            .spawn(move || {
                loop {
                    // Sleep until the next timer (at most a second), not a
                    // fixed 100 ms: an idle supervisor used to wake 10 times
                    // a second for as long as the app ran.
                    let now = ctx.start_epoch.elapsed().as_secs_f64();
                    let wait = sv
                        .lock()
                        .ok()
                        .and_then(|s| s.next_deadline())
                        .map_or(1.0, |at| (at - now).clamp(0.01, 1.0));
                    let input = match rx.recv_timeout(Duration::from_secs_f64(wait)) {
                        Ok(i) => i,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Input::Tick,
                        Err(_) => break,
                    };
                    let now = ctx.start_epoch.elapsed().as_secs_f64();
                    let effects = sv
                        .lock()
                        .map(|mut s| s.handle(input, now))
                        .unwrap_or_default();
                    for e in effects {
                        run_effect(&ctx, e);
                    }
                }
            })
            .expect("spawn engine supervisor");
        EngineDriver {
            inputs: tx,
            pid,
            supervisor,
        }
    }

    /// Queues an input.
    pub fn request(&self, input: Input) {
        let _ = self.inputs.send(input);
    }

    /// The supervisor (save admission, flags).
    pub fn supervisor(&self) -> &Arc<Mutex<Supervisor>> {
        &self.supervisor
    }

    /// SIGKILLs the process (tests).
    pub fn kill_process(&self) {
        if let Some(pid) = *self.pid.lock().unwrap_or_else(|e| e.into_inner()) {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }

    /// Stops the engine and waits until its process is gone. The stop
    /// itself (a graceful `Shutdown`, SIGTERM, then SIGKILL) runs in the
    /// supervisor's kill task; waiting here keeps the runtime alive until
    /// it is done, so a save in flight is finished rather than cut off by
    /// the runtime shutting down (which dropped the child and SIGKILLed it).
    pub fn shutdown(self) {
        let save_busy = self
            .supervisor
            .lock()
            .map(|s| s.save_busy())
            .unwrap_or(false);
        let _ = self.inputs.send(Input::Stop);
        // The kill task's own bound: the graceful wait it was given, then
        // 1 s after SIGTERM and 0.5 s after SIGKILL, plus some slack.
        let grace = if save_busy {
            SAVE_EXIT_GRACE_S
        } else {
            KILL_GRACE_S
        };
        let deadline = Instant::now() + Duration::from_secs_f64(grace + 3.0);
        // The supervisor picks the Stop up within one loop turn; give it
        // that long before trusting "no process" to mean "already gone".
        std::thread::sleep(Duration::from_millis(150));
        while self.process_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if self.process_running() {
            tracing::warn!("engine still running after the shutdown wait; killing it");
            self.kill_process();
        }
    }

    /// Whether an engine process is still running.
    pub fn process_running(&self) -> bool {
        self.pid.lock().map(|p| p.is_some()).unwrap_or(false)
    }
}

fn run_effect(ctx: &Arc<DriverCtx>, effect: Effect) {
    match effect {
        // Health arrives about once a second for as long as the app runs and
        // changes nothing on screen by itself.
        Effect::Ui(ev @ Event::Health(_)) => ctx.outbox.send_quiet(ev),
        Effect::Ui(ev) => ctx.outbox.send(ev),
        Effect::Spawn => spawn_engine(ctx),
        Effect::Connect => connect_engine(ctx),
        Effect::Send(cmd) => {
            tracing::info!(command = ?std::mem::discriminant(&cmd), "engine command");
            if !queue_command(ctx, cmd) {
                tracing::warn!("engine command dropped: not connected");
            }
        }
        Effect::Kill { graceful, grace_s } => kill_engine(ctx, graceful, grace_s),
        Effect::StartAfter(_) => {}
    }
}

fn spawn_engine(ctx: &Arc<DriverCtx>) {
    let c = ctx.clone();
    ctx.handle.spawn(async move {
        retire_stale_engine(&c.socket).await;
        if let Some(parent) = c.socket.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut cmd = tokio::process::Command::new(&c.engine);
        cmd.arg("--socket")
            .arg(&c.socket)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &c.env {
            cmd.env(k, v);
        }
        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id().unwrap_or(0);
                if let Some(stderr) = child.stderr.take() {
                    let inputs = c.inputs.clone();
                    let log = c.log.clone();
                    c.handle.spawn(async move {
                        let mut writer = fthrust_core::diagnostics::logging::RotatingWriter::open(
                            &log,
                            2 * 1024 * 1024,
                            1,
                            false,
                        )
                        .ok();
                        pump_stderr(stderr, |line| {
                            if let Some(w) = writer.as_mut() {
                                let _ = w.write_line(line);
                            }
                            if let Some(input) = startup_error_line(line) {
                                let _ = inputs.send(input);
                            }
                        })
                        .await;
                    });
                }
                if let Ok(mut p) = c.pid.lock() {
                    *p = Some(pid);
                }
                if let Ok(mut slot) = c.child.lock() {
                    *slot = Some(child);
                }
                tracing::info!(pid, "engine spawned");
                let _ = c.inputs.send(Input::Spawned { pid });
                // Watch for exit.
                let watcher = c.clone();
                c.handle.spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        let status = {
                            let Ok(mut slot) = watcher.child.lock() else {
                                break;
                            };
                            match slot.as_mut() {
                                Some(child) => match child.try_wait() {
                                    Ok(Some(status)) => Some(Some(status)),
                                    Ok(None) => None,
                                    Err(_) => Some(None),
                                },
                                None => break,
                            }
                        };
                        if let Some(status) = status {
                            if let Ok(mut slot) = watcher.child.lock() {
                                *slot = None;
                            }
                            if let Ok(mut p) = watcher.pid.lock() {
                                *p = None;
                            }
                            if let Ok(mut slot) = watcher.commands.lock() {
                                *slot = None;
                            }
                            let code = status.and_then(|s| s.code());
                            let _ = watcher.inputs.send(Input::ProcessExited { code });
                            break;
                        }
                    }
                });
            }
            Err(e) => {
                let missing = e.kind() == std::io::ErrorKind::NotFound;
                let _ = c.inputs.send(Input::SpawnFailed {
                    detail: format!("{}: {e}", c.engine.display()),
                    missing,
                });
            }
        }
    });
}

fn connect_engine(ctx: &Arc<DriverCtx>) {
    let c = ctx.clone();
    ctx.handle.spawn(async move {
        for _ in 0..CONNECT_ATTEMPTS {
            tokio::time::sleep(CONNECT_INTERVAL).await;
            if c.child.lock().map(|s| s.is_none()).unwrap_or(true) {
                return;
            }
            tracing::info!(socket = %c.socket.display(), "connecting to the engine");
            match EngineClient::connect(&c.socket).await {
                Ok(mut client) => {
                    let hello = client.hello().clone();
                    tracing::info!(pid = hello.pid, version = %hello.version, "engine connected");
                    // The command queue exists before anyone learns we are
                    // connected, so the Configure that follows Hello is never
                    // dropped; one task owns the client and keeps order.
                    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<Command>();
                    if let Ok(mut slot) = c.commands.lock() {
                        *slot = Some(cmd_tx);
                    }
                    let _ = c.inputs.send(Input::Hello {
                        pid: hello.pid,
                        version: hello.version,
                    });
                    let forward_inputs = c.inputs.clone();
                    let outbox = c.outbox.clone();
                    let commands = c.commands.clone();
                    c.handle.spawn(async move {
                        loop {
                            tokio::select! {
                                ev = client.events().recv() => match ev {
                                    Some(ev) => {
                                        let input = match ev {
                                            IpcEvent::StartupWarning { code, detail } => {
                                                Input::StartupWarning { code, detail }
                                            }
                                            IpcEvent::StartupError { code, detail } => {
                                                Input::StartupError { code, detail }
                                            }
                                            IpcEvent::Configured { warnings } => Input::Configured { warnings },
                                            IpcEvent::Status(s) => Input::Status(*s),
                                            IpcEvent::Heartbeat => Input::Heartbeat,
                                            // Hundreds per save, and nothing
                                            // on screen follows them.
                                            p @ IpcEvent::SaveProgress { .. } => {
                                                outbox.send_quiet(Event::Engine(p));
                                                continue;
                                            }
                                            other => {
                                                outbox.send(Event::Engine(other));
                                                continue;
                                            }
                                        };
                                        let _ = forward_inputs.send(input);
                                    }
                                    None => break,
                                },
                                cmd = cmd_rx.recv() => match cmd {
                                    Some(cmd) => {
                                        if let Err(e) = client.send(cmd).await {
                                            tracing::warn!(error = %e, "engine command failed");
                                            break;
                                        }
                                    }
                                    None => break,
                                },
                            }
                        }
                        if let Ok(mut slot) = commands.lock() {
                            *slot = None;
                        }
                        tracing::warn!("engine event stream ended");
                        let _ = forward_inputs.send(Input::Disconnected);
                    });
                    return;
                }
                Err(_) => continue,
            }
        }
    });
}

/// How long an engine left over from an earlier session gets to let go of
/// the socket: the engine's own wait for a save in flight, plus a margin.
pub const STALE_ENGINE_EXIT: Duration = Duration::from_secs(65);

/// An engine left over from an earlier session (the UI died mid-save, say)
/// may still own the socket. The UI used to unlink the socket and start a
/// second engine beside it, both capturing. Now it asks the old one to
/// stop if it still serves clients, then waits until the socket refuses
/// connections or the process is gone (an engine finishing a save no
/// longer accepts clients, so it gets the time its save needs), and only
/// then SIGTERMs and SIGKILLs it. The owner's pid comes from the socket's
/// peer credentials, which a connection still waiting in the backlog has
/// too. A socket file nobody listens on is left for the new engine, which
/// replaces it.
pub async fn retire_stale_engine(socket: &Path) {
    retire_stale_engine_within(socket, STALE_ENGINE_EXIT).await;
}

/// [`retire_stale_engine`] with the wait bound given (tests).
pub async fn retire_stale_engine_within(socket: &Path, bound: Duration) {
    let connect = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::UnixStream::connect(socket),
    );
    let Ok(Ok(probe)) = connect.await else {
        return;
    };
    let pid = probe
        .peer_cred()
        .ok()
        .and_then(|c| c.pid())
        .filter(|p| *p > 0 && *p as u32 != std::process::id());
    drop(probe);
    tracing::warn!(
        ?pid,
        "an engine from an earlier session still owns the socket; stopping it"
    );
    let hello = tokio::time::timeout(Duration::from_secs(2), EngineClient::connect(socket));
    if let Ok(Ok(client)) = hello.await {
        let _ = client.send(Command::Shutdown).await;
    }
    let alive = |pid: Option<i32>| {
        pid.is_none_or(|p| nix::sys::signal::kill(nix::unistd::Pid::from_raw(p), None).is_ok())
    };
    let let_go = tokio::time::timeout(bound, async {
        loop {
            if !alive(pid) || tokio::net::UnixStream::connect(socket).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .is_ok();
    let Some(pid) = pid.filter(|_| !let_go) else {
        return;
    };
    let target = nix::unistd::Pid::from_raw(pid);
    let _ = nix::sys::signal::kill(target, nix::sys::signal::Signal::SIGTERM);
    let gone = tokio::time::timeout(Duration::from_secs(2), async {
        while alive(Some(pid)) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .is_ok();
    if !gone {
        let _ = nix::sys::signal::kill(target, nix::sys::signal::Signal::SIGKILL);
    }
}

/// Reads the engine's stderr to the end, a line at a time. Bytes that are
/// not UTF-8 are replaced rather than ending the read: a reader that stopped
/// closed the pipe, and the engine's next `eprintln!` then died on EPIPE.
pub async fn pump_stderr(stderr: impl tokio::io::AsyncRead + Unpin, mut on_line: impl FnMut(&str)) {
    use tokio::io::AsyncBufReadExt;
    let mut reader = tokio::io::BufReader::new(stderr);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf);
                on_line(line.trim_end_matches(['\n', '\r']));
            }
        }
    }
}

/// An `FTHR_STARTUP_ERROR: CODE: detail` line as a supervisor input.
pub fn startup_error_line(line: &str) -> Option<Input> {
    let rest = line.strip_prefix("FTHR_STARTUP_ERROR: ")?;
    let (code, detail) = rest.split_once(": ").unwrap_or((rest, ""));
    Some(Input::StartupError {
        code: code.trim().to_owned(),
        detail: detail.trim().chars().take(1000).collect(),
    })
}

/// Queues `cmd` for the connected client; `false` when not connected.
fn queue_command(ctx: &Arc<DriverCtx>, cmd: Command) -> bool {
    ctx.commands
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|tx| tx.send(cmd).is_ok()))
        .unwrap_or(false)
}

fn kill_engine(ctx: &Arc<DriverCtx>, graceful: bool, grace_s: f64) {
    let c = ctx.clone();
    ctx.handle.spawn(async move {
        if graceful {
            let _ = queue_command(&c, Command::Shutdown);
        }
        let taken = c.child.lock().ok().and_then(|mut s| s.take());
        let Some(mut child) = taken else {
            return;
        };
        let pid = child.id();
        let grace = Duration::from_secs_f64(grace_s.max(0.0));
        let mut done = graceful && tokio::time::timeout(grace, child.wait()).await.is_ok();
        if !done && let Some(pid) = pid {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
            done = tokio::time::timeout(Duration::from_secs(1), child.wait())
                .await
                .is_ok();
        }
        let mut code = None;
        if !done {
            let _ = child.start_kill();
            if let Ok(Ok(status)) =
                tokio::time::timeout(Duration::from_millis(500), child.wait()).await
            {
                code = status.code();
            }
        } else if let Ok(status) = child.try_wait()
            && let Some(status) = status
        {
            code = status.code();
        }
        if let Ok(mut p) = c.pid.lock() {
            *p = None;
        }
        if let Ok(mut slot) = c.commands.lock() {
            *slot = None;
        }
        // The exit watcher no longer owns the child: report the exit here so
        // the supervisor leaves `Stopping` and relaunches when asked to.
        let _ = c.inputs.send(Input::ProcessExited { code });
    });
}
