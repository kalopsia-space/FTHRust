//! The IPC service: one client at a time over the Unix socket, the engine
//! and its workers behind it, a 500 ms heartbeat, a stdin-EOF parent watch
//! and signal handling.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use fthrust_ipc::transport::{Connection, EngineServer};
use fthrust_ipc::{
    BackendKind, Command, Event, IpcError, PROTOCOL_VERSION, RecordingState, SavedClip,
    StatusSnapshot,
};
use tokio::signal::unix::SignalKind;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::engine::{CaptureEngine, EngineDeps, EngineNotice};
use crate::save::{RecordWorker, SaveRequest, SaveWorker, panic_message};

/// Heartbeat period.
pub const HEARTBEAT: Duration = Duration::from_millis(500);
/// Longest a shutdown waits for a save in flight.
pub const SHUTDOWN_SAVE_WAIT: Duration = Duration::from_secs(60);
/// Longest a shutdown waits for capture, audio and a recording to stop.
pub const SHUTDOWN_STOP_WAIT: Duration = Duration::from_secs(15);
/// Longest a shutdown waits for its last events to reach the client.
pub const SHUTDOWN_FLUSH_WAIT: Duration = Duration::from_secs(1);
/// How long the runtime may take to wind down once the service is done.
/// Work still running then (a configure job stuck in a driver) is
/// abandoned with the process.
const RUNTIME_SHUTDOWN_WAIT: Duration = Duration::from_secs(1);
/// The `StartupError` code for a configure job that panicked.
pub const CONFIGURE_FAILED: &str = "CONFIGURE_FAILED";

/// Why the service could not run.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// The socket could not be bound.
    #[error("{0}")]
    Ipc(#[from] IpcError),
    /// The async runtime could not start.
    #[error("tokio: {0}")]
    Runtime(std::io::Error),
}

/// What ends the service besides a `Shutdown` command. In production these
/// come from stdin EOF and SIGTERM/SIGINT; tests send them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The parent closed our stdin.
    ParentGone,
    /// SIGTERM or SIGINT. The first one starts a graceful shutdown; another
    /// one during it stops waiting for the save and the capture threads.
    Signal,
}

enum Internal {
    Notice(EngineNotice),
    Worker(Event),
    Stop(Stop),
}

struct ConfigureOutcome {
    engine: CaptureEngine,
    result: Result<(Vec<String>, Vec<crate::engine::StartupNotice>), crate::engine::StartupError>,
    fresh: bool,
}

struct Service {
    configuring: Option<tokio::task::JoinHandle<ConfigureOutcome>>,
    deps: Option<EngineDeps>,
    default_override: Option<BackendKind>,
    engine: Option<CaptureEngine>,
    save: Option<SaveWorker>,
    record: Option<RecordWorker>,
    startup_events: Vec<Event>,
    last_save: Option<(u64, Result<SavedClip, String>)>,
    bytes_written: u64,
    dirty: bool,
    /// Saves handed to the save worker; each is answered by exactly one
    /// `SaveResult` from it.
    saves_submitted: u64,
    /// `SaveResult`s received from the save worker.
    save_results: u64,
    internal_tx: mpsc::Sender<Internal>,
    worker_tx: crossbeam_channel::Sender<Event>,
}

impl Service {
    fn status(&self) -> StatusSnapshot {
        let mut s = match &self.engine {
            Some(e) => e.status(),
            None => StatusSnapshot {
                frames_captured: 0,
                bytes_written: 0,
                encoder_open: false,
                active_codec: String::new(),
                health_flags: 0,
                capture_generation: 0,
                encoder_generation: 0,
                content_sample_sequence: 0,
                content_suspicious_streak: 0,
                content_luma_mean: 0.0,
                content_luma_variance: 0.0,
                capture_failure_detail: String::new(),
                recovery_reason: String::new(),
                paused: false,
                recording: RecordingState::Idle,
                backend: None,
                selected_output: String::new(),
                native_width: 0,
                native_height: 0,
                audio: fthrust_ipc::AudioStatus {
                    system: fthrust_ipc::SourceStatus::Disabled,
                    microphone: fthrust_ipc::SourceStatus::Disabled,
                },
                last_save: None,
            },
        };
        s.bytes_written = self.bytes_written;
        s.last_save = self.last_save.clone();
        if let Some(r) = &self.record {
            s.recording = r.state();
        }
        s
    }

    /// Whether a submitted save has not been answered yet.
    fn save_pending(&self) -> bool {
        self.save_results < self.saves_submitted
    }

    /// Applies a command; returns `Some(code)` when the process must exit.
    fn handle(
        &mut self,
        command: Command,
        events: Option<&mpsc::Sender<Event>>,
    ) -> Option<ExitCode> {
        match command {
            Command::Configure(config) => {
                let mut config = config;
                if config.backend_override.is_none() {
                    config.backend_override = self.default_override;
                }
                if self.configuring.is_some() {
                    emit(
                        events,
                        Event::StartupError {
                            code: "ENGINE_BUSY".into(),
                            detail: "a Configure is already being applied".into(),
                        },
                    );
                    return None;
                }
                // Starting or reconfiguring can take seconds (backend dialogs,
                // driver init): run it off the loop so heartbeats, signals and
                // save acknowledgements keep flowing.
                let job: Box<dyn FnOnce() -> ConfigureOutcome + Send> = match self.engine.take() {
                    None => {
                        let deps = self.deps.take()?;
                        Box::new(move || {
                            let mut engine = CaptureEngine::new(config, deps);
                            let warnings = engine.config().normalized().1;
                            let result = engine.start().map(|n| (warnings, n));
                            ConfigureOutcome {
                                engine,
                                result,
                                fresh: true,
                            }
                        })
                    }
                    Some(mut engine) => Box::new(move || {
                        let result = engine.reconfigure(config);
                        ConfigureOutcome {
                            engine,
                            result,
                            fresh: false,
                        }
                    }),
                };
                self.configuring = Some(tokio::task::spawn_blocking(job));
            }
            Command::SaveClip {
                request_id,
                path,
                seconds,
            } => match &self.save {
                Some(save) => {
                    self.bytes_written = 0;
                    self.saves_submitted += 1;
                    save.submit(SaveRequest {
                        request_id,
                        path,
                        seconds,
                    })
                }
                None => emit(
                    events,
                    Event::SaveResult {
                        request_id,
                        result: Err(if self.configuring.is_some() {
                            "Capture is starting"
                        } else {
                            "Capture is not running"
                        }
                        .into()),
                    },
                ),
            },
            Command::StartRecording { request_id, path } => match &self.record {
                Some(r) => r.start(request_id, path),
                None => emit(
                    events,
                    Event::RecordingError {
                        request_id,
                        message: "Capture is not running".into(),
                    },
                ),
            },
            Command::StopRecording { request_id } => match &self.record {
                Some(r) => r.stop(request_id),
                None => emit(
                    events,
                    Event::RecordingError {
                        request_id,
                        message: "No recording is in progress".into(),
                    },
                ),
            },
            Command::Pause => {
                if let Some(e) = &self.engine {
                    e.set_paused(true);
                }
            }
            Command::Resume => {
                if let Some(e) = &self.engine {
                    e.set_paused(false);
                }
            }
            Command::GetStatus => {
                emit(events, Event::Status(Box::new(self.status())));
                self.dirty = false;
            }
            Command::Shutdown => return Some(ExitCode::SUCCESS),
        }
        None
    }

    fn on_worker(&mut self, event: &Event) {
        match event {
            Event::SaveResult { request_id, result } => {
                self.save_results += 1;
                self.last_save = Some((*request_id, result.clone()));
            }
            Event::SaveProgress { bytes_written, .. } => self.bytes_written = *bytes_written,
            _ => {}
        }
        self.dirty = true;
    }

    /// Finishes a `Configure` job: installs the engine and its workers (first
    /// time) and emits the reply events. Returns an exit code on a startup
    /// error of a fresh engine.
    fn finish_configure(
        &mut self,
        outcome: ConfigureOutcome,
        events: Option<&mpsc::Sender<Event>>,
    ) -> Option<ExitCode> {
        let ConfigureOutcome {
            engine,
            result,
            fresh,
        } = outcome;
        match result {
            Ok((warnings, notices)) => {
                for n in notices {
                    let e = Event::StartupWarning {
                        code: n.code,
                        detail: n.detail,
                    };
                    if fresh {
                        self.startup_events.push(e.clone());
                    }
                    emit(events, e);
                }
                if fresh {
                    let handle = engine.handle();
                    self.save = Some(SaveWorker::spawn(handle.clone(), self.worker_tx.clone()));
                    self.record = Some(RecordWorker::new(handle, self.worker_tx.clone()));
                    let notices = engine.events();
                    let tx = self.internal_tx.clone();
                    std::thread::spawn(move || {
                        for n in notices {
                            if tx.blocking_send(Internal::Notice(n)).is_err() {
                                break;
                            }
                        }
                    });
                }
                self.engine = Some(engine);
                emit(events, Event::Configured { warnings });
                self.dirty = true;
                None
            }
            Err(e) => {
                let event = Event::StartupError {
                    code: e.code.to_string(),
                    detail: e.detail,
                };
                self.engine = Some(engine);
                self.dirty = true;
                if fresh {
                    self.startup_events.push(event.clone());
                    emit(events, event);
                    Some(ExitCode::from(1))
                } else {
                    emit(events, event);
                    None
                }
            }
        }
    }

    /// A `Configure` job panicked. The engine (or the dependencies of a
    /// first start) went down with it, so nothing can capture any more:
    /// report a `StartupError` and exit, like a failed first start. Waiting
    /// on would leave every later `Configure` silently ignored.
    fn configure_failed(
        &mut self,
        error: tokio::task::JoinError,
        events: Option<&mpsc::Sender<Event>>,
    ) -> Option<ExitCode> {
        let why = if error.is_panic() {
            panic_message(error.into_panic().as_ref())
        } else {
            "it was cancelled".to_string()
        };
        let detail = format!("applying the capture settings failed: {why}");
        tracing::error!("{detail}");
        eprintln!("FTHR_STARTUP_ERROR: {CONFIGURE_FAILED}: {detail}");
        let event = Event::StartupError {
            code: CONFIGURE_FAILED.into(),
            detail,
        };
        self.startup_events.push(event.clone());
        self.dirty = true;
        emit(events, event);
        Some(ExitCode::from(1))
    }

    fn shutdown(&mut self) {
        if let Some(r) = &self.record {
            if r.state() != RecordingState::Idle {
                r.stop(0);
            }
            r.join();
        }
        if let Some(e) = self.engine.as_mut() {
            e.shutdown();
        }
    }
}

/// Sends an event to the client, if there is one. A client that is slow to
/// read gets it delivered by a task instead of losing it. With no client the
/// event is dropped: nothing is ever queued for a reader that does not exist.
fn emit(events: Option<&mpsc::Sender<Event>>, event: Event) {
    let Some(events) = events else {
        return;
    };
    if let Err(mpsc::error::TrySendError::Full(event)) = events.try_send(event) {
        let tx = events.clone();
        tokio::spawn(async move {
            let _ = tx.send(event).await;
        });
    }
}

/// Sends an event during shutdown: to the client if there is one, and
/// never past `deadline`.
async fn deliver(client: Option<&Connection>, event: Event, deadline: Instant) {
    if let Some(c) = client {
        let _ = tokio::time::timeout_at(deadline, c.events.send(event)).await;
    }
}

/// Runs the service on `socket` until shutdown; returns the exit code.
pub fn serve(
    socket: PathBuf,
    deps: EngineDeps,
    default_override: Option<BackendKind>,
) -> Result<ExitCode, DaemonError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(DaemonError::Runtime)?;
    let served = runtime.block_on(async {
        let (stop_tx, stop_rx) = mpsc::channel(8);
        // Registered here, before the socket is bound and the ready line is
        // printed, so a signal racing the ready line is caught.
        forward_signals(
            &[SignalKind::terminate(), SignalKind::interrupt()],
            &stop_tx,
        );
        watch_parent(stop_tx);
        serve_until(socket, deps, default_override, stop_rx).await
    });
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_WAIT);
    served
}

/// Sends [`Stop::Signal`] for every delivery of each of `kinds`, for as long
/// as the service listens. Must run inside the runtime.
pub fn forward_signals(kinds: &[SignalKind], stops: &mpsc::Sender<Stop>) {
    for kind in kinds {
        match tokio::signal::unix::signal(*kind) {
            Ok(mut stream) => {
                let tx = stops.clone();
                tokio::spawn(async move {
                    while stream.recv().await.is_some() {
                        if tx.send(Stop::Signal).await.is_err() {
                            break;
                        }
                    }
                });
            }
            Err(e) => tracing::warn!("cannot listen for {kind:?}: {e}"),
        }
    }
}

/// Parent watch: the UI holds our stdin; EOF means it is gone.
fn watch_parent(stops: mpsc::Sender<Stop>) {
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = [0u8; 64];
        let mut stdin = std::io::stdin();
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = stops.blocking_send(Stop::ParentGone);
    });
}

/// Runs the service on `socket` until a `Shutdown` command or a [`Stop`]
/// from `stops`; returns the exit code. [`serve`] wires `stops` to stdin
/// and the signals; tests drive it directly.
#[doc(hidden)]
pub async fn serve_until(
    socket: PathBuf,
    deps: EngineDeps,
    default_override: Option<BackendKind>,
    mut stops: mpsc::Receiver<Stop>,
) -> Result<ExitCode, DaemonError> {
    let (internal_tx, mut internal_rx) = mpsc::channel::<Internal>(256);
    let (worker_tx, worker_rx) = crossbeam_channel::unbounded::<Event>();
    {
        let tx = internal_tx.clone();
        tokio::spawn(async move {
            while let Some(stop) = stops.recv().await {
                if tx.send(Internal::Stop(stop)).await.is_err() {
                    break;
                }
            }
        });
    }
    let server = EngineServer::bind(&socket).await?;
    println!("FTHRUST_ENGINE_READY {}", socket.display());
    {
        let tx = internal_tx.clone();
        std::thread::spawn(move || {
            for e in worker_rx {
                if tx.blocking_send(Internal::Worker(e)).is_err() {
                    break;
                }
            }
        });
    }
    let mut service = Service {
        configuring: None,
        deps: Some(deps),
        default_override,
        engine: None,
        save: None,
        record: None,
        startup_events: Vec::new(),
        last_save: None,
        bytes_written: 0,
        dirty: false,
        saves_submitted: 0,
        save_results: 0,
        internal_tx,
        worker_tx,
    };
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut client: Option<Connection> = None;
    let code = loop {
        let events = client.as_ref().map(|c| c.events.clone());
        let events = events.as_ref();
        tokio::select! {
            accepted = server.accept(), if client.is_none() => {
                match accepted {
                    Ok(conn) => {
                        let hello = Event::Hello {
                            protocol_version: PROTOCOL_VERSION,
                            pid: std::process::id(),
                            version: crate::VERSION.to_string(),
                        };
                        let _ = conn.events.send(hello).await;
                        for e in &service.startup_events {
                            let _ = conn.events.send(e.clone()).await;
                        }
                        let _ = conn.events.send(Event::Status(Box::new(service.status()))).await;
                        client = Some(conn);
                    }
                    Err(e) => tracing::warn!("accept failed: {e}"),
                }
            }
            command = async { match client.as_mut() { Some(c) => c.commands.recv().await, None => std::future::pending().await } } => {
                match command {
                    Some(cmd) => {
                        if let Some(code) = service.handle(cmd, events) {
                            break code;
                        }
                    }
                    None => {
                        tracing::info!("client disconnected");
                        client = None;
                    }
                }
            }
            joined = async { match service.configuring.as_mut() { Some(j) => j.await, None => std::future::pending().await } } => {
                service.configuring = None;
                let exit = match joined {
                    Ok(outcome) => service.finish_configure(outcome, events),
                    Err(e) => service.configure_failed(e, events),
                };
                if let Some(code) = exit {
                    break code;
                }
            }
            internal = internal_rx.recv() => {
                match internal {
                    Some(Internal::Notice(EngineNotice::StatusChanged)) => service.dirty = true,
                    Some(Internal::Notice(EngineNotice::Recovering { .. }))
                    | Some(Internal::Notice(EngineNotice::Failed { .. }))
                    | Some(Internal::Notice(EngineNotice::GenerationStarted { .. })) => service.dirty = true,
                    Some(Internal::Worker(e)) => {
                        service.on_worker(&e);
                        emit(events, e);
                    }
                    Some(Internal::Stop(Stop::ParentGone)) => {
                        tracing::info!("parent closed our stdin; shutting down");
                        break ExitCode::SUCCESS;
                    }
                    Some(Internal::Stop(Stop::Signal)) => {
                        tracing::info!("signal received; shutting down");
                        break ExitCode::SUCCESS;
                    }
                    None => break ExitCode::SUCCESS,
                }
            }
            _ = heartbeat.tick() => {
                if client.is_some() {
                    let e = if service.dirty {
                        Event::Status(Box::new(service.status()))
                    } else {
                        Event::Heartbeat
                    };
                    service.dirty = false;
                    emit(events, e);
                }
            }
        }
    };
    // No new client is served from here on: close the listener (and remove
    // the socket file) now, so a relaunched UI's new engine can bind while
    // this one finishes its save. The connected client keeps its own socket.
    drop(server);
    shut_down(service, client.as_ref(), &mut internal_rx).await;
    if let Some(conn) = client.take() {
        flush(conn).await;
    }
    Ok(code)
}

/// The bounded shutdown: the save in flight, then capture, audio and a
/// recording, each with its own limit. A signal during either wait cuts it
/// short.
async fn shut_down(
    mut service: Service,
    client: Option<&Connection>,
    internal_rx: &mut mpsc::Receiver<Internal>,
) {
    // A save in flight finishes first (bounded like the UI's completion
    // timeout), and its result still reaches the client. The wait is for
    // the result itself: the save thread clears its busy flag right after
    // sending it, while the event may still be crossing the forwarder.
    let deadline = Instant::now() + SHUTDOWN_SAVE_WAIT;
    while service.save_pending() {
        match tokio::time::timeout_at(deadline, internal_rx.recv()).await {
            Ok(Some(Internal::Worker(e))) => {
                service.on_worker(&e);
                deliver(client, e, deadline).await;
            }
            Ok(Some(Internal::Stop(Stop::Signal))) => {
                tracing::warn!("signal received again; not waiting for the clip save");
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {
                tracing::warn!(
                    "the clip save did not finish within {SHUTDOWN_SAVE_WAIT:?}; shutting down without it"
                );
                break;
            }
        }
    }
    // Capture, audio and a recording stop on a blocking thread, so a stuck
    // driver or disk cannot keep the process alive; a recording's last
    // events still go out. A configure job still running is let finish so
    // its engine is shut down too.
    let deadline = Instant::now() + SHUTDOWN_STOP_WAIT;
    let stopping = async move {
        if let Some(job) = service.configuring.take()
            && let Ok(outcome) = job.await
        {
            service.engine = Some(outcome.engine);
        }
        let _ = tokio::task::spawn_blocking(move || service.shutdown()).await;
    };
    tokio::pin!(stopping);
    loop {
        tokio::select! {
            () = &mut stopping => break,
            internal = internal_rx.recv() => match internal {
                Some(Internal::Worker(e)) => deliver(client, e, deadline).await,
                Some(Internal::Stop(Stop::Signal)) => {
                    tracing::warn!("signal received again; not waiting for capture to stop");
                    return;
                }
                Some(_) => {}
                None => break,
            },
            () = tokio::time::sleep_until(deadline) => {
                tracing::warn!("capture did not stop within {SHUTDOWN_STOP_WAIT:?}; exiting anyway");
                return;
            }
        }
    }
    // Events the workers sent while stopping may still be crossing the
    // forwarder.
    let deadline = Instant::now() + Duration::from_millis(100);
    while let Ok(Some(internal)) = tokio::time::timeout_at(deadline, internal_rx.recv()).await {
        if let Internal::Worker(e) = internal {
            deliver(client, e, deadline).await;
        }
    }
}

/// Lets the connection's writer send everything queued, bounded.
async fn flush(conn: Connection) {
    let Connection {
        commands,
        events,
        mut closed,
    } = conn;
    drop(events);
    let _ = tokio::time::timeout(SHUTDOWN_FLUSH_WAIT, async {
        while !*closed.borrow_and_update() {
            if closed.changed().await.is_err() {
                break;
            }
        }
    })
    .await;
    drop(commands);
}

/// Builds the production dependencies.
pub fn production_deps(paths: Option<fthrust_core::FthrPaths>) -> EngineDeps {
    EngineDeps {
        backends: Arc::new(crate::backend::RealFactory::new(paths)),
        audio: Arc::new(crate::audio::pipewire::PipeWireAudio::new()),
        backoff: crate::engine::real_backoff(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_signal_is_forwarded_not_just_the_first() {
        // SIGTERM used to be honoured once: the listener ended after the
        // first, so a second one during a slow shutdown did nothing at all.
        let (tx, mut rx) = mpsc::channel(8);
        forward_signals(&[SignalKind::user_defined1()], &tx);
        for round in 0..3 {
            nix::sys::signal::raise(nix::sys::signal::Signal::SIGUSR1).unwrap();
            let got = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
            assert_eq!(got, Ok(Some(Stop::Signal)), "signal {round}");
        }
    }
}
