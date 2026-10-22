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
    }
}

struct Rig {
    engine: CaptureEngine,
    save: SaveWorker,
    record: RecordWorker,
    events: crossbeam_channel::Receiver<Event>,
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
    let record = RecordWorker::new(engine.handle(), tx);
    Rig {
        engine,
        save,
        record,
        events: rx,
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
    match e {
        Event::SaveResult {
            result: Err(msg), ..
        } => {
            // The reference's wording for an empty ring; "still rebuilding"
            // is reserved for a ring holding only an older generation.
            assert_eq!(
                msg,
                "Nothing to save: the replay buffer contains no video packets"
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(!path.exists());
    r.engine.shutdown();
    let _ = &r.script;
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
