//! Background services. Every service talks to the UI through
//! [`Event`]s drained by [`Services::tick`] once per frame and
//! [`Request`]s the UI sets as `AppState` flags; nothing else touches the
//! state from another thread.

pub mod browse;
pub mod camera;
pub mod diagnostics;
pub mod engine;
pub mod export;
pub mod finalize;
pub mod focus;
pub mod game;
pub mod hotkey_client;
pub mod hotkeys;
pub mod instance;
pub mod mic_level;
pub mod playback;
pub mod portal_shortcuts;
pub mod recording;
pub mod save;
pub mod screenshot;
pub mod sounds;
pub mod status;
pub mod upload;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use fthrust_core::capture_health::HealthSnapshot;
use fthrust_core::hotkeys::HotkeyAction;
use fthrust_ipc::Command;

use crate::library::scan::{ScanEvent, ScanKind, ScanService};
use crate::shell::error_bar::{Action, Level};
use crate::state::{AppState, StatusStyle, StatusText};

/// A capture-card notification (drawn by the overlay in Task 15).
#[derive(Clone, Debug, PartialEq)]
pub enum CardKind {
    /// A clip was saved.
    Clip {
        /// Length.
        seconds: u32,
        /// Capture fps.
        fps: u32,
        /// Resolution label (`1080P`, `SOURCE`).
        resolution: String,
    },
    /// A screenshot was saved.
    Screenshot,
    /// A save failed.
    Error {
        /// Detail (may be empty).
        detail: String,
    },
    /// An upload finished.
    Upload {
        /// File name.
        filename: String,
    },
    /// An upload failed.
    UploadFailed {
        /// Detail.
        detail: String,
    },
    /// A manual recording was saved.
    RecordingSaved {
        /// File name.
        filename: String,
    },
    /// A prompt (game detected).
    Prompt {
        /// Text.
        text: String,
    },
    /// Capture switched to a source.
    Capturing {
        /// Source name.
        source: String,
    },
    /// The startup card.
    Startup,
}

/// What a service tells the UI.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The capture status text.
    Status {
        /// Text (the reference's exact strings).
        text: String,
        /// Style.
        style: StatusStyle,
    },
    /// A library scan event.
    Scan(ScanEvent),
    /// A second launch asked for the window.
    ActivationRequested,
    /// An error-bar entry.
    ErrorBar {
        /// Title (normalised by the bar).
        title: String,
        /// Detail.
        detail: String,
        /// Level.
        level: Level,
        /// Actions.
        actions: Vec<Action>,
        /// Incident key.
        key: Option<String>,
        /// Dismiss on action.
        dismiss_on_action: bool,
    },
    /// Remove a keyed incident (recovered).
    IncidentCleared(String),
    /// The engine's active encoder name (settings label).
    EncoderActive(String),
    /// Engine connection state for the pages (Record button, Save).
    EngineConnected(bool),
    /// The latest capture health.
    Health(HealthSnapshot),
    /// A global hotkey fired.
    Hotkey(HotkeyAction),
    /// An engine event for the save / recording services.
    Engine(fthrust_ipc::Event),
    /// Show a capture card.
    Card(CardKind),
    /// A file chooser answered: the paths (empty on cancel) or the error.
    Picked {
        /// What was asked for.
        kind: crate::state::BrowseKind,
        /// The result.
        paths: Result<Vec<PathBuf>, String>,
    },
    /// Play a sound cue.
    Sound(String),
    /// The engine's active (applied) capture configuration.
    ActiveConfig(Option<fthrust_core::capture_config::CaptureConfig>),
    /// The GlobalShortcuts portal bound these (`action`, trigger description).
    PortalShortcutsBound(Vec<(String, String)>),
    /// The GlobalShortcuts portal failed.
    PortalShortcutsFailed(String),
    /// The compositor's toplevel list.
    Toplevels(Vec<fthrust_wayland::toplevels::Toplevel>),
    /// Whether the toplevel protocol is available.
    GameDetectionAvailability(bool),
}

impl PartialEq for ScanEvent {
    fn eq(&self, other: &Self) -> bool {
        matches!((self, other), (ScanEvent::Done { generation: a, .. }, ScanEvent::Done { generation: b, .. }) if a == b)
            || matches!((self, other), (ScanEvent::Batch { generation: a, .. }, ScanEvent::Batch { generation: b, .. }) if a == b)
            || matches!(
                (self, other),
                (ScanEvent::Failed { .. }, ScanEvent::Failed { .. })
            )
    }
}

/// What the UI asks a service to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Rescan the library.
    Scan,
    /// The clips root or import folders changed.
    SetRoots {
        /// Clips root.
        clips_root: PathBuf,
        /// Import roots.
        imported: Vec<PathBuf>,
    },
    /// Restart the capture engine.
    RestartEngine,
    /// An error-bar action id.
    ErrorAction(String),
    /// Refresh the running-window list.
    RefreshWindows,
    /// Open the desktop's shortcut settings.
    OpenDesktopShortcuts,
    /// Re-detect encoders.
    RedetectEncoders,
    /// Export the diagnostic report.
    ExportDiagnostics,
    /// Refresh audio devices.
    RefreshAudioDevices,
    /// Refresh cameras.
    RefreshCameras,
    /// Export the theme ZIP.
    ExportTheme,
    /// Preview a sound.
    PreviewSound(String),
    /// Toggle manual recording.
    ToggleRecording,
    /// Upload a clip.
    Upload(PathBuf),
    /// Open a clip in the editor.
    OpenClip(PathBuf),
    /// Open a screenshot.
    OpenScreenshot(PathBuf),
    /// Show a file in the file manager.
    ShowInFileManager(PathBuf),
    /// Hotkey bindings changed.
    HotkeysChanged,
    /// Restart with the recording quality profile.
    RestartForRecording(fthrust_ipc::EngineConfig),
    /// Pause capture (focus lost).
    PauseCapture,
    /// Resume capture (focus regained).
    ResumeCapture,
    /// Stop everything.
    Shutdown,
}

/// The app id registered with the desktop portals.
pub const APP_ID: &str = "space.kalopsia.fthrust";

/// Sends events to the UI and wakes it.
#[derive(Clone)]
pub struct Outbox {
    tx: Sender<Event>,
    ctx: egui::Context,
    waker: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl Outbox {
    /// An outbox over `tx` that repaints `ctx`.
    pub fn new(tx: Sender<Event>, ctx: egui::Context) -> Self {
        Outbox {
            tx,
            ctx,
            waker: None,
        }
    }

    /// Sends an event, wakes the event loop and requests a repaint.
    pub fn send(&self, event: Event) {
        let _ = self.tx.send(event);
        if let Some(w) = &self.waker {
            w();
        }
        self.ctx.request_repaint();
    }
}

/// The running services.
pub struct Services {
    events: Receiver<Event>,
    outbox: Outbox,
    scan: Option<ScanService>,
    scan_forwarder: Option<std::thread::JoinHandle<()>>,
    runtime: Option<tokio::runtime::Runtime>,
    test_requests: Option<Vec<Request>>,
    scan_roots: (PathBuf, Vec<PathBuf>),
    instance: Option<instance::Instance>,
    engine: Option<engine::EngineDriver>,
    save: save::SaveService,
    recording: recording::RecordingService,
    screenshot: screenshot::ScreenshotService,
    hotkey_server: Option<hotkeys::HotkeyServer>,
    game: game::GameFlow,
    focus: focus::FocusMonitor,
    detector: Option<game::GameDetector>,
    overlay: Option<crate::overlay::Overlay>,
    sounds: Option<sounds::SoundPlayer>,
    last_tokens: Option<crate::tokens::Tokens>,
    portal: Option<portal_shortcuts::PortalShortcuts>,
    portal_active: bool,
    portal_requested: std::collections::BTreeMap<String, String>,
    portal_cleared: std::collections::BTreeSet<String>,
    active_config: Option<fthrust_core::capture_config::CaptureConfig>,
    epoch: std::time::Instant,
    finalizer: finalize::Finalizer,
    upload: upload::UploadService,
    camera: camera::CameraService,
    mic_level: Option<mic_level::MicLevel>,
    mic_level_retry: f64,
    mic_generation: u64,
    gary: Option<crate::overlay::gary::GaryOverlay>,
    gary_key: Option<String>,
    clock_offset: f64,
    wall_override: Option<time::OffsetDateTime>,
    test_commands: Option<Vec<Command>>,
    test_cards: Vec<CardKind>,
    engine_connected: bool,
    health: Option<HealthSnapshot>,
    engine_status: Option<StatusText>,
    recording_transition: bool,
    save_transient_ended: bool,
}

impl Services {
    /// Starts the runtime and the library scan service.
    pub fn start(state: &AppState, ctx: &egui::Context) -> Services {
        Self::start_with_waker(state, ctx, None)
    }

    /// [`Services::start`] with a function the service threads call to wake
    /// the event loop (the runner's user event), so events are handled even
    /// when no frame can be painted.
    pub fn start_with_waker(
        state: &AppState,
        ctx: &egui::Context,
        waker: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    ) -> Services {
        let (tx, rx) = channel();
        let outbox = Outbox {
            tx,
            ctx: ctx.clone(),
            waker,
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("fthrust-services")
            .enable_all()
            .build()
            .ok();
        let imported = imported_roots(state);
        let scan = ScanService::start(state.library.clips_root.clone(), imported.clone());
        scan.request(ScanKind::Foreground);
        let mut services = Services {
            events: rx,
            outbox,
            scan: None,
            scan_forwarder: None,
            runtime,
            test_requests: None,
            scan_roots: (state.library.clips_root.clone(), imported),
            instance: None,
            engine: None,
            save: save::SaveService::new(),
            recording: recording::RecordingService::new(),
            screenshot: screenshot::ScreenshotService::new(),
            hotkey_server: None,
            game: game::GameFlow::default(),
            focus: focus::FocusMonitor::default(),
            detector: None,
            overlay: None,
            sounds: None,
            last_tokens: None,
            portal: None,
            portal_active: false,
            portal_requested: std::collections::BTreeMap::new(),
            portal_cleared: std::collections::BTreeSet::new(),
            active_config: None,
            epoch: std::time::Instant::now(),
            finalizer: finalize::Finalizer::new(),
            upload: upload::UploadService::new(),
            camera: camera::CameraService::new(),
            mic_level: None,
            mic_level_retry: 0.0,
            mic_generation: 0,
            gary: None,
            gary_key: None,
            clock_offset: 0.0,
            wall_override: None,
            test_commands: None,
            test_cards: Vec::new(),
            engine_connected: false,
            health: None,
            engine_status: None,
            recording_transition: false,
            save_transient_ended: false,
        };
        services.attach_scan(scan);
        if let Some(handle) = services.runtime() {
            services
                .screenshot
                .set_portal(std::sync::Arc::new(screenshot::AshpdScreenshot::new(
                    handle,
                )));
        }
        services.start_hotkeys(state);
        services.start_game_detection();
        let assets = crate::assets_dir();
        let output = state
            .settings
            .get_str("notification_monitor")
            .filter(|m| !m.is_empty() && *m != "auto")
            .map(str::to_owned);
        services.overlay = Some(crate::overlay::Overlay::start(
            state.tokens.clone(),
            output,
            assets.clone(),
        ));
        let player = sounds::SoundPlayer::start(assets);
        player.preload_async(&state.theme);
        services.sounds = Some(player);
        services.last_tokens = Some(state.tokens.clone());
        services
    }

    /// Cards and cues queued by the pages and the other services.
    fn flush_cards_and_sounds(&mut self, state: &mut AppState) {
        if self.last_tokens.as_ref() != Some(&state.tokens) {
            self.last_tokens = Some(state.tokens.clone());
            if let Some(o) = &self.overlay {
                o.set_tokens(state.tokens.clone());
            }
        }
        let cards = std::mem::take(&mut state.pending_cards);
        let visuals = state
            .settings
            .get_bool("capture_card_enabled")
            .unwrap_or(true);
        let output = state
            .settings
            .get_str("notification_monitor")
            .filter(|m| !m.is_empty() && *m != "auto")
            .map(str::to_owned);
        for card in cards {
            if let Some(key) = sounds::sound_key_for(&card) {
                state.pending_sounds.push(key.to_owned());
            }
            if visuals
                && crate::overlay::capture_card::visual(&card)
                && let Some(o) = &self.overlay
            {
                o.show_on(card, None, output.clone());
            }
        }
        // Without a player (tests) the cues stay queued for inspection.
        if let Some(p) = &self.sounds {
            for key in std::mem::take(&mut state.pending_sounds) {
                p.play(&state.settings, &state.theme, &key);
            }
        }
    }

    /// Starts the toplevel reader (availability is reported as an event).
    fn start_game_detection(&mut self) {
        let outbox = self.outbox.clone();
        match fthrust_wayland::toplevels::ToplevelWatcher::connect() {
            Ok(w) => self.detector = Some(game::GameDetector::start(Box::new(w), outbox)),
            Err(e) => {
                tracing::info!(error = %e, "toplevel list unavailable");
                outbox.send(Event::GameDetectionAvailability(false));
            }
        }
    }

    fn run_game_actions(&mut self, state: &mut AppState, actions: Vec<game::GameAction>) {
        for a in actions {
            match a {
                game::GameAction::Card(c) => self.show_card(state, c),
                game::GameAction::Restart => self.dispatch(Request::RestartEngine, state),
            }
        }
    }

    fn sync_focus_target(&mut self, state: &AppState) {
        let enabled = state
            .settings
            .get_bool("anticheat_detection_enabled")
            .unwrap_or(false)
            && state.settings.get_str("capture_mode") == Some("window");
        let target = enabled.then(|| {
            state
                .settings
                .get_str("target_window_name")
                .unwrap_or("")
                .to_owned()
        });
        self.focus.set_target(target.as_deref());
    }

    /// Starts hotkey delivery: the private socket, the compositor binds
    /// (Hyprland) and the GlobalShortcuts portal.
    fn start_hotkeys(&mut self, state: &AppState) {
        self.portal_cleared =
            fthrust_core::hotkeys::load_portal_cleared(&state.paths).unwrap_or_default();
        match fthrust_core::runtime_dir() {
            Ok(dir) => {
                match hotkeys::HotkeyServer::start(&dir, state.hotkeys.clone(), self.outbox.clone())
                {
                    Ok(server) => self.hotkey_server = Some(server),
                    Err(e) => tracing::warn!(error = %e, "hotkey socket unavailable"),
                }
            }
            Err(e) => tracing::warn!(error = %e, "no runtime directory for the hotkey socket"),
        }
        self.apply_compositor_binds(state);
        if let Some(handle) = self.runtime() {
            self.portal_requested =
                portal_shortcuts::requested_triggers(&state.hotkeys, &self.portal_cleared);
            self.portal = Some(portal_shortcuts::PortalShortcuts::start(
                handle,
                self.outbox.clone(),
                &state.hotkeys,
                &self.portal_cleared,
            ));
        }
    }

    fn apply_compositor_binds(&self, state: &AppState) {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return;
        };
        let sock = std::path::PathBuf::from(&state.hotkey_socket_path);
        let exe = std::path::PathBuf::from(&state.client_exe);
        match hotkeys::apply_compositor_config(
            &home,
            &state.hotkeys,
            &exe,
            &sock,
            &state.compositor,
            hotkeys::hyprland_instance_present(),
            None,
        ) {
            Ok(a) => tracing::info!(
                changed = a.changed,
                reloaded = a.reloaded,
                "compositor binds applied"
            ),
            Err(e) => tracing::warn!(error = %e, "compositor binds not applied"),
        }
    }

    fn attach_scan(&mut self, scan: ScanService) {
        // Scan events arrive on the scan service's own channel; a forwarder
        // moves them into the outbox so the UI wakes up.
        let (tx, rx) = channel::<ScanEvent>();
        let outbox = self.outbox.clone();
        self.scan_forwarder = Some(
            std::thread::Builder::new()
                .name("fthrust-scan-forward".into())
                .spawn(move || {
                    while let Ok(ev) = rx.recv() {
                        outbox.send(Event::Scan(ev));
                    }
                })
                .expect("spawn scan forwarder"),
        );
        scan.forward_to(tx);
        self.scan = Some(scan);
    }

    /// A runtime without background threads; requests are recorded.
    pub fn start_for_tests(ctx: &egui::Context) -> Services {
        let (tx, rx) = channel();
        Services {
            events: rx,
            outbox: Outbox {
                tx,
                ctx: ctx.clone(),
                waker: None,
            },
            scan: None,
            scan_forwarder: None,
            runtime: None,
            test_requests: Some(Vec::new()),
            scan_roots: (PathBuf::new(), Vec::new()),
            instance: None,
            engine: None,
            save: save::SaveService::new(),
            recording: recording::RecordingService::new(),
            screenshot: screenshot::ScreenshotService::new(),
            hotkey_server: None,
            game: game::GameFlow::default(),
            focus: focus::FocusMonitor::default(),
            detector: None,
            overlay: None,
            sounds: None,
            last_tokens: None,
            portal: None,
            portal_active: false,
            portal_requested: std::collections::BTreeMap::new(),
            portal_cleared: std::collections::BTreeSet::new(),
            active_config: None,
            epoch: std::time::Instant::now(),
            finalizer: finalize::Finalizer::new(),
            upload: upload::UploadService::new(),
            camera: camera::CameraService::new(),
            mic_level: None,
            mic_level_retry: 0.0,
            mic_generation: 0,
            gary: None,
            gary_key: None,
            clock_offset: 0.0,
            wall_override: None,
            test_commands: Some(Vec::new()),
            test_cards: Vec::new(),
            engine_connected: false,
            health: None,
            engine_status: None,
            recording_transition: false,
            save_transient_ended: false,
        }
    }

    /// Installs a screenshot portal implementation (tests).
    pub fn set_screenshot_portal_for_tests(
        &mut self,
        portal: std::sync::Arc<dyn screenshot::PortalScreenshot>,
    ) {
        self.screenshot.set_portal(portal);
    }

    /// Pauses portal calls (tests).
    pub fn hold_screenshot_portal_for_tests(&self, hold: bool) {
        self.screenshot.set_hold(hold);
    }

    /// A screenshot request is running.
    pub fn screenshot_inflight(&self) -> bool {
        self.screenshot.inflight()
    }

    /// Screenshot requests queued behind the running one.
    pub fn screenshot_pending(&self) -> u8 {
        self.screenshot.pending()
    }

    /// The recording state.
    pub fn recording_state(&self) -> recording::RecordState {
        self.recording.state()
    }

    /// A replay save is in flight.
    pub fn save_busy(&self) -> bool {
        self.save.is_busy()
    }

    /// Raises the low-space floor a running recording stops at (tests).
    pub fn set_low_space_floor_for_tests(&mut self, bytes: u64) {
        self.recording.set_low_space_floor(bytes);
    }

    /// Installs a sound sink (tests of the cues and previews).
    pub fn set_sound_sink_for_tests(&mut self, sink: std::sync::Arc<dyn sounds::Sink>) {
        self.sounds = Some(sounds::SoundPlayer::with_sink(sink, crate::assets_dir()));
    }

    /// Pretends the engine applied `config` (tests).
    pub fn set_active_config_for_tests(
        &mut self,
        config: Option<fthrust_core::capture_config::CaptureConfig>,
    ) {
        self.active_config = config;
    }

    fn run_rec_actions(&mut self, state: &mut AppState, actions: Vec<recording::RecAction>) {
        for a in actions {
            match a {
                recording::RecAction::Send(cmd) => self.send_command(cmd),
                recording::RecAction::Card(card) => self.show_card(state, card),
                recording::RecAction::Error {
                    title,
                    detail,
                    level,
                    actions,
                } => state
                    .error_bar
                    .push(&title, &detail, level, actions, None, true),
                recording::RecAction::RestartForRecording(config) => {
                    if self.test_requests.is_some() {
                        self.dispatch(Request::RestartForRecording(config), state);
                    } else if let Some(d) = &self.engine {
                        if let Ok(mut sv) = d.supervisor().lock() {
                            sv.set_recording_preparing(true);
                        }
                        d.request(engine::Input::Restart {
                            config,
                            allow_recording_prepare: true,
                            explicit: false,
                        });
                    }
                }
                recording::RecAction::RestoreClipProfile => {
                    if self.test_requests.is_some() {
                        self.dispatch(Request::RestartEngine, state);
                    } else if let Some(d) = &self.engine
                        && let Ok(config) = engine::engine_config(state)
                    {
                        d.request(engine::Input::Restart {
                            config,
                            allow_recording_prepare: false,
                            explicit: false,
                        });
                    }
                }
                recording::RecAction::RefreshLibrary => state.scan_clips_requested = true,
            }
        }
    }

    /// The Record button or a start/stop hotkey.
    pub fn toggle_recording(&mut self, state: &mut AppState) {
        let now = self.now();
        let wall = self.wall();
        let active = self.active_config.clone();
        let actions = self.recording.toggle(
            state,
            self.engine_connected,
            self.save.is_busy(),
            active.as_ref(),
            now,
            wall,
        );
        self.run_rec_actions(state, actions);
        self.sync_recording_ui(state);
    }

    fn sync_recording_ui(&mut self, state: &mut AppState) {
        state.record_label = self.recording.label.clone();
        state.record_active = self.recording.active_style;
        state.record_enabled = self.recording.button_enabled(self.engine_connected);
        self.recording_transition = matches!(
            self.recording.state(),
            recording::RecordState::Preparing
                | recording::RecordState::Starting
                | recording::RecordState::Stopping
        );
        if let Some(d) = &self.engine
            && let Ok(mut sv) = d.supervisor().lock()
        {
            sv.set_recording(self.recording.busy());
            sv.set_recording_preparing(self.recording.state() == recording::RecordState::Preparing);
        }
    }

    /// Seconds since the services started (tests can advance it).
    pub fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64() + self.clock_offset
    }

    /// Advances the clock (tests).
    pub fn advance_clock_for_tests(&mut self, seconds: f64) {
        self.clock_offset += seconds;
    }

    /// Fixes the wall clock used for file names (tests).
    pub fn set_wall_clock_for_tests(&mut self, at: time::OffsetDateTime) {
        self.wall_override = Some(at);
    }

    fn wall(&self) -> time::OffsetDateTime {
        self.wall_override.unwrap_or_else(|| {
            time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        })
    }

    /// Commands sent to the engine so far (tests).
    pub fn drain_commands_for_tests(&mut self) -> Vec<Command> {
        let taken = self.test_commands.take().unwrap_or_default();
        self.test_commands = Some(Vec::new());
        taken
    }

    /// Cards shown so far (tests).
    pub fn cards_for_tests(&self) -> &[CardKind] {
        &self.test_cards
    }

    /// Sends a command to the engine (recorded in tests).
    pub fn send_command(&mut self, cmd: Command) {
        if let Some(recorded) = self.test_commands.as_mut() {
            recorded.push(cmd);
            return;
        }
        if let Some(d) = &self.engine {
            d.request(engine::Input::Send(cmd));
        }
    }

    fn show_card(&mut self, state: &mut AppState, card: CardKind) {
        if self.test_commands.is_some() {
            self.test_cards.push(card.clone());
        }
        state.pending_cards.push(card);
    }

    fn run_save_actions(&mut self, state: &mut AppState, actions: Vec<save::SaveAction>) {
        for a in actions {
            match a {
                save::SaveAction::Send(cmd) => self.send_command(cmd),
                save::SaveAction::Card(card) => self.show_card(state, card),
                save::SaveAction::Sound(key) => state.pending_sounds.push(key),
                save::SaveAction::Error {
                    title,
                    detail,
                    level,
                    actions,
                } => state
                    .error_bar
                    .push(&title, &detail, level, actions, None, true),
                save::SaveAction::Finalize { path, end, seconds } => {
                    let duration_s = if seconds > 0 {
                        f64::from(seconds)
                    } else {
                        fthrust_media::export::media_facts(&path).map_or(30.0, |f| f.duration_s)
                    };
                    self.finalizer.submit(finalize::FinalizeJob {
                        clip: path,
                        end,
                        duration_s,
                        layers: fthrust_core::finalize::image_layers(&state.settings),
                        camera: self.camera.for_save(state, end, duration_s),
                        camera_rect: fthrust_core::finalize::camera_rect(&state.settings),
                        panic_for_tests: false,
                    });
                }
                save::SaveAction::Diag { event, fields } => {
                    diagnostics::emit(state, "clip_save", &event, fields)
                }
            }
        }
    }

    /// `_save_clip` for `seconds`.
    pub fn save_clip(&mut self, state: &mut AppState, seconds: u32) {
        tracing::info!(
            seconds,
            connected = self.engine_connected,
            "save clip requested"
        );
        let now = self.now();
        let wall = self.wall();
        let health = self.health.clone();
        let actions = self.save.save_clip(
            state,
            seconds,
            self.engine_connected,
            health.as_ref(),
            self.recording_transition,
            now,
            wall,
        );
        self.run_save_actions(state, actions);
        if let Some(d) = &self.engine
            && let Ok(mut sv) = d.supervisor().lock()
        {
            sv.set_save_busy(self.save.is_busy());
        }
    }

    fn on_hotkey(&mut self, state: &mut AppState, action: HotkeyAction) {
        tracing::info!(action = %action.id(), "hotkey");
        match action {
            HotkeyAction::SaveClip => {
                let seconds = state.settings.get_i64("clip_length").unwrap_or(30).max(1) as u32;
                self.save_clip(state, seconds);
            }
            HotkeyAction::SaveCustomClip(n) => self.save_clip(state, n),
            HotkeyAction::SaveScreenshot => state.screenshot_requested = true,
            // The reference gates each key: start only when idle, stop only
            // while recording, so a stray press never ends a long recording.
            HotkeyAction::StartRecording => {
                if self.recording.state() == recording::RecordState::Idle {
                    state.record_requested = true;
                }
            }
            HotkeyAction::StopRecording => {
                if self.recording.state() == recording::RecordState::Recording {
                    state.record_requested = true;
                }
            }
            HotkeyAction::ConfirmGameDetection => state.game_confirm_requested = true,
            HotkeyAction::DismissGameDetection => state.game_dismiss_requested = true,
        }
    }

    /// Starts the engine supervisor for `engine` (the binary) on `socket`
    /// with extra environment, and launches it with the current settings.
    pub fn start_engine_with(
        &mut self,
        engine: PathBuf,
        socket: PathBuf,
        env: Vec<(String, String)>,
        state: &AppState,
    ) {
        let Some(handle) = self.runtime() else {
            return;
        };
        let driver = engine::EngineDriver::start(
            handle,
            self.outbox.clone(),
            engine,
            socket,
            env,
            state.paths.engine_log(),
        );
        if let Ok(config) = engine::engine_config(state) {
            driver.request(engine::Input::Start(config));
        }
        self.engine = Some(driver);
    }

    /// Starts the engine found next to the executable (or `$FTHRUST_ENGINE`).
    pub fn start_engine(&mut self, state: &AppState) {
        match (engine::discover_engine(None), fthrust_ipc::socket_path()) {
            (Some(engine), Ok(socket)) => self.start_engine_with(engine, socket, Vec::new(), state),
            (None, _) => {
                let Some(handle) = self.runtime() else {
                    return;
                };
                let driver = engine::EngineDriver::start(
                    handle,
                    self.outbox.clone(),
                    PathBuf::from("fthrust-engine"),
                    PathBuf::from("engine.sock"),
                    Vec::new(),
                    state.paths.engine_log(),
                );
                driver.request(engine::Input::SpawnFailed {
                    detail: "engine executable not found".into(),
                    missing: true,
                });
                self.engine = Some(driver);
            }
            (_, Err(e)) => tracing::error!("engine socket path: {e}"),
        }
    }

    /// Kills the engine process outright (tests of the exit path).
    pub fn kill_engine_for_tests(&self) {
        if let Some(d) = &self.engine {
            d.kill_process();
        }
    }

    /// The engine supervisor's save admission (plan 5 save flow).
    pub fn engine(&self) -> Option<&engine::EngineDriver> {
        self.engine.as_ref()
    }

    /// Keeps the instance lock and activation listener alive.
    pub fn attach_instance(&mut self, instance: instance::Instance) {
        self.instance = Some(instance);
    }

    /// The outbox (for services started later).
    pub fn outbox(&self) -> &Outbox {
        &self.outbox
    }

    /// The tokio runtime handle, when running.
    pub fn runtime(&self) -> Option<tokio::runtime::Handle> {
        self.runtime.as_ref().map(|r| r.handle().clone())
    }

    /// Queues an event as a service would (tests).
    pub fn inject(&self, event: Event) {
        self.outbox.send(event);
    }

    /// The requests sent so far (tests).
    pub fn drain_requests_for_tests(&mut self) -> Vec<Request> {
        let taken = self.test_requests.take().unwrap_or_default();
        self.test_requests = Some(Vec::new());
        taken
    }

    /// Applies pending events to the state, then turns request flags into
    /// requests.
    pub fn tick(&mut self, state: &mut AppState) {
        {
            let now = self.now();
            if self.test_commands.is_none() {
                self.camera.ensure_scanned();
            }
            self.camera.tick(state, now);
            for outcome in self.finalizer.poll() {
                finalize::apply(state, &outcome);
            }
            self.upload.tick(state, now);
            if self.upload.busy() {
                self.outbox
                    .ctx
                    .request_repaint_after(std::time::Duration::from_millis(250));
            }
            // Nothing else wakes the UI for these: the heartbeat ticks
            // services without painting.
            if self.finalizer.busy() {
                self.outbox
                    .ctx
                    .request_repaint_after(std::time::Duration::from_millis(250));
            }
            if self.camera.preview_active(state) {
                self.outbox
                    .ctx
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
            if self.test_commands.is_none() {
                self.tick_mic_and_gary(state, now);
            }
            if let Some(kind) = state.browse_requested.take()
                && let Some(handle) = self.runtime()
            {
                let outbox = self.outbox.clone();
                handle.spawn(async move {
                    let paths = browse::pick(&kind).await;
                    outbox.send(Event::Picked { kind, paths });
                });
            }
        }
        if self
            .instance
            .as_ref()
            .is_some_and(|i| i.activation_requested())
        {
            apply_event(state, Event::ActivationRequested);
        }
        let now = self.now();
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Hotkey(action) => self.on_hotkey(state, action),
                Event::Engine(ipc) => {
                    let actions = self.save.on_engine_event(state, &ipc, now);
                    self.run_save_actions(state, actions);
                    let actions = self.recording.on_engine_event(&ipc, now);
                    self.run_rec_actions(state, actions);
                    state.pending_engine_events.push(ipc);
                }
                Event::ActiveConfig(c) => self.active_config = c,
                Event::PortalShortcutsBound(bound) => {
                    let (changed, kept) = portal_shortcuts::adopt_bound(
                        &mut state.hotkeys,
                        &self.portal_requested,
                        &self.portal_cleared,
                        &bound,
                    );
                    if changed {
                        let _ = state.hotkeys.save(&state.paths);
                    }
                    self.portal_active = true;
                    state.hotkeys_portal_active = true;
                    if !kept.is_empty() {
                        state.error_bar.push(
                            "DESKTOP KEPT ITS SHORTCUT",
                            &portal_shortcuts::kept_warning(&kept),
                            Level::Warning,
                            Vec::new(),
                            None,
                            true,
                        );
                    }
                }
                Event::Toplevels(snapshot) => {
                    self.sync_focus_target(state);
                    let was_paused = self.focus.paused();
                    let change = self.focus.observe(&snapshot);
                    let actions = self.game.on_snapshot(state, snapshot, now);
                    self.run_game_actions(state, actions);
                    // Switching sources changes the focus target.
                    self.sync_focus_target(state);
                    match change {
                        Some(focus::FocusChange::Lost) => {
                            self.dispatch(Request::PauseCapture, state);
                            state.status = StatusText {
                                text: status::FOCUS_PAUSED.into(),
                                style: StatusStyle::Warning,
                            };
                        }
                        Some(focus::FocusChange::Regained) => {
                            self.dispatch(Request::ResumeCapture, state);
                            state.status = StatusText {
                                text: "STARTING CAPTURE".into(),
                                style: StatusStyle::Idle,
                            };
                        }
                        None => {
                            if was_paused && !self.focus.paused() {
                                self.dispatch(Request::ResumeCapture, state);
                            }
                        }
                    }
                }
                Event::GameDetectionAvailability(available) => {
                    state.game_detection_available = available;
                    if !available {
                        state.running_windows.clear();
                        state.error_bar.push(
                            "LIMITED FEATURE SUPPORT",
                            "Game detection and focus pause need a compositor that exposes its window list (wlr-foreign-toplevel-management). Hotkeys and capture are unaffected.",
                            Level::Warning,
                            Vec::new(),
                            None,
                            true,
                        );
                    }
                }
                Event::PortalShortcutsFailed(reason) => {
                    tracing::info!(reason, "GlobalShortcuts portal");
                    if self.portal_active {
                        state.error_bar.push(
                            "GLOBAL HOTKEYS",
                            &reason,
                            Level::Warning,
                            Vec::new(),
                            None,
                            true,
                        );
                    } else if let Some(p) = self.portal.take() {
                        p.stop();
                    }
                }
                Event::Health(h) => self.health = Some(h),
                Event::EngineConnected(on) => {
                    self.engine_connected = on;
                    if !on {
                        self.health = None;
                        let actions = self.save.on_engine_lost(state, now);
                        self.run_save_actions(state, actions);
                        let actions = self.recording.on_engine_lost(now);
                        self.run_rec_actions(state, actions);
                        self.sync_recording_ui(state);
                    }
                    apply_event(state, Event::EngineConnected(on));
                }
                Event::Status { text, style } => {
                    if self.engine_status.as_ref().is_none_or(|s| s.text != text) {
                        tracing::info!(status = %text, "engine status");
                    }
                    self.engine_status = Some(StatusText {
                        text: text.clone(),
                        style,
                    });
                    apply_event(state, Event::Status { text, style });
                }
                Event::Card(card) => self.show_card(state, card),
                Event::Picked { kind, paths } => match paths {
                    Ok(paths) => browse::apply(state, kind, paths),
                    Err(e) => {
                        tracing::warn!(error = %e, "file chooser unavailable");
                        state.dialogs.push_message(
                            "File chooser unavailable",
                            format!("The desktop file chooser could not open: {e}"),
                        );
                    }
                },
                other => apply_event(state, other),
            }
        }
        // Requests set by the pages this frame (UI-thread services act now,
        // before the transient statuses are applied).
        for req in collect_requests(state) {
            self.dispatch(req, state);
        }
        if std::mem::take(&mut state.game_confirm_requested) {
            let actions = self.game.confirm(state);
            self.run_game_actions(state, actions);
        }
        if std::mem::take(&mut state.game_dismiss_requested) {
            self.game.dismiss();
        }
        self.game.on_tick(now);
        if std::mem::take(&mut state.screenshot_requested) {
            let wall = self.wall();
            let actions = self.screenshot.request(state, wall);
            screenshot::apply(state, actions, &mut self.test_cards);
        }
        {
            let wall = self.wall();
            let actions = self.screenshot.on_tick(state, wall);
            screenshot::apply(state, actions, &mut self.test_cards);
        }
        let actions = self.save.on_tick(state, now);
        self.run_save_actions(state, actions);
        let actions = self.recording.on_tick(now);
        self.run_rec_actions(state, actions);
        let active = self.active_config.clone();
        let actions = self
            .recording
            .continue_prepared(self.engine_connected, active.as_ref(), now);
        self.run_rec_actions(state, actions);
        self.sync_recording_ui(state);
        // A transient save status overrides the engine's; when it ends the
        // engine's last status returns.
        let transient = self
            .recording
            .transient_status(now)
            .or_else(|| self.save.transient_status(now))
            .or_else(|| self.upload.transient_status(now));
        match transient {
            Some(t) => {
                state.status = t;
                self.save_transient_ended = true;
            }
            None if self.save_transient_ended => {
                if let Some(t) = &self.engine_status {
                    state.status = t.clone();
                }
                self.save_transient_ended = false;
            }
            None => {}
        }
        if let Some(d) = &self.engine
            && let Ok(mut sv) = d.supervisor().lock()
        {
            sv.set_save_busy(self.save.is_busy());
        }
        self.flush_cards_and_sounds(state);
    }

    fn dispatch(&mut self, req: Request, state: &mut AppState) {
        // UI-thread services act directly, in tests too.
        if req == Request::ToggleRecording {
            self.toggle_recording(state);
            return;
        }
        if let Request::PreviewSound(key) = &req {
            if let Some(p) = &self.sounds {
                p.preview(&state.settings, &state.theme, key);
            }
            return;
        }
        if let Some(recorded) = self.test_requests.as_mut() {
            recorded.push(req);
            return;
        }
        match req {
            Request::Scan => {
                if let Some(s) = &self.scan {
                    s.request(ScanKind::Foreground);
                }
            }
            Request::SetRoots {
                clips_root,
                imported,
            } => {
                self.scan_roots = (clips_root.clone(), imported.clone());
                state.library.clips_root = clips_root.clone();
                if let Some(s) = &self.scan {
                    s.set_roots(clips_root, imported);
                }
            }
            Request::ExportDiagnostics => {
                // Served by the dialogs module (`dialogs::service`).
                state.diagnostic_export_requested = true;
            }
            Request::ToggleRecording => self.toggle_recording(state),
            Request::PreviewSound(key) => {
                if let Some(p) = &self.sounds {
                    p.preview(&state.settings, &state.theme, &key);
                }
            }
            Request::RestartEngine => {
                if let Some(d) = &self.engine {
                    match engine::engine_config(state) {
                        Ok(config) => d.request(engine::Input::Restart {
                            config,
                            allow_recording_prepare: false,
                            explicit: false,
                        }),
                        Err(e) => state.error_bar.push(
                            "SETTINGS APPLY FAILED",
                            &e,
                            Level::Error,
                            vec![],
                            None,
                            true,
                        ),
                    }
                }
            }
            Request::ErrorAction(id) if self.upload.error_action(state, &id) => {}
            Request::Upload(path) => self.upload.enqueue(state, path),
            Request::ErrorAction(id) => match id.as_str() {
                "restart" | "restart-capture" => {
                    if let Some(d) = &self.engine
                        && let Ok(config) = engine::engine_config(state)
                    {
                        d.request(engine::Input::Restart {
                            config,
                            allow_recording_prepare: false,
                            explicit: true,
                        });
                    }
                }
                "open-folder" => {
                    if let Ok(dir) = state.settings.clips_directory() {
                        let _ = std::fs::create_dir_all(&dir);
                        crate::shell::open_url(&dir.display().to_string());
                    }
                }
                "open-recordings" => {
                    if let Ok(dir) = state.settings.recording_directory() {
                        let _ = std::fs::create_dir_all(&dir);
                        crate::shell::open_url(&dir.display().to_string());
                    }
                }
                "open-settings" | "open-audio-settings" => {
                    state.page = crate::state::Page::Settings;
                    if id == "open-audio-settings" {
                        state.settings_tab = crate::state::Tab::Audio;
                    }
                }
                "edit-game-crop" => state.popups.game.show(),
                "open-camera-settings" => {
                    state.page = crate::state::Page::Settings;
                    state.settings_tab = crate::state::Tab::Visuals;
                }
                "configure-shortcuts" => state.open_desktop_shortcuts_requested = true,
                "learn-more" => crate::shell::open_url("https://github.com/kalopsia-space/FTHRust"),
                other => tracing::debug!(other, "unknown error-bar action"),
            },
            Request::HotkeysChanged => {
                if let Some(s) = &self.hotkey_server {
                    s.update_bindings(state.hotkeys.clone());
                }
                self.apply_compositor_binds(state);
                self.portal_requested = portal_shortcuts::requested_triggers(&state.hotkeys, &self.portal_cleared);
                if let Some(p) = &self.portal {
                    p.rebind(&state.hotkeys, &self.portal_cleared);
                }
            }
            Request::RefreshWindows => {
                if let Some(d) = &self.detector {
                    d.refresh();
                }
            }
            Request::PauseCapture => {
                if let Some(d) = &self.engine {
                    d.request(engine::Input::Pause);
                }
            }
            Request::ResumeCapture => {
                if let Some(d) = &self.engine {
                    d.request(engine::Input::Resume);
                }
            }
            Request::OpenDesktopShortcuts => match &self.portal {
                Some(p) => p.configure(),
                None => state.error_bar.push(
                    "GLOBAL HOTKEYS",
                    "Your desktop does not offer the GlobalShortcuts portal; bind the socket commands in your compositor instead.",
                    Level::Warning,
                    Vec::new(),
                    None,
                    true,
                ),
            },
            Request::RefreshCameras => self.camera.refresh(),
            other => tracing::debug!(?other, "request without a consumer yet"),
        }
    }

    /// The mic level stream (Audio page meter, Gary) and the Gary thread.
    fn tick_mic_and_gary(&mut self, state: &mut AppState, now: f64) {
        use crate::state::{Page, Tab};
        let audio_on = state
            .settings
            .get_bool("audio_capture_enabled")
            .unwrap_or(true);
        let gary_on = state.flag("gary_mode_enabled", false) && audio_on;
        let audio_page = state.page == Page::Settings && state.settings_tab == Tab::Audio;
        let target = state
            .settings
            .get_str("mic_device_id")
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let gain = state
            .settings
            .get_i64("mic_volume")
            .unwrap_or(100)
            .clamp(0, 200) as f32
            / 100.0;
        if audio_page || gary_on {
            let restart =
                mic_level::needs_restart(self.mic_level.as_ref().map(|m| &m.target), &target);
            if let Some(m) = &self.mic_level {
                m.set_gain(gain);
            }
            if restart && now >= self.mic_level_retry {
                self.mic_level = None;
                match mic_level::MicLevel::start(target, gain) {
                    Ok(m) => {
                        self.mic_level = Some(m);
                        // Gary must pick up the new stream's meter.
                        self.mic_generation += 1;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "mic level unavailable");
                        self.mic_level_retry = now + 5.0;
                    }
                }
            }
        } else {
            self.mic_level = None;
        }
        state.mic_level = self.mic_level.as_ref().map(|m| m.read());
        if audio_page && self.mic_level.is_some() {
            self.outbox
                .ctx
                .request_repaint_after(std::time::Duration::from_millis(60));
        }
        // Gary: started on first use, told about every change.
        if gary_on && self.gary.is_none() {
            self.gary = Some(crate::overlay::gary::GaryOverlay::start(crate::assets_dir()));
        }
        if let Some(g) = &self.gary {
            let (low, high) = fthrust_core::gary::clamp_thresholds(
                state
                    .settings
                    .get_i64("gary_min_level")
                    .unwrap_or(fthrust_core::gary::DEFAULT_MIN),
                state
                    .settings
                    .get_i64("gary_max_level")
                    .unwrap_or(fthrust_core::gary::DEFAULT_MAX),
            );
            let image = state
                .settings
                .get_str("gary_image_path")
                .filter(|p| !p.is_empty())
                .map(std::path::PathBuf::from)
                .filter(|p| p.is_file());
            let output = state
                .settings
                .get_str("notification_monitor")
                .filter(|m| !m.is_empty() && *m != "auto")
                .map(str::to_owned);
            let key = format!(
                "{gary_on}|{low}|{high}|{image:?}|{output:?}|{}|{}",
                self.mic_level.is_some(),
                self.mic_generation
            );
            if self.gary_key.as_deref() != Some(&key) {
                self.gary_key = Some(key);
                g.set(crate::overlay::gary::GarySettings {
                    enabled: gary_on,
                    thresholds: (low, high),
                    image,
                    output,
                    meter: self.mic_level.as_ref().map(|m| m.meter()),
                });
            }
        }
    }

    /// The reference's exit order, part one: exports are cancelled (12 s
    /// bound) and a running manual recording is stopped and finalised
    /// (bounded) before anything else goes away.
    pub fn prepare_exit(&mut self, state: &mut AppState) {
        // `_perform_full_shutdown`: close the share windows (cancelling
        // their exports), then cancel queued exports with a bounded wait.
        crate::editor::share::close_all(state);
        self.finalizer.shutdown(std::time::Duration::from_secs(5));
        self.upload.shutdown();
        self.camera.shutdown();
        self.gary = None;
        self.mic_level = None;
        state
            .export_queue
            .shutdown(std::time::Duration::from_secs(12));
        if self.recording.state() == recording::RecordState::Recording {
            self.toggle_recording(state);
        }
        if self.test_commands.is_some() || self.engine.is_none() {
            return;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while self.recording.busy() && std::time::Instant::now() < deadline {
            match self
                .events
                .recv_timeout(std::time::Duration::from_millis(50))
            {
                Ok(Event::Engine(ipc)) => {
                    let now = self.now();
                    let actions = self.recording.on_engine_event(&ipc, now);
                    self.run_rec_actions(state, actions);
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
            let now = self.now();
            let actions = self.recording.on_tick(now);
            self.run_rec_actions(state, actions);
        }
    }

    /// Stops every service (bounded), in the reference's order: input and
    /// detection first, then feedback, then the engine, then the lock.
    pub fn shutdown(mut self) {
        if let Some(d) = self.detector.take() {
            d.stop();
        }
        if let Some(p) = self.portal.take() {
            p.stop();
        }
        if let Some(s) = self.hotkey_server.take() {
            s.close();
        }
        if let Some(p) = self.sounds.take() {
            p.stop();
            drop(p);
        }
        if let Some(o) = self.overlay.take() {
            o.shutdown();
        }
        if let Some(engine) = self.engine.take() {
            engine.shutdown();
        }
        if let Some(scan) = self.scan.take() {
            scan.stop();
        }
        self.instance.take();
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_timeout(std::time::Duration::from_secs(3));
        }
    }
}

fn imported_roots(state: &AppState) -> Vec<PathBuf> {
    state
        .settings
        .get("imported_clip_folders")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

fn apply_event(state: &mut AppState, ev: Event) {
    match ev {
        Event::Status { text, style } => state.status = StatusText { text, style },
        Event::Scan(ScanEvent::Batch { records, .. }) => {
            if state.library.records.is_empty() && !records.is_empty() {
                state.library.records = records;
                state.library.scanned = true;
                state.library_dirty = true;
            }
        }
        Event::Scan(ScanEvent::Done { records, .. }) => {
            if state.library.records != records || !state.library.scanned {
                state.library.records = records;
                state.library_dirty = true;
            }
            state.library.scanned = true;
            state.thumb_requested.clear();
            state.storage_error_reported = false;
        }
        Event::Scan(ScanEvent::Failed { error, .. }) => {
            if !state.storage_error_reported {
                state.storage_error_reported = true;
                state.dialogs.push_message(
                    "Clips Folder Unavailable",
                    format!("{error}\n\nFTHR will not switch to another folder. Restore access or choose a writable clips folder in Settings."),
                );
            }
            state.library.scanned = true;
            state.library_dirty = true;
        }
        Event::ActivationRequested => {
            state.activation_requested = true;
        }
        Event::ErrorBar {
            title,
            detail,
            level,
            actions,
            key,
            dismiss_on_action,
        } => state
            .error_bar
            .push(&title, &detail, level, actions, key, dismiss_on_action),
        Event::IncidentCleared(key) => {
            state.error_bar.remove_by_key(&key);
        }
        Event::EncoderActive(codec) => state.active_encoder = Some(codec),
        Event::Health(_)
        | Event::Hotkey(_)
        | Event::Engine(_)
        | Event::ActiveConfig(_)
        | Event::PortalShortcutsBound(_)
        | Event::PortalShortcutsFailed(_)
        | Event::Toplevels(_)
        | Event::GameDetectionAvailability(_) => {}
        Event::Card(card) => state.pending_cards.push(card),
        Event::Picked {
            kind,
            paths: Ok(paths),
        } => browse::apply(state, kind, paths),
        Event::Picked { paths: Err(e), .. } => {
            state.dialogs.push_message(
                "File chooser unavailable",
                format!("The desktop file chooser could not open: {e}"),
            );
        }
        Event::Sound(key) => state.pending_sounds.push(key),
        Event::EngineConnected(on) => {
            state.engine_connected = on;
            if on && !state.startup_cue_played {
                state.startup_cue_played = true;
                state.pending_sounds.push("startup".into());
            }
        }
    }
}

fn collect_requests(state: &mut AppState) -> Vec<Request> {
    let mut out = Vec::new();
    if std::mem::take(&mut state.scan_clips_requested) {
        out.push(Request::Scan);
    }
    if std::mem::take(&mut state.clips_directory_changed)
        || std::mem::take(&mut state.imported_folders_changed)
    {
        let clips_root = state
            .settings
            .clips_directory()
            .unwrap_or_else(|_| state.paths.default_clips_directory());
        out.push(Request::SetRoots {
            clips_root,
            imported: imported_roots(state),
        });
    }
    if std::mem::take(&mut state.engine_restart_requested) {
        out.push(Request::RestartEngine);
    }
    if let Some(id) = state.error_action_requested.take() {
        out.push(Request::ErrorAction(id));
    }
    if std::mem::take(&mut state.refresh_windows_requested) {
        out.push(Request::RefreshWindows);
    }
    if std::mem::take(&mut state.open_desktop_shortcuts_requested) {
        out.push(Request::OpenDesktopShortcuts);
    }
    if std::mem::take(&mut state.redetect_encoders_requested) {
        out.push(Request::RedetectEncoders);
    }
    if std::mem::take(&mut state.refresh_audio_devices_requested) {
        out.push(Request::RefreshAudioDevices);
    }
    if std::mem::take(&mut state.refresh_cameras_requested) {
        out.push(Request::RefreshCameras);
    }
    if std::mem::take(&mut state.theme_export_requested) {
        out.push(Request::ExportTheme);
    }
    if let Some(key) = state.sound_preview_requested.take() {
        out.push(Request::PreviewSound(key));
    }
    if std::mem::take(&mut state.record_requested) {
        out.push(Request::ToggleRecording);
    }
    if let Some(p) = state.upload_requested.take() {
        out.push(Request::Upload(p));
    }
    if let Some(p) = state.open_screenshot_requested.take() {
        out.push(Request::OpenScreenshot(p));
    }
    if let Some(p) = state.show_in_file_manager_requested.take() {
        out.push(Request::ShowInFileManager(p));
    }
    if std::mem::take(&mut state.hotkeys_changed) {
        out.push(Request::HotkeysChanged);
    }
    out
}
