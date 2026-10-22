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
use tokio::sync::mpsc;

use crate::engine::{CaptureEngine, EngineDeps, EngineNotice};
use crate::save::{RecordWorker, SaveRequest, SaveWorker};

/// Heartbeat period.
pub const HEARTBEAT: Duration = Duration::from_millis(500);

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

enum Internal {
    Notice(EngineNotice),
    Worker(Event),
    ParentGone,
    Signal,
}

struct Service {
    deps: Option<EngineDeps>,
    default_override: Option<BackendKind>,
    engine: Option<CaptureEngine>,
    save: Option<SaveWorker>,
    record: Option<RecordWorker>,
    startup_events: Vec<Event>,
    last_save: Option<(u64, Result<SavedClip, String>)>,
    bytes_written: u64,
    dirty: bool,
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

    /// Applies a command; returns `Some(code)` when the process must exit.
    fn handle(&mut self, command: Command, events: &mpsc::Sender<Event>) -> Option<ExitCode> {
        match command {
            Command::Configure(config) => {
                let mut config = config;
                if config.backend_override.is_none() {
                    config.backend_override = self.default_override;
                }
                match self.engine.as_mut() {
                    None => {
                        let deps = self.deps.take()?;
                        let mut engine = CaptureEngine::new(config, deps);
                        let warnings = engine.config().normalized().1;
                        match engine.start() {
                            Ok(notices) => {
                                for n in notices {
                                    let e = Event::StartupWarning {
                                        code: n.code,
                                        detail: n.detail,
                                    };
                                    self.startup_events.push(e.clone());
                                    let _ = events.try_send(e);
                                }
                                let handle = engine.handle();
                                self.save =
                                    Some(SaveWorker::spawn(handle.clone(), self.worker_tx.clone()));
                                self.record =
                                    Some(RecordWorker::new(handle, self.worker_tx.clone()));
                                let notices = engine.events();
                                let tx = self.internal_tx.clone();
                                std::thread::spawn(move || {
                                    for n in notices {
                                        if tx.blocking_send(Internal::Notice(n)).is_err() {
                                            break;
                                        }
                                    }
                                });
                                self.engine = Some(engine);
                                let _ = events.try_send(Event::Configured { warnings });
                                self.dirty = true;
                            }
                            Err(e) => {
                                let event = Event::StartupError {
                                    code: e.code.to_string(),
                                    detail: e.detail,
                                };
                                self.startup_events.push(event.clone());
                                let _ = events.try_send(event);
                                self.engine = Some(engine);
                                return Some(ExitCode::from(1));
                            }
                        }
                    }
                    Some(engine) => match engine.reconfigure(config) {
                        Ok(warnings) => {
                            let _ = events.try_send(Event::Configured { warnings });
                            self.dirty = true;
                        }
                        Err(e) => {
                            let _ = events.try_send(Event::StartupError {
                                code: e.code.to_string(),
                                detail: e.detail,
                            });
                            self.dirty = true;
                        }
                    },
                }
            }
            Command::SaveClip {
                request_id,
                path,
                seconds,
            } => match &self.save {
                Some(save) => {
                    self.bytes_written = 0;
                    save.submit(SaveRequest {
                        request_id,
                        path,
                        seconds,
                    })
                }
                None => {
                    let _ = events.try_send(Event::SaveResult {
                        request_id,
                        result: Err("Capture is not running".into()),
                    });
                }
            },
            Command::StartRecording { request_id, path } => match &self.record {
                Some(r) => r.start(request_id, path),
                None => {
                    let _ = events.try_send(Event::RecordingError {
                        request_id,
                        message: "Capture is not running".into(),
                    });
                }
            },
            Command::StopRecording { request_id } => match &self.record {
                Some(r) => r.stop(request_id),
                None => {
                    let _ = events.try_send(Event::RecordingError {
                        request_id,
                        message: "No recording is in progress".into(),
                    });
                }
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
                let _ = events.try_send(Event::Status(Box::new(self.status())));
                self.dirty = false;
            }
            Command::Shutdown => return Some(ExitCode::SUCCESS),
        }
        None
    }

    fn on_worker(&mut self, event: &Event) {
        match event {
            Event::SaveResult { request_id, result } => {
                self.last_save = Some((*request_id, result.clone()));
            }
            Event::SaveProgress { bytes_written, .. } => self.bytes_written = *bytes_written,
            _ => {}
        }
        self.dirty = true;
    }

    fn shutdown(&mut self) {
        if let Some(r) = &self.record
            && r.state() != RecordingState::Idle
        {
            r.stop(0);
        }
        if let Some(e) = self.engine.as_mut() {
            e.shutdown();
        }
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
    runtime.block_on(serve_async(socket, deps, default_override))
}

async fn serve_async(
    socket: PathBuf,
    deps: EngineDeps,
    default_override: Option<BackendKind>,
) -> Result<ExitCode, DaemonError> {
    let (internal_tx, mut internal_rx) = mpsc::channel::<Internal>(256);
    let (worker_tx, worker_rx) = crossbeam_channel::unbounded::<Event>();
    {
        // Parent watch: the UI holds our stdin; EOF means it is gone.
        let tx = internal_tx.clone();
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
            let _ = tx.blocking_send(Internal::ParentGone);
        });
    }
    {
        let tx = internal_tx.clone();
        tokio::spawn(async move {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
            let mut int =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok();
            tokio::select! {
                _ = async { if let Some(t) = term.as_mut() { t.recv().await; } else { std::future::pending::<()>().await } } => {}
                _ = async { if let Some(i) = int.as_mut() { i.recv().await; } else { std::future::pending::<()>().await } } => {}
            }
            let _ = tx.send(Internal::Signal).await;
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
        deps: Some(deps),
        default_override,
        engine: None,
        save: None,
        record: None,
        startup_events: Vec::new(),
        last_save: None,
        bytes_written: 0,
        dirty: false,
        internal_tx,
        worker_tx,
    };
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut client: Option<Connection> = None;
    let (dummy_tx, _dummy_rx) = mpsc::channel::<Event>(1);
    let code = loop {
        let events = client
            .as_ref()
            .map(|c| c.events.clone())
            .unwrap_or_else(|| dummy_tx.clone());
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
                        if let Some(code) = service.handle(cmd, &events) {
                            break code;
                        }
                    }
                    None => {
                        tracing::info!("client disconnected");
                        client = None;
                    }
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
                        let _ = events.try_send(e);
                    }
                    Some(Internal::ParentGone) => {
                        tracing::info!("parent closed our stdin; shutting down");
                        break ExitCode::SUCCESS;
                    }
                    Some(Internal::Signal) => {
                        tracing::info!("signal received; shutting down");
                        break ExitCode::SUCCESS;
                    }
                    None => break ExitCode::SUCCESS,
                }
            }
            _ = heartbeat.tick() => {
                if client.is_some() {
                    let e = if service.dirty { Event::Status(Box::new(service.status())) } else { Event::Heartbeat };
                    service.dirty = false;
                    let _ = events.try_send(e);
                }
            }
        }
    };
    service.shutdown();
    // Let the writer task flush the last events before the process exits.
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(client);
    drop(server);
    Ok(code)
}

/// Builds the production dependencies.
pub fn production_deps(paths: Option<fthrust_core::FthrPaths>) -> EngineDeps {
    EngineDeps {
        backends: Arc::new(crate::backend::RealFactory::new(paths)),
        audio: Arc::new(crate::audio::pipewire::PipeWireAudio::new()),
        backoff: crate::engine::real_backoff(),
    }
}
