use std::sync::Arc;
use std::time::{Duration, Instant};

use fthrust_core::capture_config::{CaptureConfig, Codec, EncoderPref};
use fthrust_core::capture_health::flags;
use fthrust_engine::audio::pipewire::PipeWireAudio;
use fthrust_engine::backend::RealFactory;
use fthrust_engine::engine::{CaptureEngine, EngineDeps, real_backoff};
use fthrust_engine::save::{SaveRequest, SaveWorker};
use fthrust_ipc::{BackendKind, EngineConfig, Event};

fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

fn config(encoder: EncoderPref, backend_override: Option<BackendKind>) -> EngineConfig {
    EngineConfig {
        capture: CaptureConfig {
            fps: 60,
            buffer_seconds: 10,
            bitrate_kbps: 16000,
            codec: Codec::H264,
            encoder,
            preset: 4,
            audio_enabled: true,
            ..CaptureConfig::default()
        },
        max_buffer_mb: 0,
        backend_override,
    }
}

fn deps() -> EngineDeps {
    EngineDeps {
        backends: Arc::new(RealFactory::new(None)),
        audio: Arc::new(PipeWireAudio::new()),
        backoff: real_backoff(),
    }
}

fn first_output_size() -> (u32, u32) {
    let conn = fthrust_wayland::connection::WaylandConnection::connect(
        Instant::now() + Duration::from_secs(5),
        &|| true,
    )
    .unwrap();
    let (_, info) = conn.outputs().select("").unwrap();
    (info.width, info.height)
}

#[test]
fn live_wayland_capture_saves_a_real_clip() {
    if !wayland() {
        return eprintln!("skipping: no WAYLAND_DISPLAY");
    }
    let software = std::env::var("FTHRUST_LIVE_SOFTWARE").as_deref() == Ok("1");
    let encoder = if software {
        EncoderPref::Software
    } else {
        EncoderPref::Auto
    };
    let mut engine = CaptureEngine::new(config(encoder, None), deps());
    let notices = engine.start().unwrap();
    eprintln!("startup notices: {notices:?}");
    std::thread::sleep(Duration::from_secs(5));
    let s = engine.status();
    assert_eq!(
        s.health_flags & !flags::CONTENT_SUSPECT,
        flags::ACTIVE,
        "{s:?}"
    );
    assert!(
        s.frames_captured >= 200,
        "{} frames in 5 s",
        s.frames_captured
    );
    assert!(
        matches!(
            s.backend,
            Some(BackendKind::ExtImageCopy | BackendKind::WlrScreencopy)
        ),
        "{:?}",
        s.backend
    );
    assert_eq!((s.native_width, s.native_height), first_output_size());
    if !software {
        assert!(
            s.active_codec.contains("vaapi") || s.active_codec.contains("nvenc"),
            "{}",
            s.active_codec
        );
    }
    let (tx, rx) = crossbeam_channel::unbounded::<Event>();
    let save = SaveWorker::spawn(engine.handle(), tx);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("live.mp4");
    save.submit(SaveRequest {
        request_id: 1,
        path: path.clone(),
        seconds: 3,
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let clip = loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Event::SaveResult { result, .. }) => break result.unwrap(),
            Ok(_) => {}
            Err(_) => panic!("no save result"),
        }
    };
    let meta = fthrust_media::probe::probe_video_metadata(&path).unwrap();
    let d = meta.duration_seconds.unwrap();
    assert!((d - 3.0).abs() <= 1.0 / 60.0 + 0.05, "duration {d}");
    let (w, h) = (meta.width.unwrap(), meta.height.unwrap());
    assert!(
        w.is_multiple_of(2) && h.is_multiple_of(2) && w <= s.native_width && h <= s.native_height
    );
    eprintln!("clip: {clip:?}");
    engine.shutdown();
}

#[test]
fn override_env_selects_backend() {
    if !wayland() {
        return eprintln!("skipping: no WAYLAND_DISPLAY");
    }
    let mut engine = CaptureEngine::new(
        config(EncoderPref::Software, Some(BackendKind::WlrScreencopy)),
        deps(),
    );
    engine.start().unwrap();
    let started = Instant::now();
    while engine.status().frames_captured < 5 {
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(engine.status().backend, Some(BackendKind::WlrScreencopy));
    engine.shutdown();
}
