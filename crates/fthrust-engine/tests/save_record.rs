use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fthrust_core::capture_config::{CaptureConfig, Codec, EncoderPref};
use fthrust_core::capture_health::flags;
use fthrust_engine::audio::SyntheticAudio;
use fthrust_engine::backend::{KindScript, SyntheticFactory, SyntheticScript};
use fthrust_engine::engine::{CaptureEngine, EngineDeps};
use fthrust_engine::save::{RecordWorker, SaveRequest, SaveWorker, admission};
use fthrust_ipc::{BackendKind, EngineConfig, Event, RecordingState, StatusSnapshot};

fn have_software_h264() -> bool {
    fthrust_media::init().unwrap();
    ["libopenh264", "libx264"]
        .iter()
        .any(|n| ffmpeg_next::encoder::find_by_name(n).is_some())
}

fn config(f: impl FnOnce(&mut CaptureConfig)) -> EngineConfig {
    let mut capture = CaptureConfig {
        fps: 30,
        buffer_seconds: 10,
        bitrate_kbps: 2000,
        codec: Codec::H264,
        encoder: EncoderPref::Software,
        preset: 1,
        audio_enabled: true,
        microphone_endpoint_id: "mic".into(),
        ..CaptureConfig::default()
    };
    f(&mut capture);
    EngineConfig {
        capture,
        max_buffer_mb: 0,
        backend_override: None,
        automatic_start: false,
    }
}

struct Rig {
    engine: CaptureEngine,
    save: SaveWorker,
    record: RecordWorker,
    events: crossbeam_channel::Receiver<Event>,
    events_tx: crossbeam_channel::Sender<Event>,
    script: Arc<Mutex<SyntheticScript>>,
}

/// An audio provider whose window extraction takes a while, to make a save
/// observably long.
struct SlowAudio(SyntheticAudio);

impl fthrust_engine::audio::AudioProvider for SlowAudio {
    fn start(&self, cfg: &fthrust_engine::audio::AudioConfig) -> Vec<(String, String)> {
        self.0.start(cfg)
    }
    fn window(
        &self,
        kind: fthrust_engine::audio::AudioSourceKind,
        end_ns: i64,
        duration_ms: u32,
    ) -> Option<Vec<f32>> {
        std::thread::sleep(Duration::from_millis(400));
        self.0.window(kind, end_ns, duration_ms)
    }
    fn status(&self) -> fthrust_ipc::AudioStatus {
        self.0.status()
    }
    fn stop(&self) {
        self.0.stop()
    }
}

/// Synthetic audio whose first window extraction panics.
struct PanicOnceAudio {
    inner: SyntheticAudio,
    armed: std::sync::atomic::AtomicBool,
}

impl fthrust_engine::audio::AudioProvider for PanicOnceAudio {
    fn start(&self, cfg: &fthrust_engine::audio::AudioConfig) -> Vec<(String, String)> {
        self.inner.start(cfg)
    }
    fn window(
        &self,
        kind: fthrust_engine::audio::AudioSourceKind,
        end_ns: i64,
        duration_ms: u32,
    ) -> Option<Vec<f32>> {
        if self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            panic!("injected window failure");
        }
        self.inner.window(kind, end_ns, duration_ms)
    }
    fn status(&self) -> fthrust_ipc::AudioStatus {
        self.inner.status()
    }
    fn stop(&self) {
        self.inner.stop()
    }
}

fn rig(cfg: EngineConfig, audio: SyntheticAudio, interval: Duration) -> Rig {
    rig_with(cfg, Arc::new(audio), interval)
}

fn rig_with(
    cfg: EngineConfig,
    audio: Arc<dyn fthrust_engine::audio::AudioProvider>,
    interval: Duration,
) -> Rig {
    let mut script = SyntheticScript::new(320, 180);
    script.frame_interval = interval;
    script
        .kinds
        .insert(BackendKind::WlrScreencopy, KindScript::default());
    let script = Arc::new(Mutex::new(script));
    let deps = EngineDeps {
        backends: Arc::new(SyntheticFactory {
            script: script.clone(),
            candidates: vec![BackendKind::WlrScreencopy],
        }),
        audio,
        backoff: Arc::new(|_, running: &dyn Fn() -> bool| running()),
    };
    let engine = CaptureEngine::new(cfg, deps);
    let (tx, rx) = crossbeam_channel::unbounded();
    let save = SaveWorker::spawn(engine.handle(), tx.clone());
    let record = RecordWorker::new(engine.handle(), tx.clone());
    Rig {
        engine,
        save,
        record,
        events: rx,
        events_tx: tx,
        script,
    }
}

fn next_event(
    rx: &crossbeam_channel::Receiver<Event>,
    timeout: Duration,
    mut pred: impl FnMut(&Event) -> bool,
) -> Event {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(e) if pred(&e) => return e,
            Ok(_) => {}
            Err(_) => panic!("no matching event within {timeout:?}"),
        }
    }
}

fn wait_frames(engine: &CaptureEngine, n: u64) {
    let started = Instant::now();
    while engine.status().frames_captured < n {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "frames {}",
            engine.status().frames_captured
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn admission_messages_are_the_references() {
    let mut s = StatusSnapshot {
        health_flags: flags::ACTIVE,
        ..fthrust_ipc::StatusSnapshot {
            frames_captured: 0,
            bytes_written: 0,
            encoder_open: true,
            active_codec: String::new(),
            health_flags: 0,
            capture_generation: 1,
            encoder_generation: 1,
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
        }
    };
    assert_eq!(admission(&s, true, true), Ok(()));
    assert_eq!(
        admission(&s, false, true),
        Err("Capture restarted; the replay buffer is still rebuilding")
    );
    assert_eq!(
        admission(&s, true, false),
        Err("Nothing to save: the replay buffer contains no video packets")
    );
    s.paused = true;
    assert_eq!(
        admission(&s, true, true),
        Err("Capture is not receiving new frames; restart capture before saving")
    );
    s.paused = false;
    for f in [
        flags::RECOVERING,
        flags::BACKEND_FAILED,
        flags::ACTIVE | flags::PAUSED,
    ] {
        s.health_flags = f;
        assert_eq!(
            admission(&s, true, true),
            Err("Capture is not receiving new frames; restart capture before saving")
        );
    }
}

#[test]
fn save_before_first_keyframe_is_refused() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // The synthetic backend delivers its first frame as soon as capture
    // starts, so the save races that frame's packet into the ring; on a busy
    // machine the frame can win. One attempt that finds the ring empty shows
    // the refusal.
    for attempt in 1..=5 {
        let mut r = rig(
            config(|_| {}),
            SyntheticAudio::new(),
            Duration::from_millis(700),
        );
        r.engine.start().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("early.mp4");
        r.save.submit(SaveRequest {
            request_id: 1,
            path: path.clone(),
            seconds: 5,
        });
        let e = next_event(&r.events, Duration::from_secs(5), |e| {
            matches!(e, Event::SaveResult { request_id: 1, .. })
        });
        r.engine.shutdown();
        let _ = &r.script;
        match e {
            Event::SaveResult {
                result: Err(msg), ..
            } => {
                // The reference's wording for an empty ring; "still
                // rebuilding" is reserved for a ring holding only an older
                // generation.
                assert_eq!(
                    msg,
                    "Nothing to save: the replay buffer contains no video packets"
                );
                assert!(!path.exists());
                return;
            }
            Event::SaveResult { result: Ok(_), .. } => {
                eprintln!("attempt {attempt}: the first frame reached the ring first");
            }
            other => panic!("{other:?}"),
        }
    }
    panic!("the first frame always reached the ring before the save");
}

#[test]
fn save_writes_requested_seconds_with_two_tracks() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut r = rig(
        config(|_| {}),
        SyntheticAudio::new(),
        Duration::from_millis(16),
    );
    r.engine.start().unwrap();
    std::thread::sleep(Duration::from_secs(6));
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("clip.mp4");
    r.save.submit(SaveRequest {
        request_id: 7,
        path: path.clone(),
        seconds: 3,
    });
    let started = next_event(&r.events, Duration::from_secs(5), |e| {
        matches!(e, Event::SaveStarted { request_id: 7 })
    });
    assert_eq!(started, Event::SaveStarted { request_id: 7 });
    let done = next_event(&r.events, Duration::from_secs(20), |e| {
        matches!(e, Event::SaveResult { request_id: 7, .. })
    });
    let Event::SaveResult {
        result: Ok(clip), ..
    } = done
    else {
        panic!("{done:?}")
    };
    assert_eq!(clip.path, path);
    assert!(clip.audio_included);
    assert_eq!(clip.audio_tracks, ["Default Mix", "Microphone"]);
    assert_eq!(clip.audio_error, None);
    let meta = fthrust_media::probe::probe_video_metadata(&path).unwrap();
    let d = meta.duration_seconds.unwrap();
    assert!((d - 3.0).abs() <= 1.0 / 30.0 + 0.05, "duration {d}");
    assert_eq!(
        fthrust_media::combined_audio::descriptors_from_file(&path)
            .unwrap()
            .len(),
        2
    );
    r.engine.shutdown();
}

#[test]
fn commands_keep_flowing_during_save() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut r = rig_with(
        config(|c| {
            c.bitrate_kbps = 20_000;
            c.microphone_endpoint_id.clear();
        }),
        Arc::new(SlowAudio(SyntheticAudio::new())),
        Duration::from_millis(2),
    );
    r.engine.start().unwrap();
    wait_frames(&r.engine, 30 * 8);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("big.mp4");
    r.save.submit(SaveRequest {
        request_id: 2,
        path,
        seconds: 8,
    });
    next_event(&r.events, Duration::from_secs(5), |e| {
        matches!(e, Event::SaveStarted { request_id: 2 })
    });
    let frames_at_start = r.engine.status().frames_captured;
    let mut worst = Duration::ZERO;
    loop {
        let t = Instant::now();
        let _ = r.engine.status();
        worst = worst.max(t.elapsed());
        match r.events.try_recv() {
            Ok(Event::SaveResult {
                request_id: 2,
                result,
            }) => {
                result.unwrap();
                break;
            }
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    assert!(
        worst < Duration::from_millis(50),
        "status blocked for {worst:?}"
    );
    assert!(
        r.engine.status().frames_captured > frames_at_start,
        "capture continued during the save"
    );
    r.engine.shutdown();
}

#[test]
fn save_while_paused_or_busy_is_refused() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut r = rig(
        config(|_| {}),
        SyntheticAudio::new(),
        Duration::from_millis(16),
    );
    r.engine.start().unwrap();
    wait_frames(&r.engine, 10);
    r.engine.set_paused(true);
    std::thread::sleep(Duration::from_millis(200));
    let tmp = tempfile::tempdir().unwrap();
    r.save.submit(SaveRequest {
        request_id: 3,
        path: tmp.path().join("p.mp4"),
        seconds: 1,
    });
    match next_event(&r.events, Duration::from_secs(5), |e| {
        matches!(e, Event::SaveResult { request_id: 3, .. })
    }) {
        Event::SaveResult {
            result: Err(msg), ..
        } => {
            assert_eq!(
                msg,
                "Capture is not receiving new frames; restart capture before saving"
            )
        }
        other => panic!("{other:?}"),
    }
    r.engine.set_paused(false);
    std::thread::sleep(Duration::from_secs(2));
    r.save.submit(SaveRequest {
        request_id: 4,
        path: tmp.path().join("a.mp4"),
        seconds: 1,
    });
    r.save.submit(SaveRequest {
        request_id: 5,
        path: tmp.path().join("b.mp4"),
        seconds: 1,
    });
    let mut results = Vec::new();
    for _ in 0..2 {
        let e = next_event(&r.events, Duration::from_secs(20), |e| {
            matches!(e, Event::SaveResult { .. })
        });
        if let Event::SaveResult { request_id, result } = e {
            results.push((request_id, result.is_ok(), result.err()));
        }
    }
    results.sort_by_key(|r| r.0);
    assert!(results[0].1, "{results:?}");
    assert_eq!(
        results[1].2.as_deref(),
        Some("A clip save is already in progress")
    );
    r.engine.shutdown();
}

#[test]
fn video_only_when_audio_unavailable() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut r = rig(
        config(|c| c.microphone_endpoint_id.clear()),
        SyntheticAudio::unavailable(),
        Duration::from_millis(16),
    );
    let notices = r.engine.start().unwrap();
    assert_eq!(notices[0].code, "DESKTOP_AUDIO_UNAVAILABLE");
    std::thread::sleep(Duration::from_secs(3));
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("silent.mp4");
    r.save.submit(SaveRequest {
        request_id: 9,
        path: path.clone(),
        seconds: 2,
    });
    let done = next_event(&r.events, Duration::from_secs(20), |e| {
        matches!(e, Event::SaveResult { request_id: 9, .. })
    });
    let Event::SaveResult {
        result: Ok(clip), ..
    } = done
    else {
        panic!("{done:?}")
    };
    assert!(!clip.audio_included);
    assert!(
        clip.audio_error
            .as_deref()
            .is_some_and(|e| e.contains("Default Mix")),
        "{:?}",
        clip.audio_error
    );
    assert!(path.exists());
    r.engine.shutdown();
}

#[test]
fn record_start_stop_produces_fragmented_mp4_and_refuses_existing() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut r = rig(
        config(|c| c.microphone_endpoint_id.clear()),
        SyntheticAudio::new(),
        Duration::from_millis(16),
    );
    r.engine.start().unwrap();
    wait_frames(&r.engine, 5);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rec.mp4");
    r.record.start(11, path.clone());
    next_event(&r.events, Duration::from_secs(5), |e| {
        matches!(e, Event::RecordingStarted { request_id: 11, .. })
    });
    assert!(matches!(r.record.state(), RecordingState::Recording { .. }));
    std::thread::sleep(Duration::from_secs(2));
    r.record.stop(12);
    let e = next_event(&r.events, Duration::from_secs(10), |e| {
        matches!(
            e,
            Event::RecordingStopped { .. } | Event::RecordingError { .. }
        )
    });
    let Event::RecordingStopped {
        request_id,
        path: p,
        bytes,
    } = e
    else {
        panic!("{e:?}")
    };
    assert_eq!((request_id, p), (12, path.clone()));
    assert_eq!(bytes, std::fs::metadata(&path).unwrap().len());
    assert_eq!(r.record.state(), RecordingState::Idle);
    let meta = fthrust_media::probe::probe_video_metadata(&path).unwrap();
    assert!(meta.duration_seconds.unwrap() >= 1.5);
    assert_eq!(
        fthrust_media::combined_audio::descriptors_from_file(&path)
            .unwrap()
            .len(),
        1,
        "system audio track"
    );
    r.record.start(13, path.clone());
    let e = next_event(&r.events, Duration::from_secs(5), |e| {
        matches!(e, Event::RecordingError { request_id: 13, .. })
    });
    let Event::RecordingError { message, .. } = e else {
        unreachable!()
    };
    assert!(message.contains("exists"), "{message}");
    r.engine.shutdown();
}

#[test]
fn record_backlog_overflow_ends_the_recording_with_an_error() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // A slow audio provider stalls the recorder; with a one-packet backlog the
    // tap overflows and the recording must end with RecordingError instead of
    // silently dropping packets.
    let r = rig_with(
        config(|c| c.microphone_endpoint_id.clear()),
        Arc::new(SlowAudio(SyntheticAudio::new())),
        Duration::from_millis(16),
    );
    let mut r = r;
    r.engine.start().unwrap();
    wait_frames(&r.engine, 5);
    let record = RecordWorker::with_backlog(r.engine.handle(), r.events_tx.clone(), 0);
    let tmp = tempfile::tempdir().unwrap();
    record.start(21, tmp.path().join("overflow.mp4"));
    next_event(&r.events, Duration::from_secs(5), |e| {
        matches!(e, Event::RecordingStarted { request_id: 21, .. })
    });
    let e = next_event(&r.events, Duration::from_secs(15), |e| {
        matches!(
            e,
            Event::RecordingError { .. } | Event::RecordingStopped { .. }
        )
    });
    let Event::RecordingError { message, .. } = e else {
        panic!("expected an error, got {e:?}")
    };
    assert!(message.contains("behind"), "{message}");
    assert_eq!(record.state(), RecordingState::Idle);
    r.engine.shutdown();
}

#[test]
fn a_panicking_save_is_answered_and_frees_the_worker() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // A panic in the save thread used to kill it with its busy flag set: no
    // SaveResult, and every later save refused as "already in progress".
    let mut r = rig_with(
        config(|c| c.microphone_endpoint_id.clear()),
        Arc::new(PanicOnceAudio {
            inner: SyntheticAudio::new(),
            armed: std::sync::atomic::AtomicBool::new(true),
        }),
        Duration::from_millis(16),
    );
    r.engine.start().unwrap();
    wait_frames(&r.engine, 60);
    let tmp = tempfile::tempdir().unwrap();
    let first = tmp.path().join("first.mp4");
    r.save.submit(SaveRequest {
        request_id: 31,
        path: first.clone(),
        seconds: 1,
    });
    match next_event(&r.events, Duration::from_secs(10), |e| {
        matches!(e, Event::SaveResult { request_id: 31, .. })
    }) {
        Event::SaveResult {
            result: Err(msg), ..
        } => assert!(msg.contains("injected window failure"), "{msg}"),
        other => panic!("{other:?}"),
    }
    // The worker clears its busy flag just after sending the result.
    let freed = Instant::now();
    while r.save.is_busy() {
        assert!(
            freed.elapsed() < Duration::from_secs(2),
            "the worker stays busy"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!first.exists());
    assert!(
        !fthrust_media::transactional_save::temporary_path_for(&first).exists(),
        "no partial file left behind"
    );
    let second = tmp.path().join("second.mp4");
    r.save.submit(SaveRequest {
        request_id: 32,
        path: second.clone(),
        seconds: 1,
    });
    match next_event(&r.events, Duration::from_secs(20), |e| {
        matches!(e, Event::SaveResult { request_id: 32, .. })
    }) {
        Event::SaveResult { result: Ok(_), .. } => {}
        other => panic!("{other:?}"),
    }
    assert!(second.exists());
    r.engine.shutdown();
}

/// Synthetic audio whose status takes a while, like PipeWire's per-source
/// locks when the capture callback or a long window holds them.
struct SlowStatusAudio {
    inner: SyntheticAudio,
    delay: Duration,
}

impl fthrust_engine::audio::AudioProvider for SlowStatusAudio {
    fn start(&self, cfg: &fthrust_engine::audio::AudioConfig) -> Vec<(String, String)> {
        self.inner.start(cfg)
    }
    fn window(
        &self,
        kind: fthrust_engine::audio::AudioSourceKind,
        end_ns: i64,
        duration_ms: u32,
    ) -> Option<Vec<f32>> {
        self.inner.window(kind, end_ns, duration_ms)
    }
    fn status(&self) -> fthrust_ipc::AudioStatus {
        std::thread::sleep(self.delay);
        self.inner.status()
    }
    fn stop(&self) {
        self.inner.stop()
    }
}

/// A desktop source whose blocks reach the ring `lag` after they were
/// captured, like a real capture stream's latency.
struct LaggyAudio {
    ring: Arc<Mutex<Option<fthrust_engine::audio::AudioRing>>>,
    running: Arc<std::sync::atomic::AtomicBool>,
    lag_ns: i64,
}

impl LaggyAudio {
    fn new(lag: Duration) -> Self {
        Self {
            ring: Arc::new(Mutex::new(None)),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            lag_ns: lag.as_nanos() as i64,
        }
    }
}

impl fthrust_engine::audio::AudioProvider for LaggyAudio {
    fn start(&self, cfg: &fthrust_engine::audio::AudioConfig) -> Vec<(String, String)> {
        use std::sync::atomic::Ordering;
        *self.ring.lock().unwrap() = cfg
            .system
            .as_ref()
            .map(|_| fthrust_engine::audio::AudioRing::new(cfg.keep_seconds));
        self.running.store(true, Ordering::SeqCst);
        let (ring, running, lag) = (self.ring.clone(), self.running.clone(), self.lag_ns);
        std::thread::spawn(move || {
            const BLOCK_NS: i64 = 10_000_000;
            let mut phase = 0u64;
            let mut next = fthrust_wayland::backend::monotonic_ns() - lag;
            while running.load(Ordering::SeqCst) {
                let delivered_up_to = fthrust_wayland::backend::monotonic_ns() - lag;
                while next + BLOCK_NS <= delivered_up_to {
                    let samples: Vec<f32> = (0..480u64)
                        .flat_map(|i| {
                            let t = (phase + i) as f32 / 48_000.0;
                            let v = (t * 440.0 * std::f32::consts::TAU).sin() * 0.3;
                            [v, v]
                        })
                        .collect();
                    phase += 480;
                    if let Some(r) = ring.lock().unwrap().as_mut() {
                        r.push(fthrust_media::audio_timing::AudioBlock {
                            start_ns: next,
                            samples,
                        });
                    }
                    next += BLOCK_NS;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        Vec::new()
    }
    fn window(
        &self,
        kind: fthrust_engine::audio::AudioSourceKind,
        end_ns: i64,
        duration_ms: u32,
    ) -> Option<Vec<f32>> {
        if kind != fthrust_engine::audio::AudioSourceKind::System {
            return None;
        }
        Some(
            self.ring
                .lock()
                .unwrap()
                .as_ref()?
                .window(end_ns, duration_ms),
        )
    }
    fn captured_until(&self, kind: fthrust_engine::audio::AudioSourceKind) -> Option<i64> {
        if kind != fthrust_engine::audio::AudioSourceKind::System {
            return None;
        }
        self.ring.lock().unwrap().as_ref()?.captured_until()
    }
    fn status(&self) -> fthrust_ipc::AudioStatus {
        let system = if self.ring.lock().unwrap().is_some() {
            fthrust_ipc::SourceStatus::Streaming
        } else {
            fthrust_ipc::SourceStatus::Disabled
        };
        fthrust_ipc::AudioStatus {
            system,
            microphone: fthrust_ipc::SourceStatus::Disabled,
        }
    }
    fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        *self.ring.lock().unwrap() = None;
    }
}

/// Stream durations in seconds: (video, every audio track).
fn stream_seconds(path: &std::path::Path) -> (f64, Vec<f64>) {
    let ictx = ffmpeg_next::format::input(path).unwrap();
    let seconds = |s: &ffmpeg_next::format::stream::Stream| {
        let tb = s.time_base();
        s.duration() as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator())
    };
    let mut video = 0.0;
    let mut audio = Vec::new();
    for s in ictx.streams() {
        match s.parameters().medium() {
            ffmpeg_next::media::Type::Video => video = seconds(&s),
            ffmpeg_next::media::Type::Audio => audio.push(seconds(&s)),
            _ => {}
        }
    }
    (video, audio)
}

fn record_for(r: &Rig, id: u64, path: &std::path::Path, seconds: Duration) -> Event {
    r.record.start(id, path.to_path_buf());
    next_event(
        &r.events,
        Duration::from_secs(10),
        |e| matches!(e, Event::RecordingStarted { request_id, .. } if *request_id == id),
    );
    std::thread::sleep(seconds);
    r.record.stop(id + 1);
    next_event(&r.events, Duration::from_secs(10), |e| {
        matches!(
            e,
            Event::RecordingStopped { .. } | Event::RecordingError { .. }
        )
    })
}

#[test]
fn a_clip_does_not_end_on_audio_the_source_has_not_delivered() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // Capture audio reaches the ring a latency after it plays. The save waited
    // for the video's last frame but took the audio window at once, so the
    // clip ended on zero padding where the source had not delivered yet.
    let mut r = rig_with(
        config(|c| c.microphone_endpoint_id.clear()),
        Arc::new(LaggyAudio::new(Duration::from_millis(150))),
        Duration::from_millis(16),
    );
    r.engine.start().unwrap();
    std::thread::sleep(Duration::from_secs(4));
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("tail.mp4");
    r.save.submit(SaveRequest {
        request_id: 41,
        path: path.clone(),
        seconds: 2,
    });
    let done = next_event(&r.events, Duration::from_secs(20), |e| {
        matches!(e, Event::SaveResult { request_id: 41, .. })
    });
    assert!(
        matches!(&done, Event::SaveResult { result: Ok(c), .. } if c.audio_included),
        "{done:?}"
    );
    let pcm = fthrust_media::audio_decode::decode_to_pcm(&path).unwrap();
    let frames = pcm.samples.len() / 2;
    let tail = &pcm.samples[(frames - 48 * 80) * 2..(frames - 48 * 10) * 2];
    let rms = (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt();
    assert!(
        rms > 0.1,
        "the clip's last 80 ms hold silence (rms {rms}) where the tone plays on"
    );
    r.engine.shutdown();
}

#[test]
fn stopping_a_recording_keeps_the_audio_up_to_its_last_frame() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // The record thread pulls audio 200 ms behind the clock in 100 ms steps;
    // a stop used to end the file there, 200-300 ms short of the video.
    let mut r = rig(
        config(|c| c.microphone_endpoint_id.clear()),
        SyntheticAudio::new(),
        Duration::from_millis(16),
    );
    r.engine.start().unwrap();
    wait_frames(&r.engine, 30);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("tail.mp4");
    let e = record_for(&r, 51, &path, Duration::from_secs(3));
    assert!(matches!(e, Event::RecordingStopped { .. }), "{e:?}");
    let (video, audio) = stream_seconds(&path);
    assert_eq!(audio.len(), 1);
    assert!(video > 2.5, "video {video}");
    assert!(
        (audio[0] - video).abs() <= 0.07,
        "audio {:.3} s, video {video:.3} s",
        audio[0]
    );
    r.engine.shutdown();
}

#[test]
fn recordings_carry_the_microphone() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // Clips get Default Mix and Microphone; recordings only ever wrote the
    // desktop track.
    let mut r = rig(
        config(|_| {}),
        SyntheticAudio::new(),
        Duration::from_millis(16),
    );
    r.engine.start().unwrap();
    wait_frames(&r.engine, 30);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("mic.mp4");
    let e = record_for(&r, 61, &path, Duration::from_secs(2));
    assert!(matches!(e, Event::RecordingStopped { .. }), "{e:?}");
    let names: Vec<String> = fthrust_media::combined_audio::descriptors_from_file(&path)
        .unwrap()
        .into_iter()
        .map(|d| d.handler_name.unwrap_or_default())
        .collect();
    assert_eq!(names, ["Default Mix", "Microphone"]);
    let (video, audio) = stream_seconds(&path);
    for a in audio {
        assert!(
            (a - video).abs() <= 0.07,
            "audio {a:.3} s, video {video:.3} s"
        );
    }
    r.engine.shutdown();
}

#[test]
fn a_slow_audio_status_does_not_stall_capture() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // status() held the engine's status lock while it asked the audio
    // sources for theirs; the capture thread takes that lock for every
    // frame, so a slow source status (its locks held by a long save or the
    // capture callback) stalled capture.
    let mut r = rig_with(
        config(|c| c.microphone_endpoint_id.clear()),
        Arc::new(SlowStatusAudio {
            inner: SyntheticAudio::new(),
            delay: Duration::from_millis(150),
        }),
        Duration::from_millis(16),
    );
    r.engine.start().unwrap();
    wait_frames(&r.engine, 10);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let poller = {
        let (handle, stop) = (r.engine.handle(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = handle.status();
            }
        })
    };
    let ring = r.engine.ring();
    std::thread::sleep(Duration::from_millis(300));
    let before = ring.stats().packets;
    std::thread::sleep(Duration::from_secs(2));
    let during = ring.stats().packets - before;
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    poller.join().unwrap();
    // About 120 frames when capture runs freely; it got none while status
    // held its lock. A loaded machine (the whole suite runs in parallel)
    // still clears 20 by far.
    assert!(
        during >= 20,
        "{during} frames in 2 s at a 16 ms frame interval while status was polled"
    );
    r.engine.shutdown();
}

#[test]
fn starting_a_recording_does_not_wait_on_the_engine() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // StartRecording ran on the daemon loop: the admission checks and the
    // file creation, muxer and encoder setup all held up heartbeats and
    // every other command. Now the record thread does all of it.
    let mut r = rig_with(
        config(|c| c.microphone_endpoint_id.clear()),
        Arc::new(SlowStatusAudio {
            inner: SyntheticAudio::new(),
            delay: Duration::from_millis(300),
        }),
        Duration::from_millis(16),
    );
    r.engine.start().unwrap();
    wait_frames(&r.engine, 10);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("quick.mp4");
    let t = Instant::now();
    r.record.start(71, path.clone());
    let took = t.elapsed();
    assert!(took < Duration::from_millis(100), "start took {took:?}");
    next_event(&r.events, Duration::from_secs(10), |e| {
        matches!(e, Event::RecordingStarted { request_id: 71, .. })
    });
    r.record.start(72, tmp.path().join("second.mp4"));
    let e = next_event(&r.events, Duration::from_secs(5), |e| {
        matches!(e, Event::RecordingError { request_id: 72, .. })
    });
    assert!(
        matches!(&e, Event::RecordingError { message, .. } if message.contains("already in progress")),
        "{e:?}"
    );
    r.record.stop(73);
    let e = next_event(&r.events, Duration::from_secs(10), |e| {
        matches!(
            e,
            Event::RecordingStopped { .. } | Event::RecordingError { .. }
        )
    });
    assert!(
        matches!(e, Event::RecordingStopped { request_id: 73, .. }),
        "{e:?}"
    );
    assert!(path.exists());
    r.engine.shutdown();
}
