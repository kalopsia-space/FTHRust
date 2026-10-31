//! The daemon's shutdown paths, run in-process with injected stops so a
//! save can be made as slow as a test needs.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fthrust_core::capture_config::{CaptureConfig, Codec, EncoderPref};
use fthrust_core::capture_health::flags;
use fthrust_engine::audio::{AudioConfig, AudioProvider, AudioSourceKind, SyntheticAudio};
use fthrust_engine::backend::{KindScript, SyntheticFactory, SyntheticScript};
use fthrust_engine::daemon::{CONFIGURE_FAILED, Stop, serve_until};
use fthrust_engine::engine::EngineDeps;
use fthrust_ipc::transport::EngineClient;
use fthrust_ipc::{AudioStatus, BackendKind, Command, EngineConfig, Event};
use tokio::sync::mpsc;

fn have_software_h264() -> bool {
    fthrust_media::init().unwrap();
    ["libopenh264", "libx264"]
        .iter()
        .any(|n| ffmpeg_next::encoder::find_by_name(n).is_some())
}

/// Synthetic audio whose window extraction sleeps, so a save takes as long
/// as a test needs.
struct SlowAudio {
    inner: SyntheticAudio,
    delay: Duration,
}

impl AudioProvider for SlowAudio {
    fn start(&self, cfg: &AudioConfig) -> Vec<(String, String)> {
        self.inner.start(cfg)
    }
    fn window(&self, kind: AudioSourceKind, end_ns: i64, duration_ms: u32) -> Option<Vec<f32>> {
        std::thread::sleep(self.delay);
        self.inner.window(kind, end_ns, duration_ms)
    }
    fn status(&self) -> AudioStatus {
        self.inner.status()
    }
    fn stop(&self) {
        self.inner.stop()
    }
}

/// Audio whose start panics, taking the configure job down with it.
struct PanickingAudio;

impl AudioProvider for PanickingAudio {
    fn start(&self, _cfg: &AudioConfig) -> Vec<(String, String)> {
        panic!("injected audio start failure");
    }
    fn window(&self, _: AudioSourceKind, _: i64, _: u32) -> Option<Vec<f32>> {
        None
    }
    fn status(&self) -> AudioStatus {
        SyntheticAudio::new().status()
    }
    fn stop(&self) {}
}

struct Daemon {
    socket: PathBuf,
    stops: mpsc::Sender<Stop>,
    done: std::sync::mpsc::Receiver<Result<ExitCode, String>>,
    tmp: tempfile::TempDir,
}

fn start(audio: Arc<dyn AudioProvider>) -> Daemon {
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("run").join("engine.sock");
    let mut script = SyntheticScript::new(320, 180);
    script.frame_interval = Duration::from_millis(16);
    script
        .kinds
        .insert(BackendKind::WlrScreencopy, KindScript::default());
    let deps = EngineDeps {
        backends: Arc::new(SyntheticFactory {
            script: Arc::new(Mutex::new(script)),
            candidates: vec![BackendKind::WlrScreencopy],
        }),
        audio,
        backoff: Arc::new(|_, running: &dyn Fn() -> bool| running()),
    };
    let (stops, stop_rx) = mpsc::channel(8);
    let (done_tx, done) = std::sync::mpsc::channel();
    let path = socket.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let served = rt.block_on(serve_until(path, deps, None, stop_rx));
        rt.shutdown_timeout(Duration::from_secs(1));
        let _ = done_tx.send(served.map_err(|e| e.to_string()));
    });
    Daemon {
        socket,
        stops,
        done,
        tmp,
    }
}

fn config() -> EngineConfig {
    EngineConfig {
        capture: CaptureConfig {
            fps: 30,
            buffer_seconds: 10,
            bitrate_kbps: 4000,
            codec: Codec::H264,
            encoder: EncoderPref::Software,
            preset: 1,
            audio_enabled: true,
            microphone_endpoint_id: "mic".into(),
            ..CaptureConfig::default()
        },
        max_buffer_mb: 0,
        backend_override: None,
    }
}

async fn connect(socket: &Path) -> EngineClient {
    let started = Instant::now();
    loop {
        match EngineClient::connect(socket).await {
            Ok(c) => return c,
            Err(e) if started.elapsed() < Duration::from_secs(10) => {
                let _ = e;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("connect: {e}"),
        }
    }
}

async fn next_event(
    client: &mut EngineClient,
    timeout: Duration,
    mut pred: impl FnMut(&Event) -> bool,
) -> Event {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, client.events().recv()).await {
            Ok(Some(e)) if pred(&e) => return e,
            Ok(Some(_)) => {}
            Ok(None) => panic!("engine hung up"),
            Err(_) => panic!("no matching event within {timeout:?}"),
        }
    }
}

/// Configures capture and waits until the ring holds a few seconds.
async fn capture_running(client: &mut EngineClient) {
    client.send(Command::Configure(config())).await.unwrap();
    next_event(client, Duration::from_secs(10), |e| {
        matches!(e, Event::Configured { .. })
    })
    .await;
    next_event(client, Duration::from_secs(10), |e| {
        matches!(e, Event::Status(s) if s.health_flags & flags::ACTIVE != 0 && s.frames_captured >= 90)
    })
    .await;
}

/// Waits (off the async runtime) for the service to return.
async fn finished(d: &Daemon, within: Duration) -> Result<ExitCode, String> {
    let started = Instant::now();
    loop {
        match d.done.try_recv() {
            Ok(r) => return r,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(
                    started.elapsed() < within,
                    "the engine did not shut down within {within:?}"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("service thread: {e}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ui_dying_during_a_save_does_not_hang_the_shutdown() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // With no client, events went to a one-slot channel nobody read, and the
    // shutdown's save wait awaited `send` on it: the save's second event
    // blocked the engine forever, still capturing.
    let d = start(Arc::new(SlowAudio {
        inner: SyntheticAudio::new(),
        delay: Duration::from_millis(1500),
    }));
    let mut client = connect(&d.socket).await;
    capture_running(&mut client).await;
    client
        .send(Command::SaveClip {
            request_id: 1,
            path: d.tmp.path().join("clip.mp4"),
            seconds: 0,
        })
        .await
        .unwrap();
    next_event(&mut client, Duration::from_secs(10), |e| {
        matches!(e, Event::SaveStarted { request_id: 1 })
    })
    .await;
    drop(client);
    // Let the engine see the hang-up before the parent goes.
    tokio::time::sleep(Duration::from_millis(300)).await;
    d.stops.send(Stop::ParentGone).await.unwrap();
    let code = finished(&d, Duration::from_secs(30)).await.unwrap();
    assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
    assert!(
        d.tmp.path().join("clip.mp4").exists(),
        "the save in flight still finished"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_configure_is_reported_and_ends_the_engine() {
    // The configure job runs on a blocking thread; when it panicked, the
    // JoinError was dropped along with the engine inside it, nothing was
    // reported, and every later Configure was silently ignored.
    let d = start(Arc::new(PanickingAudio));
    let mut client = connect(&d.socket).await;
    client.send(Command::Configure(config())).await.unwrap();
    let e = next_event(&mut client, Duration::from_secs(10), |e| {
        matches!(e, Event::StartupError { .. } | Event::Configured { .. })
    })
    .await;
    let Event::StartupError { code, detail } = e else {
        panic!("{e:?}")
    };
    assert_eq!(code, CONFIGURE_FAILED);
    assert!(detail.contains("injected audio start failure"), "{detail}");
    let code = finished(&d, Duration::from_secs(10)).await.unwrap();
    assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(1)));
}
