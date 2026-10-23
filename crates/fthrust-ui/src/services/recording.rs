//! Manual recording (`main.py` `_start_manual_recording` and friends):
//! the recording quality profile, the start/stop commands, the 10 s
//! acknowledgement timeout, finalisation by probing the file, and the
//! Record button's texts and statuses.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use fthrust_core::capture_config::{
    BitrateLevel, CaptureConfig, bitrate_preset_kbps, resolution_dims,
    validate_custom_bitrate_kbps, validate_fps,
};
use fthrust_core::clip_files::{recording_path, unique_path};
use fthrust_ipc::{Command, EngineConfig, Event as IpcEvent};
use time::OffsetDateTime;

use super::CardKind;
use crate::shell::error_bar::{Action, Level};
use crate::state::{AppState, StatusStyle, StatusText};

/// The start acknowledgement deadline.
pub const START_ACK_S: f64 = 10.0;
/// Free space required before a recording starts (512 MiB).
pub const MIN_FREE_BYTES: u64 = 512 * 1024 * 1024;
/// How long `RECORDING SAVED` shows.
pub const SAVED_STATUS_S: f64 = 2.5;
/// How long `RECORDING FAILED` shows.
pub const FAILED_STATUS_S: f64 = 3.0;
/// A running recording stops below this much free space (256 MiB).
pub const LOW_SPACE_FLOOR: u64 = 256 * 1024 * 1024;
/// How often the free space is checked while recording.
pub const SPACE_CHECK_S: f64 = 1.0;

/// The recording state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordState {
    /// Nothing in progress.
    Idle,
    /// The engine is restarting with the recording profile.
    Preparing,
    /// `StartRecording` sent, waiting for the acknowledgement.
    Starting,
    /// Writing.
    Recording,
    /// `StopRecording` sent.
    Stopping,
    /// The file is being validated.
    Finalizing,
}

/// What the service wants done.
#[derive(Debug)]
pub enum RecAction {
    /// Send a command.
    Send(Command),
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
    /// Show a card.
    Card(CardKind),
    /// Restart the engine with the recording profile.
    RestartForRecording(EngineConfig),
    /// Restart the engine with the clips profile.
    RestoreClipProfile,
    /// Rescan the library.
    RefreshLibrary,
}

/// `<dir>/FTHR_recording_%Y-%m-%d_%H-%M-%S[_N].mp4`, creating `dir`;
/// paths over 255 characters are refused.
pub fn recording_target(dir: &Path, at: OffsetDateTime) -> Result<PathBuf, String> {
    if recording_path(dir, at).as_os_str().len() > 255 {
        return Err("Choose a shorter recording folder; the native engine accepts paths up to 255 characters.".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = unique_path(recording_path(dir, at));
    if path.as_os_str().len() > 255 {
        return Err("Choose a shorter recording folder; the native engine accepts paths up to 255 characters.".into());
    }
    Ok(path)
}

/// Free bytes on the filesystem holding `dir`.
pub fn free_space_bytes(dir: &Path) -> Result<u64, String> {
    let st = nix::sys::statvfs::statvfs(dir).map_err(|e| e.to_string())?;
    Ok(st.blocks_available() * st.fragment_size())
}

/// `_requested_capture_config('recording')`: the clips profile with the
/// `recording_*` quality keys.
pub fn recording_config(state: &AppState) -> Result<EngineConfig, String> {
    let mut config = super::engine::engine_config(state)?;
    let s = &state.settings;
    if let Some(fps) = s.get_i64("recording_framerate") {
        config.capture.fps = validate_fps(fps).map_err(|e| e.to_string())?;
    }
    let resolution = s
        .get_str("recording_resolution")
        .unwrap_or("source")
        .to_lowercase();
    let (w, h) = resolution_dims(&resolution).unwrap_or((0, 0));
    config.capture.width = w;
    config.capture.height = h;
    let level = BitrateLevel::parse(s.get_str("recording_bitrate_level").unwrap_or("high"));
    config.capture.bitrate_kbps = if level == BitrateLevel::Custom {
        validate_custom_bitrate_kbps(s.get_i64("recording_custom_bitrate_kbps").unwrap_or(25_000))
            .map_err(|e| e.to_string())?
    } else {
        bitrate_preset_kbps(&resolution, level).unwrap_or(25_000)
    };
    config.capture.bitrate_kbps = config.capture.bitrate_kbps.clamp(500, 200_000);
    Ok(config)
}

fn quality(c: &CaptureConfig) -> (u32, u32, u32, u32) {
    (c.fps, c.width, c.height, c.bitrate_kbps)
}

/// `hh:mm:ss`.
pub fn format_elapsed(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total % 3600) / 60,
        total % 60
    )
}

/// The service.
pub struct RecordingService {
    state: RecordState,
    path: Option<PathBuf>,
    pending_path: Option<PathBuf>,
    desired: Option<CaptureConfig>,
    requested_at: f64,
    started_at: f64,
    next_request_id: u64,
    request_id: u64,
    finalize: Option<Receiver<Result<(), String>>>,
    transient: Option<(StatusText, Option<f64>)>,
    profile_is_recording: bool,
    low_space_floor: u64,
    last_space_check: f64,
    pub(crate) label: String,
    pub(crate) active_style: bool,
}

impl Default for RecordingService {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingService {
    /// Idle.
    pub fn new() -> Self {
        RecordingService {
            state: RecordState::Idle,
            path: None,
            pending_path: None,
            desired: None,
            requested_at: 0.0,
            started_at: 0.0,
            next_request_id: 1,
            request_id: 0,
            finalize: None,
            transient: None,
            profile_is_recording: false,
            low_space_floor: LOW_SPACE_FLOOR,
            last_space_check: 0.0,
            label: "Record".into(),
            active_style: false,
        }
    }

    /// The state.
    pub fn state(&self) -> RecordState {
        self.state
    }

    /// The free-space floor a running recording stops at (tests raise it).
    pub fn set_low_space_floor(&mut self, bytes: u64) {
        self.low_space_floor = bytes;
    }

    /// The engine is gone: a recording in progress is over, its fragments
    /// kept when the file exists.
    pub fn on_engine_lost(&mut self, now: f64) -> Vec<RecAction> {
        let mut out = Vec::new();
        if self.state == RecordState::Idle {
            return out;
        }
        let keep = self.path.as_ref().is_some_and(|p| p.exists());
        self.fail(
            "The capture engine stopped during the recording.",
            keep,
            &mut out,
            now,
        );
        out
    }

    /// Not idle (blocks engine restarts).
    pub fn busy(&self) -> bool {
        self.state != RecordState::Idle
    }

    /// Whether the button is clickable (idle or recording).
    pub fn button_enabled(&self, connected: bool) -> bool {
        connected && matches!(self.state, RecordState::Idle | RecordState::Recording)
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

    /// The transient status, if any.
    pub fn transient_status(&mut self, now: f64) -> Option<StatusText> {
        if let Some((_, Some(until))) = &self.transient
            && now >= *until
        {
            self.transient = None;
        }
        self.transient.as_ref().map(|(t, _)| t.clone())
    }

    fn set_label(&mut self, label: &str, active: bool) {
        self.label = label.into();
        self.active_style = active;
    }

    /// The Record button was pressed (or a start/stop hotkey).
    #[allow(clippy::too_many_arguments)]
    pub fn toggle(
        &mut self,
        state: &mut AppState,
        connected: bool,
        save_busy: bool,
        active: Option<&CaptureConfig>,
        now: f64,
        wall: OffsetDateTime,
    ) -> Vec<RecAction> {
        match self.state {
            RecordState::Idle => self.start(state, connected, save_busy, active, now, wall),
            RecordState::Recording => self.stop(now),
            _ => Vec::new(),
        }
    }

    fn start(
        &mut self,
        state: &mut AppState,
        connected: bool,
        save_busy: bool,
        active: Option<&CaptureConfig>,
        now: f64,
        wall: OffsetDateTime,
    ) -> Vec<RecAction> {
        let mut out = Vec::new();
        if save_busy {
            out.push(RecAction::Error {
                title: "WAIT FOR CLIP SAVE".into(),
                detail: "Start the manual recording after the current replay clip finishes saving."
                    .into(),
                level: Level::Warning,
                actions: vec![],
            });
            return out;
        }
        if !connected {
            out.push(RecAction::Error {
                title: "RECORDING COULD NOT START".into(),
                detail: "The capture engine is not connected.".into(),
                level: Level::Error,
                actions: vec![Action {
                    label: "RESTART CAPTURE".into(),
                    id: "restart-capture".into(),
                }],
            });
            return out;
        }
        let dir = match state.settings.recording_directory() {
            Ok(d) => d,
            Err(e) => {
                out.push(RecAction::Error {
                    title: "RECORDING FOLDER UNAVAILABLE".into(),
                    detail: e.to_string(),
                    level: Level::Error,
                    actions: vec![],
                });
                return out;
            }
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            out.push(RecAction::Error {
                title: "RECORDING FOLDER UNAVAILABLE".into(),
                detail: e.to_string(),
                level: Level::Error,
                actions: vec![],
            });
            return out;
        }
        match free_space_bytes(&dir) {
            Ok(free) if free < MIN_FREE_BYTES => {
                out.push(RecAction::Error {
                    title: "NOT ENOUGH DISK SPACE".into(),
                    detail: "Manual recording requires at least 512 MB free before it starts."
                        .into(),
                    level: Level::Error,
                    actions: vec![],
                });
                return out;
            }
            Err(e) => {
                out.push(RecAction::Error {
                    title: "RECORDING FOLDER UNAVAILABLE".into(),
                    detail: e,
                    level: Level::Error,
                    actions: vec![],
                });
                return out;
            }
            Ok(_) => {}
        }
        let path = match recording_target(&dir, wall) {
            Ok(p) => p,
            Err(e) => {
                out.push(RecAction::Error {
                    title: "RECORDING PATH TOO LONG".into(),
                    detail: e,
                    level: Level::Error,
                    actions: vec![],
                });
                return out;
            }
        };
        let desired = match recording_config(state) {
            Ok(c) => c,
            Err(e) => {
                out.push(RecAction::Error {
                    title: "RECORDING COULD NOT START".into(),
                    detail: e,
                    level: Level::Error,
                    actions: vec![],
                });
                return out;
            }
        };
        self.pending_path = Some(path);
        self.desired = Some(desired.capture.clone());
        let needs_profile = active.is_none_or(|a| quality(a) != quality(&desired.capture));
        if needs_profile {
            self.profile_is_recording = true;
            self.state = RecordState::Preparing;
            self.set_label("Preparing…", true);
            self.show("PREPARING RECORDING QUALITY", StatusStyle::Idle, None);
            out.push(RecAction::RestartForRecording(desired));
            return out;
        }
        self.submit_start(now, &mut out);
        out
    }

    /// `_continue_prepared_manual_recording`: once the recording profile is
    /// active, submit the start.
    pub fn continue_prepared(
        &mut self,
        connected: bool,
        active: Option<&CaptureConfig>,
        now: f64,
    ) -> Vec<RecAction> {
        let mut out = Vec::new();
        if self.state != RecordState::Preparing || !connected {
            return out;
        }
        let ready = match (&self.desired, active) {
            (Some(d), Some(a)) => quality(d) == quality(a),
            _ => false,
        };
        if ready {
            self.submit_start(now, &mut out);
        }
        out
    }

    fn submit_start(&mut self, now: f64, out: &mut Vec<RecAction>) {
        let Some(path) = self.pending_path.take() else {
            self.fail(
                "The recording destination was lost before capture started.",
                false,
                out,
                now,
            );
            return;
        };
        self.request_id = self.next_request_id;
        self.next_request_id += 1;
        self.path = Some(path.clone());
        self.state = RecordState::Starting;
        self.requested_at = now;
        self.set_label("Starting…", true);
        self.show("STARTING RECORDING", StatusStyle::Idle, None);
        out.push(RecAction::Send(Command::StartRecording {
            request_id: self.request_id,
            path,
        }));
    }

    fn stop(&mut self, now: f64) -> Vec<RecAction> {
        let mut out = Vec::new();
        self.request_id = self.next_request_id;
        self.next_request_id += 1;
        self.state = RecordState::Stopping;
        self.requested_at = now;
        self.set_label("Finalizing…", true);
        self.show("FINALIZING RECORDING", StatusStyle::Idle, None);
        out.push(RecAction::Send(Command::StopRecording {
            request_id: self.request_id,
        }));
        out
    }

    fn fail(&mut self, detail: &str, keep_recording: bool, out: &mut Vec<RecAction>, now: f64) {
        let recording = self.path.take();
        self.pending_path = None;
        self.finalize = None;
        self.state = RecordState::Idle;
        if let Some(p) = &recording
            && !keep_recording
        {
            let _ = std::fs::remove_file(p);
        }
        self.set_label("Record", false);
        self.show(
            "RECORDING FAILED",
            StatusStyle::Warning,
            Some(now + FAILED_STATUS_S),
        );
        let mut detail = detail.to_owned();
        if keep_recording && let Some(p) = &recording {
            detail = format!(
                "{detail} Recoverable video fragments remain in {}.",
                p.file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
        }
        out.push(RecAction::Error {
            title: "MANUAL RECORDING FAILED".into(),
            detail,
            level: Level::Error,
            actions: vec![Action {
                label: "OPEN RECORDINGS".into(),
                id: "open-recordings".into(),
            }],
        });
        if std::mem::take(&mut self.profile_is_recording) {
            out.push(RecAction::RestoreClipProfile);
        }
    }

    /// An engine event.
    pub fn on_engine_event(&mut self, ev: &IpcEvent, now: f64) -> Vec<RecAction> {
        let mut out = Vec::new();
        match ev {
            IpcEvent::RecordingStarted { request_id, path }
                if *request_id == self.request_id && self.state == RecordState::Starting =>
            {
                self.state = RecordState::Recording;
                self.started_at = now;
                self.path = Some(path.clone());
                self.set_label("Stop  00:00:00", true);
                self.transient = None;
            }
            IpcEvent::RecordingStopped {
                request_id, path, ..
            } if *request_id == self.request_id && self.state == RecordState::Stopping => {
                self.state = RecordState::Finalizing;
                self.path = Some(path.clone());
                self.set_label("Finalizing…", true);
                let (tx, rx) = channel();
                let probe_path = path.clone();
                std::thread::Builder::new()
                    .name("fthrust-recording-validate".into())
                    .spawn(move || {
                        let ok = std::fs::metadata(&probe_path)
                            .map(|m| m.len() > 0)
                            .unwrap_or(false);
                        let result = if !ok {
                            Err("The recorder produced no playable video fragments.".to_owned())
                        } else if fthrust_media::probe::probe_video_metadata(&probe_path).is_some()
                        {
                            Ok(())
                        } else {
                            Err("The recording could not be read back as a video.".to_owned())
                        };
                        let _ = tx.send(result);
                    })
                    .ok();
                self.finalize = Some(rx);
            }
            IpcEvent::RecordingError {
                request_id,
                message,
            } if *request_id == self.request_id && self.state != RecordState::Idle => {
                let keep = self.state != RecordState::Starting
                    && self.path.as_ref().is_some_and(|p| p.exists());
                self.fail(message, keep, &mut out, now);
            }
            _ => {}
        }
        out
    }

    /// Timers: the start acknowledgement, the elapsed label, finalisation.
    pub fn on_tick(&mut self, now: f64) -> Vec<RecAction> {
        let mut out = Vec::new();
        match self.state {
            RecordState::Starting if now - self.requested_at > START_ACK_S => {
                self.fail(
                    "The capture engine did not acknowledge the start request.",
                    false,
                    &mut out,
                    now,
                );
            }
            RecordState::Stopping if now - self.requested_at > START_ACK_S => {
                let keep = self.path.as_ref().is_some_and(|p| p.exists());
                self.fail(
                    "The capture engine did not acknowledge the stop request.",
                    keep,
                    &mut out,
                    now,
                );
            }
            RecordState::Recording => {
                let label = format!("Stop  {}", format_elapsed(now - self.started_at));
                self.set_label(&label, true);
                if now - self.last_space_check >= SPACE_CHECK_S {
                    self.last_space_check = now;
                    let dir = self
                        .path
                        .as_ref()
                        .and_then(|p| p.parent().map(Path::to_path_buf));
                    if let Some(dir) = dir
                        && let Ok(free) = free_space_bytes(&dir)
                        && free < self.low_space_floor
                    {
                        out.push(RecAction::Error {
                            title: "RECORDING STOPPED — LOW DISK SPACE".into(),
                            detail: "The recording was stopped because the recording folder dropped below 256 MB free.".into(),
                            level: Level::Warning,
                            actions: vec![Action {
                                label: "OPEN RECORDINGS".into(),
                                id: "open-recordings".into(),
                            }],
                        });
                        let mut stop = self.stop(now);
                        out.append(&mut stop);
                    }
                }
            }
            RecordState::Finalizing => {
                let result = self.finalize.as_ref().and_then(|rx| rx.try_recv().ok());
                if let Some(result) = result {
                    self.finalize = None;
                    match result {
                        Ok(()) => {
                            let path = self.path.take();
                            self.state = RecordState::Idle;
                            self.set_label("Record", false);
                            self.show(
                                "RECORDING SAVED",
                                StatusStyle::Active,
                                Some(now + SAVED_STATUS_S),
                            );
                            out.push(RecAction::RefreshLibrary);
                            out.push(RecAction::Card(CardKind::RecordingSaved {
                                filename: path
                                    .and_then(|p| {
                                        p.file_name().map(|f| f.to_string_lossy().into_owned())
                                    })
                                    .unwrap_or_default(),
                            }));
                            if std::mem::take(&mut self.profile_is_recording) {
                                out.push(RecAction::RestoreClipProfile);
                            }
                        }
                        Err(e) => self.fail(&e, true, &mut out, now),
                    }
                }
            }
            _ => {}
        }
        out
    }
}
