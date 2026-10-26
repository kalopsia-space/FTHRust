use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "../../fthrust-media/tests/support/decode.rs"]
mod decode;

use fthrust_core::capture_config::{CaptureConfig, Codec, EncoderPref, Scaling};
use fthrust_core::capture_health::flags;
use fthrust_engine::audio::SyntheticAudio;
use fthrust_engine::backend::{
    CaptureError, FailureKind, KindScript, RealFactory, SyntheticFactory, SyntheticScript,
    solid_grey,
};
use fthrust_engine::engine::{CaptureEngine, EngineDeps, EngineNotice};
use fthrust_ipc::{BackendKind, EngineConfig};

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
        audio_enabled: false,
        ..CaptureConfig::default()
    };
    f(&mut capture);
    EngineConfig {
        capture,
        max_buffer_mb: 0,
        backend_override: None,
    }
}

struct Harness {
    engine: CaptureEngine,
    script: Arc<Mutex<SyntheticScript>>,
    sleeps: Arc<Mutex<Vec<u64>>>,
}

fn harness(cfg: EngineConfig, script: SyntheticScript) -> Harness {
    let script = Arc::new(Mutex::new(script));
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let recorded = sleeps.clone();
    let deps = EngineDeps {
        backends: Arc::new(SyntheticFactory {
            script: script.clone(),
            candidates: vec![BackendKind::WlrScreencopy, BackendKind::ExtImageCopy],
        }),
        audio: Arc::new(SyntheticAudio::new()),
        backoff: Arc::new(move |d: Duration, running: &dyn Fn() -> bool| {
            recorded.lock().unwrap().push(d.as_millis() as u64);
            running()
        }),
    };
    Harness {
        engine: CaptureEngine::new(cfg, deps),
        script,
        sleeps,
    }
}

fn wlr_ok(width: u32, height: u32) -> SyntheticScript {
    let mut s = SyntheticScript::new(width, height);
    s.kinds
        .insert(BackendKind::WlrScreencopy, KindScript::default());
    s
}

fn wait_for(
    engine: &CaptureEngine,
    timeout: Duration,
    mut pred: impl FnMut(&fthrust_ipc::StatusSnapshot) -> bool,
) -> fthrust_ipc::StatusSnapshot {
    let started = Instant::now();
    loop {
        let s = engine.status();
        if pred(&s) {
            return s;
        }
        if started.elapsed() > timeout {
            panic!("timed out waiting; status {s:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_notice(
    engine: &CaptureEngine,
    timeout: Duration,
    mut pred: impl FnMut(&EngineNotice) -> bool,
) -> EngineNotice {
    let rx = engine.events();
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(n) if pred(&n) => return n,
            Ok(_) => {}
            Err(_) => panic!("no matching notice within {timeout:?}"),
        }
    }
}

#[test]
fn healthy_generation_publishes_frames_and_flags() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut h = harness(config(|_| {}), wlr_ok(320, 180));
    let notices = h.engine.start().unwrap();
    assert!(notices.is_empty(), "{notices:?}");
    let s = wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.frames_captured >= 20
    });
    assert_eq!(s.health_flags, flags::ACTIVE);
    assert!(s.encoder_open);
    assert!(!s.active_codec.is_empty());
    assert_eq!(s.capture_generation, 1);
    assert_eq!(s.encoder_generation, 1);
    assert_eq!(s.backend, Some(BackendKind::WlrScreencopy));
    assert_eq!((s.native_width, s.native_height), (320, 180));
    assert!(h.engine.encoder_snapshot().is_some());
    assert!(h.engine.ring().stats().packets > 0);
    h.engine.shutdown();
    let _ = &h.script;
}

#[test]
fn synthetic_capture_encodes_without_normalising() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut h = harness(config(|_| {}), wlr_ok(320, 180));
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.frames_captured >= 30
    });
    assert!(h.engine.ring().stats().packets > 0);
    assert_eq!(h.engine.normalized_frames(), 0);
    h.engine.shutdown();
}

#[test]
fn fit_scaling_masks_even_and_keeps_aspect() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut h = harness(
        config(|c| {
            c.width = 640;
            c.height = 360;
            c.scaling = Scaling::Fit;
        }),
        wlr_ok(1001, 601),
    );
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.frames_captured >= 2
    });
    let snap = h.engine.encoder_snapshot().unwrap();
    assert_eq!((snap.width, snap.height), (598, 360));
    h.engine.shutdown();
    let mut h = harness(
        config(|c| {
            c.width = 640;
            c.height = 360;
            c.scaling = Scaling::Stretch;
        }),
        wlr_ok(1001, 601),
    );
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.frames_captured >= 2
    });
    assert_eq!(
        {
            let s = h.engine.encoder_snapshot().unwrap();
            (s.width, s.height)
        },
        (640, 360)
    );
    h.engine.shutdown();
    let mut h = harness(config(|_| {}), wlr_ok(1001, 601));
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.frames_captured >= 2
    });
    assert_eq!(
        {
            let s = h.engine.encoder_snapshot().unwrap();
            (s.width, s.height)
        },
        (1000, 600),
        "native, even-masked"
    );
    h.engine.shutdown();
}

#[test]
fn pause_sets_flag_and_stops_frames() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut h = harness(config(|_| {}), wlr_ok(320, 180));
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.frames_captured >= 5
    });
    h.engine.set_paused(true);
    let s = wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.paused && s.health_flags & flags::PAUSED != 0
    });
    assert_eq!(s.health_flags, flags::ACTIVE | flags::PAUSED);
    std::thread::sleep(Duration::from_millis(300));
    let frames = h.engine.status().frames_captured;
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        h.engine.status().frames_captured,
        frames,
        "no frames while paused"
    );
    h.engine.set_paused(false);
    wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.frames_captured > frames && s.health_flags == flags::ACTIVE
    });
    h.engine.shutdown();
}

#[test]
fn exhausted_budget_falls_through_then_fails_with_reason() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // wlr always dies after 5 frames; ext works.
    let mut script = SyntheticScript::new(320, 180);
    script.kinds.insert(
        BackendKind::WlrScreencopy,
        KindScript {
            capture_error_after: Some((5, CaptureError::Failed("synthetic".into()))),
            ..KindScript::default()
        },
    );
    script
        .kinds
        .insert(BackendKind::ExtImageCopy, KindScript::default());
    let mut h = harness(config(|_| {}), script);
    h.engine.start().unwrap();
    let n = wait_notice(&h.engine, Duration::from_secs(10), |n| {
        matches!(
            n,
            EngineNotice::GenerationStarted {
                backend: BackendKind::ExtImageCopy,
                ..
            }
        )
    });
    let EngineNotice::GenerationStarted {
        capture_generation, ..
    } = n
    else {
        unreachable!()
    };
    assert_eq!(capture_generation, 5, "four wlr generations, then ext");
    assert_eq!(
        *h.sleeps.lock().unwrap(),
        [250, 750, 1500, 250],
        "wlr budget, then a fresh budget on ext"
    );
    let s = wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.backend == Some(BackendKind::ExtImageCopy) && s.frames_captured > 0
    });
    assert_eq!(s.health_flags, flags::ACTIVE);
    h.engine.shutdown();

    // Every candidate dies: FAILED with the reason, status still served.
    let mut script = SyntheticScript::new(320, 180);
    for kind in [BackendKind::WlrScreencopy, BackendKind::ExtImageCopy] {
        script.kinds.insert(
            kind,
            KindScript {
                fail_inits: VecDeque::from(vec![FailureKind::Error; 8]),
                ..KindScript::default()
            },
        );
    }
    let mut h = harness(config(|_| {}), script);
    match h.engine.start() {
        Err(e) => {
            assert_eq!(e.code, "CAPTURE_BACKEND_UNAVAILABLE");
            assert!(!e.detail.is_empty());
        }
        Ok(_) => panic!("expected a startup error"),
    }
    let s = h.engine.status();
    assert_ne!(s.health_flags & flags::BACKEND_FAILED, 0);
    assert!(!s.capture_failure_detail.is_empty());
    h.engine.shutdown();
}

#[test]
fn transient_failures_wait_without_budget() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut script = SyntheticScript::new(320, 180);
    script.kinds.insert(
        BackendKind::WlrScreencopy,
        KindScript {
            fail_inits: VecDeque::from(vec![FailureKind::CompositorUnreachable; 3]),
            ..KindScript::default()
        },
    );
    let mut h = harness(config(|_| {}), script);
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.frames_captured >= 1
    });
    assert_eq!(*h.sleeps.lock().unwrap(), [2000, 2000, 2000]);
    assert_eq!(h.engine.status().capture_generation, 1);
    h.engine.shutdown();
}

#[test]
fn output_absent_waits_and_resumes_same_output() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut script = SyntheticScript::new(320, 180);
    script.kinds.insert(
        BackendKind::WlrScreencopy,
        KindScript {
            outputs: vec!["DP-9".into()],
            ..KindScript::default()
        },
    );
    let mut h = harness(config(|c| c.monitor = "DP-9".into()), script);
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.selected_output == "DP-9" && s.frames_captured >= 1
    });
    // Unplug: the output disappears and the generation dies.
    {
        let mut s = h.script.lock().unwrap();
        let k = s.kinds.get_mut(&BackendKind::WlrScreencopy).unwrap();
        k.outputs.clear();
        k.capture_error_after = Some((0, CaptureError::Failed("unplugged".into())));
    }
    wait_notice(&h.engine, Duration::from_secs(5), |n| {
        matches!(n, EngineNotice::Recovering { .. })
    });
    // The dead generation costs one counted backoff; the absent output is
    // then waited out at 2 s without consuming budget.
    wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.health_flags & flags::RECOVERING != 0 && s.recovery_reason.contains("DP-9")
    });
    std::thread::sleep(Duration::from_millis(100));
    let sleeps = h.sleeps.lock().unwrap().clone();
    assert_eq!(sleeps[0], 250, "{sleeps:?}");
    assert!(
        sleeps.len() >= 2 && sleeps[1..].iter().all(|ms| *ms == 2000),
        "{sleeps:?}"
    );
    // Replug.
    {
        let mut s = h.script.lock().unwrap();
        let k = s.kinds.get_mut(&BackendKind::WlrScreencopy).unwrap();
        k.outputs = vec!["DP-9".into()];
        k.capture_error_after = None;
    }
    let s = wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.health_flags == flags::ACTIVE && s.capture_generation >= 2
    });
    assert_eq!(s.selected_output, "DP-9");
    h.engine.shutdown();
}

#[test]
fn disconnect_mid_generation_recovers() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut script = SyntheticScript::new(320, 180);
    script.kinds.insert(
        BackendKind::WlrScreencopy,
        KindScript {
            capture_error_after: Some((10, CaptureError::Disconnected)),
            ..KindScript::default()
        },
    );
    let mut h = harness(config(|_| {}), script);
    h.engine.start().unwrap();
    wait_notice(&h.engine, Duration::from_secs(5), |n| {
        matches!(n, EngineNotice::Recovering { .. })
    });
    h.script
        .lock()
        .unwrap()
        .kinds
        .get_mut(&BackendKind::WlrScreencopy)
        .unwrap()
        .capture_error_after = None;
    let s = wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.capture_generation >= 2 && s.health_flags == flags::ACTIVE
    });
    assert!(s.recovery_reason.is_empty());
    assert!(
        h.sleeps.lock().unwrap().contains(&2000),
        "a disconnect is transient"
    );
    h.engine.shutdown();
}

#[test]
fn encode_failures_reopen_encoder() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut script = wlr_ok(320, 180);
    script.short_stride = true;
    let mut h = harness(config(|_| {}), script);
    h.engine.start().unwrap();
    let s = wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.encoder_generation >= 2
    });
    assert!(s.frames_captured == 0);
    h.script.lock().unwrap().short_stride = false;
    wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.frames_captured > 0
    });
    h.engine.shutdown();
}

#[test]
fn content_watchdog_flags_black_screen() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut script = wlr_ok(320, 180);
    script.black = true;
    script.frame_interval = Duration::from_millis(1);
    let mut h = harness(config(|c| c.fps = 30), script);
    h.engine.start().unwrap();
    // One sample per 30 frames; 12 consecutive samples = 360 frames.
    let s = wait_for(&h.engine, Duration::from_secs(60), |s| {
        s.content_suspicious_streak >= 12
    });
    assert_ne!(s.health_flags & flags::CONTENT_SUSPECT, 0);
    assert!(s.content_luma_mean <= 8.0);
    h.engine.shutdown();
}

#[test]
fn startup_without_wayland_is_coded() {
    let deps = EngineDeps {
        backends: Arc::new(RealFactory::assume_wayland(false)),
        audio: Arc::new(SyntheticAudio::new()),
        backoff: Arc::new(|_, running: &dyn Fn() -> bool| running()),
    };
    let mut engine = CaptureEngine::new(config(|_| {}), deps);
    let err = engine.start().unwrap_err();
    assert_eq!(err.code, "WAYLAND_UNAVAILABLE");
    engine.shutdown();
}

#[test]
fn rotated_output_sizes_encoder_from_the_upright_frame() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut script = wlr_ok(320, 180);
    script.transform = fthrust_wayland::frame::OutputTransform::Rot90;
    let mut h = harness(config(|_| {}), script);
    h.engine.start().unwrap();
    let s = wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.frames_captured >= 2
    });
    let snap = h.engine.encoder_snapshot().unwrap();
    assert_eq!(
        (snap.width, snap.height),
        (180, 320),
        "the buffer is 320x180, the upright frame is 180x320"
    );
    assert_eq!((s.native_width, s.native_height), (180, 320));
    h.engine.shutdown();
}

#[test]
fn backend_and_output_are_pinned_at_open_even_before_a_frame() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // The first open sees DP-9 and dies before its first frame; every later
    // open sees only DP-1. Recovery must insist on DP-9 (OutputAbsent waits),
    // never fall back to DP-1.
    let mut script = SyntheticScript::new(320, 180);
    script.kinds.insert(
        BackendKind::WlrScreencopy,
        KindScript {
            outputs: vec!["DP-1".into()],
            outputs_sequence: VecDeque::from(vec![vec!["DP-9".to_string()]]),
            capture_error_after: Some((0, CaptureError::Failed("died early".into()))),
            ..KindScript::default()
        },
    );
    let mut h = harness(config(|c| c.monitor = "DP-9".into()), script);
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.health_flags & flags::RECOVERING != 0 && s.recovery_reason.contains("DP-9")
    });
    std::thread::sleep(Duration::from_millis(100));
    let s = h.engine.status();
    assert_ne!(
        s.selected_output, "DP-1",
        "never silently captures another monitor: {s:?}"
    );
    assert_eq!(s.capture_generation, 1);
    {
        let mut sc = h.script.lock().unwrap();
        let k = sc.kinds.get_mut(&BackendKind::WlrScreencopy).unwrap();
        k.outputs = vec!["DP-9".into()];
        k.capture_error_after = None;
    }
    let s = wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.health_flags == flags::ACTIVE && s.frames_captured > 0
    });
    assert_eq!(s.selected_output, "DP-9");
    h.engine.shutdown();
}

#[test]
fn reconfigure_restarts_audio_with_the_new_settings() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut h = harness(config(|c| c.audio_enabled = true), wlr_ok(320, 180));
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.audio.system == fthrust_ipc::SourceStatus::Streaming
    });
    let (warnings, notices) = h
        .engine
        .reconfigure(config(|c| c.audio_enabled = false))
        .unwrap();
    assert!(warnings.is_empty() && notices.is_empty());
    let s = wait_for(&h.engine, Duration::from_secs(5), |s| s.frames_captured > 0);
    assert_eq!(s.audio.system, fthrust_ipc::SourceStatus::Disabled);
    h.engine
        .reconfigure(config(|c| c.audio_enabled = true))
        .unwrap();
    wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.audio.system == fthrust_ipc::SourceStatus::Streaming
    });
    h.engine.shutdown();
}

#[test]
fn startup_failure_is_reported_even_when_instant() {
    // Every candidate fails at once (no backoff hook delay): the first
    // outcome must still reach start().
    for _ in 0..5 {
        let mut script = SyntheticScript::new(320, 180);
        for kind in [BackendKind::WlrScreencopy, BackendKind::ExtImageCopy] {
            script.kinds.insert(
                kind,
                KindScript {
                    fail_inits: VecDeque::from(vec![FailureKind::Error; 8]),
                    ..KindScript::default()
                },
            );
        }
        let mut h = harness(config(|_| {}), script);
        let started = Instant::now();
        let err = h.engine.start().unwrap_err();
        assert_eq!(err.code, "CAPTURE_BACKEND_UNAVAILABLE");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "start() waited for a lost outcome"
        );
        h.engine.shutdown();
    }
}

#[test]
fn static_screen_repeats_do_not_convert() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut script = wlr_ok(320, 180);
    script.static_after = Some(1);
    let mut h = harness(config(|_| {}), script);
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.frames_captured >= 5
    });
    let before = h.engine.encoder_conversions();
    let frames = h.engine.status().frames_captured;
    wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.frames_captured >= frames + 30
    });
    assert_eq!(h.engine.encoder_conversions(), before);
    assert!(h.engine.ring().stats().packets > 0);
    h.engine.shutdown();
}

#[test]
fn repeats_carry_the_last_fresh_frame_when_pacing_skipped_it() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // Frames arrive about eight times per encoded frame, then the screen
    // goes static: the repeats must show the last fresh frame even though
    // pacing never encoded it.
    const LAST_FRESH: u64 = 60;
    let mut script = wlr_ok(160, 90);
    script.frame_interval = Duration::from_millis(4);
    script.solid = true;
    script.static_after = Some(LAST_FRESH + 1);
    let mut h = harness(config(|c| c.fps = 30), script);
    let (tx, rx) = crossbeam_channel::unbounded();
    h.engine.set_packet_tap(Some(tx));
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(20), |s| {
        s.frames_captured >= 30
    });
    h.engine.shutdown();
    let packets: Vec<_> = rx.try_iter().collect();
    let extradata = h.engine.encoder_snapshot().map(|s| s.extradata);
    let extradata = extradata.unwrap_or_else(|| {
        panic!("no encoder snapshot after shutdown");
    });
    let (rgb, ..) = decode::decode_frame_rgb(&extradata, &packets, packets.len() - 1);
    let expected = i32::from(solid_grey(LAST_FRESH));
    let got = i32::from(rgb[rgb.len() / 2]);
    assert!(
        (got - expected).abs() <= 8,
        "last picture is grey {got}, the last fresh frame is grey {expected}"
    );
}

#[test]
fn static_black_rotated_stream_is_still_flagged() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut script = wlr_ok(320, 180);
    script.black = true;
    script.static_after = Some(1);
    script.transform = fthrust_wayland::frame::OutputTransform::Rot90;
    script.frame_interval = Duration::from_millis(1);
    let mut h = harness(config(|c| c.fps = 30), script);
    h.engine.start().unwrap();
    let s = wait_for(&h.engine, Duration::from_secs(60), |s| {
        s.content_suspicious_streak >= 12
    });
    assert_ne!(s.health_flags & flags::CONTENT_SUSPECT, 0);
    assert!(
        h.engine.normalized_frames() < 100,
        "repeats should skip normalize"
    );
    h.engine.shutdown();
}

/// Constraints that accept exactly what this machine's VA-API encoder
/// exports, or `None` without VA-API.
fn vaapi_dmabuf_constraints(
    width: u32,
    height: u32,
) -> Option<fthrust_wayland::dmabuf::DmabufConstraints> {
    use fthrust_media::encoder::{Encoder, EncoderConfig};
    fthrust_media::init().ok()?;
    if !fthrust_media::vaapi::vaapi_available() {
        return None;
    }
    let mut enc = Encoder::open(&EncoderConfig {
        src_width: width,
        src_height: height,
        enc_width: width,
        enc_height: height,
        fps: 30,
        bitrate_kbps: 2000,
        codec: Codec::H264,
        encoder: EncoderPref::Vaapi,
        preset: 1,
    })
    .ok()?;
    let image = enc
        .dmabuf_targets(width, height, 1)
        .ok()?
        .into_iter()
        .next()?;
    let device = fthrust_media::vaapi::render_node_dev(enc.render_node()?)?;
    Some(fthrust_wayland::dmabuf::DmabufConstraints {
        device,
        formats: vec![(image.fourcc, vec![image.modifier])],
    })
}

fn ext_dmabuf(
    width: u32,
    height: u32,
    constraints: fthrust_wayland::dmabuf::DmabufConstraints,
) -> SyntheticScript {
    let mut s = SyntheticScript::new(width, height);
    s.kinds
        .insert(BackendKind::ExtImageCopy, KindScript::default());
    s.dmabuf = Some(constraints);
    s
}

fn vaapi_config() -> EngineConfig {
    config(|c| c.encoder = EncoderPref::Vaapi)
}

#[test]
fn zero_copy_capture_encodes_dmabuf_frames() {
    let Some(c) = vaapi_dmabuf_constraints(320, 180) else {
        return eprintln!("skipping: no VA-API device");
    };
    let mut h = harness(vaapi_config(), ext_dmabuf(320, 180, c));
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.frames_captured >= 30
    });
    assert_eq!(h.script.lock().unwrap().dmabuf_uses, 1);
    assert_eq!(h.engine.normalized_frames(), 0);
    assert!(h.engine.ring().stats().packets > 0);
    h.engine.shutdown();
}

#[test]
fn dmabuf_geometry_change_reallocates_or_falls_back() {
    let Some(c) = vaapi_dmabuf_constraints(320, 180) else {
        return eprintln!("skipping: no VA-API device");
    };
    let mut script = ext_dmabuf(320, 180, c);
    script.dmabuf_invalidate_after = Some(10);
    let mut h = harness(vaapi_config(), script);
    h.engine.start().unwrap();
    let s = wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.frames_captured >= 40
    });
    assert_eq!(s.capture_generation, 1, "no generation restart");
    assert_eq!(h.script.lock().unwrap().dmabuf_uses, 2, "reallocated once");
    h.engine.shutdown();
}

#[test]
fn zero_copy_off_for_another_gpu_copies_frames() {
    let Some(mut c) = vaapi_dmabuf_constraints(320, 180) else {
        return eprintln!("skipping: no VA-API device");
    };
    c.device = u64::MAX;
    let mut h = harness(vaapi_config(), ext_dmabuf(320, 180, c));
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.frames_captured >= 30
    });
    assert_eq!(h.script.lock().unwrap().dmabuf_uses, 0);
    assert!(h.engine.ring().stats().packets > 0);
    h.engine.shutdown();
}
