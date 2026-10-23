//! Background services. Every service talks to the UI through
//! [`Event`]s drained by [`Services::tick`] once per frame and
//! [`Request`]s the UI sets as `AppState` flags; nothing else touches the
//! state from another thread.

pub mod diagnostics;
pub mod hotkey_client;
pub mod instance;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::library::scan::{ScanEvent, ScanKind, ScanService};
use crate::shell::error_bar::{Action, Level};
use crate::state::{AppState, StatusStyle, StatusText};

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
    /// Open the export jobs dialog.
    ExportJobs,
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
    /// Stop everything.
    Shutdown,
}

/// Sends events to the UI and wakes it.
#[derive(Clone)]
pub struct Outbox {
    tx: Sender<Event>,
    ctx: egui::Context,
}

impl Outbox {
    /// Sends an event and requests a repaint.
    pub fn send(&self, event: Event) {
        let _ = self.tx.send(event);
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
}

impl Services {
    /// Starts the runtime and the library scan service.
    pub fn start(state: &AppState, ctx: &egui::Context) -> Services {
        let (tx, rx) = channel();
        let outbox = Outbox {
            tx,
            ctx: ctx.clone(),
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
        };
        services.attach_scan(scan);
        services
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
            },
            scan: None,
            scan_forwarder: None,
            runtime: None,
            test_requests: Some(Vec::new()),
            scan_roots: (PathBuf::new(), Vec::new()),
            instance: None,
        }
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
        if self
            .instance
            .as_ref()
            .is_some_and(|i| i.activation_requested())
        {
            apply_event(state, Event::ActivationRequested);
        }
        while let Ok(ev) = self.events.try_recv() {
            apply_event(state, ev);
        }
        for req in collect_requests(state) {
            self.dispatch(req, state);
        }
    }

    fn dispatch(&mut self, req: Request, state: &mut AppState) {
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
            other => tracing::debug!(?other, "request without a consumer yet"),
        }
    }

    /// Stops every service (bounded).
    pub fn shutdown(mut self) {
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
    if std::mem::take(&mut state.export_jobs_requested) {
        out.push(Request::ExportJobs);
    }
    if std::mem::take(&mut state.record_requested) {
        out.push(Request::ToggleRecording);
    }
    if let Some(p) = state.upload_requested.take() {
        out.push(Request::Upload(p));
    }
    if let Some(p) = state.open_clip_requested.take() {
        out.push(Request::OpenClip(p));
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
