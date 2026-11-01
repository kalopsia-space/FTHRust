use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "../../fthrust-media/tests/support/decode.rs"]
mod decode;

use fthrust_core::capture_config::{CaptureConfig, Codec, EncoderPref, Scaling};
use fthrust_core::capture_health::flags;
use fthrust_engine::audio::SyntheticAudio;
use fthrust_engine::backend::{
    CaptureError, FailureKind, KindScript, OpenReason, RealFactory, SyntheticFactory,
    SyntheticScript, solid_grey,
};
use fthrust_engine::engine::{CaptureEngine, EngineDeps, EngineNotice};
use fthrust_ipc::{BackendKind, EngineConfig};
use fthrust_wayland::backend::DMABUF_FIRST_FRAME_DEADLINE;

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
        automatic_start: false,
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
fn a_preset_the_settings_do_not_show_is_not_applied() {
    fthrust_media::init().unwrap();
    let has = |n: &str| ffmpeg_next::encoder::find_by_name(n).is_some();
    // x264 writes its settings into its first packet; OpenH264 is picked
    // first where it exists.
    if has("libopenh264") || !has("libx264") {
        return eprintln!("skipping: the software H.264 encoder is not x264");
    }
    // P7 picked while NVENC was chosen, then Software, whose settings show
    // no preset: x264 runs the default P4 (fast, subme=6), not P7
    // (veryslow, subme=10).
    let mut h = harness(config(|c| c.preset = 7), wlr_ok(320, 180));
    h.engine.start().unwrap();
    let ring = h.engine.ring();
    wait_for(&h.engine, Duration::from_secs(5), |_| {
        ring.stats().packets > 0
    });
    let snapshot = ring.snapshot(10_000, 0, 0).unwrap();
    let first = snapshot.packets[0].data.bytes().unwrap();
    let settings = String::from_utf8_lossy(&first).into_owned();
    h.engine.shutdown();
    assert!(settings.contains(" subme=6 "), "{settings}");
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
fn a_transient_wayland_failure_at_startup_does_not_open_the_portal() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // The compositor does not answer the first connection: ext and wlr are
    // waited out instead of falling through to the portal, whose dialog
    // would come up for nothing.
    let mut script = SyntheticScript::new(320, 180);
    for kind in [BackendKind::ExtImageCopy, BackendKind::WlrScreencopy] {
        script.kinds.insert(
            kind,
            KindScript {
                fail_inits: VecDeque::from(vec![FailureKind::CompositorUnreachable]),
                ..KindScript::default()
            },
        );
    }
    script
        .kinds
        .insert(BackendKind::PortalScreenCast, KindScript::default());
    let script = Arc::new(Mutex::new(script));
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let recorded = sleeps.clone();
    let deps = EngineDeps {
        backends: Arc::new(SyntheticFactory {
            script,
            candidates: vec![
                BackendKind::ExtImageCopy,
                BackendKind::WlrScreencopy,
                BackendKind::PortalScreenCast,
            ],
        }),
        audio: Arc::new(SyntheticAudio::new()),
        backoff: Arc::new(move |d: Duration, running: &dyn Fn() -> bool| {
            recorded.lock().unwrap().push(d.as_millis() as u64);
            running()
        }),
    };
    let mut engine = CaptureEngine::new(config(|_| {}), deps);
    engine.start().unwrap();
    let s = wait_for(&engine, Duration::from_secs(5), |s| s.frames_captured >= 1);
    engine.shutdown();
    assert_eq!(s.backend, Some(BackendKind::ExtImageCopy), "{s:?}");
    assert_eq!(s.capture_generation, 1);
    assert_eq!(*sleeps.lock().unwrap(), [2000], "one transient wait");
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
fn a_black_screen_restarts_capture_once() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // Twelve black samples (one a second) flag the content and restart
    // capture; while the picture stays black it is not restarted again.
    let mut script = wlr_ok(320, 180);
    script.black = true;
    let mut h = harness(config(|c| c.fps = 30), script);
    h.engine.start().unwrap();
    let n = wait_notice(&h.engine, Duration::from_secs(30), |n| {
        matches!(n, EngineNotice::Recovering { .. })
    });
    let EngineNotice::Recovering { reason } = n else {
        unreachable!()
    };
    assert!(reason.contains("black"), "{reason}");
    wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.capture_generation == 2 && s.health_flags & flags::RECOVERING == 0
    });
    std::thread::sleep(Duration::from_secs(4));
    let s = h.engine.status();
    h.engine.shutdown();
    assert_eq!(s.capture_generation, 2, "restarted once: {s:?}");
    assert_ne!(s.health_flags & flags::CONTENT_SUSPECT, 0, "still flagged");
    assert!(s.content_suspicious_streak >= 15, "{s:?}");
    assert_eq!(*h.sleeps.lock().unwrap(), [250]);
}

#[test]
fn a_black_screen_never_reopens_the_portal() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // On the portal a restart means a new ScreenCast session, which can
    // put the picker in front of the game: a dark run is only flagged.
    let mut script = SyntheticScript::new(320, 180);
    script
        .kinds
        .insert(BackendKind::PortalScreenCast, KindScript::default());
    script.black = true;
    let script = Arc::new(Mutex::new(script));
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let recorded = sleeps.clone();
    let deps = EngineDeps {
        backends: Arc::new(SyntheticFactory {
            script: script.clone(),
            candidates: vec![BackendKind::PortalScreenCast],
        }),
        audio: Arc::new(SyntheticAudio::new()),
        backoff: Arc::new(move |d: Duration, running: &dyn Fn() -> bool| {
            recorded.lock().unwrap().push(d.as_millis() as u64);
            running()
        }),
    };
    let mut engine = CaptureEngine::new(config(|c| c.fps = 30), deps);
    engine.start().unwrap();
    // Two samples past the one that makes the content suspect.
    let s = wait_for(&engine, Duration::from_secs(30), |s| {
        s.content_suspicious_streak >= 14
    });
    engine.shutdown();
    assert_ne!(s.health_flags & flags::CONTENT_SUSPECT, 0, "{s:?}");
    assert_eq!(s.capture_generation, 1, "{s:?}");
    assert_eq!(script.lock().unwrap().opens, 1, "the session was reopened");
    assert!(sleeps.lock().unwrap().is_empty(), "no recovery");
}

/// A synthetic engine over `kinds` (in that candidate order) whose
/// backoffs are recorded instead of slept.
fn engine_over(script: SyntheticScript, candidates: Vec<BackendKind>) -> Harness {
    engine_with(config(|_| {}), script, candidates)
}

fn engine_with(
    cfg: EngineConfig,
    script: SyntheticScript,
    candidates: Vec<BackendKind>,
) -> Harness {
    let script = Arc::new(Mutex::new(script));
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let recorded = sleeps.clone();
    let deps = EngineDeps {
        backends: Arc::new(SyntheticFactory {
            script: script.clone(),
            candidates,
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

#[test]
fn a_portal_session_the_desktop_closed_is_not_reopened() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // The desktop's "Stop sharing" ends capture like a dismissed picker:
    // no recovery, no new session ~250 ms later.
    let mut script = SyntheticScript::new(320, 180);
    script.kinds.insert(
        BackendKind::PortalScreenCast,
        KindScript {
            capture_error_after: Some((
                10,
                CaptureError::Closed("screen sharing was stopped from the desktop".into()),
            )),
            ..KindScript::default()
        },
    );
    let mut h = engine_over(script, vec![BackendKind::PortalScreenCast]);
    h.engine.start().unwrap();
    let s = wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.health_flags & flags::BACKEND_FAILED != 0
    });
    std::thread::sleep(Duration::from_millis(300));
    let opens = h.script.lock().unwrap().opens;
    h.engine.shutdown();
    assert!(s.capture_failure_detail.contains("desktop"), "{s:?}");
    assert_eq!(opens, 1, "the session was opened again");
    assert!(h.sleeps.lock().unwrap().is_empty(), "no recovery");
}

#[test]
fn only_the_first_open_may_show_a_dialog() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // The portal dies after 10 frames, then reopens: the open the user
    // started may show the portal's dialog, the recovery may not.
    let mut script = SyntheticScript::new(320, 180);
    script.kinds.insert(
        BackendKind::PortalScreenCast,
        KindScript {
            capture_error_after: Some((
                10,
                CaptureError::Failed("stream resolution changed".into()),
            )),
            ..KindScript::default()
        },
    );
    let mut h = engine_over(script, vec![BackendKind::PortalScreenCast]);
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.capture_generation >= 2
    });
    h.engine.shutdown();
    let reasons = h.script.lock().unwrap().open_reasons.clone();
    assert_eq!(
        reasons[..2],
        [
            (BackendKind::PortalScreenCast, OpenReason::Start),
            (BackendKind::PortalScreenCast, OpenReason::Recovery),
        ],
        "{reasons:?}"
    );
}

#[test]
fn an_engine_the_app_restarted_on_its_own_never_opens_with_a_dialog() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // The app restarts a crashed or hung engine without the user asking:
    // that engine's first open is already restore-only.
    let mut script = SyntheticScript::new(320, 180);
    script
        .kinds
        .insert(BackendKind::PortalScreenCast, KindScript::default());
    let mut cfg = config(|_| {});
    cfg.automatic_start = true;
    let mut h = engine_with(cfg, script, vec![BackendKind::PortalScreenCast]);
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.frames_captured >= 5
    });
    h.engine.shutdown();
    let reasons = h.script.lock().unwrap().open_reasons.clone();
    assert_eq!(
        reasons,
        [(BackendKind::PortalScreenCast, OpenReason::Recovery)],
        "{reasons:?}"
    );
}

#[test]
fn an_automatic_start_the_portal_cannot_restore_is_coded() {
    // The app restarted the engine on its own, and the portal has no saved
    // choice: going on would mean its dialog. Capture stops with a code of
    // its own, not as if the user had dismissed the picker. (No portal is
    // contacted: without a token the restore-only open fails at once.)
    let home = tempfile::tempdir().unwrap();
    let deps = EngineDeps {
        backends: Arc::new(
            RealFactory::assume_wayland(true).with_paths(fthrust_core::FthrPaths::new(home.path())),
        ),
        audio: Arc::new(SyntheticAudio::new()),
        backoff: Arc::new(|_, running: &dyn Fn() -> bool| running()),
    };
    let mut cfg = config(|_| {});
    cfg.backend_override = Some(BackendKind::PortalScreenCast);
    cfg.automatic_start = true;
    let mut engine = CaptureEngine::new(cfg, deps);
    let err = engine.start().unwrap_err();
    engine.shutdown();
    assert_eq!(err.code, "CAPTURE_NOT_RESTORED", "{err:?}");
    assert!(err.detail.contains("dialog"), "{err:?}");
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
    // Fresh frames are stamped 6 ms before they arrive (as PipeWire stamps
    // portal frames), so after each wait for the encode slot the first ones
    // are older than the slot and the throttle skips them. Then the screen
    // goes static: the repeats must show the last fresh frame even when the
    // encoder never took it.
    const LAST_FRESH: u64 = 20;
    let mut script = wlr_ok(160, 90);
    script.frame_interval = Duration::from_millis(4);
    script.timestamp_lag = Duration::from_millis(6);
    script.solid = true;
    script.static_after = Some(LAST_FRESH + 1);
    let mut h = harness(config(|c| c.fps = 30), script);
    let (tx, rx) = crossbeam_channel::unbounded();
    h.engine.set_packet_tap(Some(tx));
    h.engine.start().unwrap();
    let started = Instant::now();
    while h.script.lock().unwrap().captures < LAST_FRESH + 30 {
        assert!(started.elapsed() < Duration::from_secs(20), "no repeats");
        std::thread::sleep(Duration::from_millis(10));
    }
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
fn capture_requests_are_paced_to_the_frame_rate() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // The backend could deliver 500 frames a second and 30 are encoded:
    // every frame requested before the next encode slot would be copied
    // by the compositor for nothing.
    let mut script = wlr_ok(160, 90);
    script.frame_interval = Duration::from_millis(2);
    let mut h = harness(config(|c| c.fps = 30), script);
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.frames_captured >= 10
    });
    let captures = || h.script.lock().unwrap().captures;
    let (c0, f0) = (captures(), h.engine.status().frames_captured);
    std::thread::sleep(Duration::from_secs(1));
    let (c1, f1) = (captures(), h.engine.status().frames_captured);
    h.engine.shutdown();
    let (captures, frames) = (c1 - c0, f1 - f0);
    assert!(frames >= 20, "{frames} frames in a second at 30 fps");
    assert!(
        captures <= frames + frames / 2,
        "{captures} capture requests for {frames} encoded frames"
    );
}

#[test]
fn a_static_screen_after_a_mode_switch_keeps_capturing() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    // After a mode switch the backend has no frame of its new buffers to
    // repeat while the compositor holds its request on a static screen,
    // here for about two seconds. The engine re-sends what it last encoded
    // instead of ending the generation (which, three times over, pushed
    // capture off ext-image-copy and zero copy).
    let mut script = wlr_ok(160, 90);
    script.unchanged = Some((10, 60));
    let mut h = harness(config(|c| c.fps = 30), script);
    h.engine.start().unwrap();
    let started = Instant::now();
    while h.script.lock().unwrap().captures < 10 + 60 + 5 {
        assert!(started.elapsed() < Duration::from_secs(20), "stalled");
        std::thread::sleep(Duration::from_millis(10));
    }
    let s = h.engine.status();
    h.engine.shutdown();
    assert_eq!(s.capture_generation, 1, "{s:?}");
    assert!(
        s.frames_captured >= 60,
        "{} frames: the static stretch was not encoded",
        s.frames_captured
    );
    assert!(h.sleeps.lock().unwrap().is_empty(), "no recovery");
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
    // The watchdog samples the surfaces once a second, from a mapping.
    wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.frames_captured >= 30 && s.content_sample_sequence >= 1
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
fn zero_copy_that_is_never_filled_falls_back_without_a_restart() {
    let Some(c) = vaapi_dmabuf_constraints(320, 180) else {
        return eprintln!("skipping: no VA-API device");
    };
    // The compositor takes the dmabuf images and completes no capture into
    // them: the backend repeats its shm frame until the first-frame
    // deadline, then goes back to shm on its own. The engine frees the
    // surfaces and keeps the generation.
    let mut script = ext_dmabuf(320, 180, c);
    script.dmabuf_never_filled = true;
    let mut h = harness(vaapi_config(), script);
    let started = Instant::now();
    h.engine.start().unwrap();
    wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.frames_captured >= 5
    });
    assert!(h.engine.zero_copy_in_use(), "zero copy started");
    let deadline = Instant::now() + DMABUF_FIRST_FRAME_DEADLINE + Duration::from_secs(5);
    while h.engine.zero_copy_in_use() {
        assert!(Instant::now() < deadline, "zero copy never fell back");
        std::thread::sleep(Duration::from_millis(50));
    }
    let fell_back_after = started.elapsed();
    let frames = h.engine.status().frames_captured;
    let s = wait_for(&h.engine, Duration::from_secs(5), |s| {
        s.frames_captured >= frames + 15
    });
    h.engine.shutdown();
    assert!(
        fell_back_after >= DMABUF_FIRST_FRAME_DEADLINE,
        "fell back after {fell_back_after:?}"
    );
    assert_eq!(s.capture_generation, 1, "{s:?}");
    assert_eq!(h.script.lock().unwrap().dmabuf_uses, 1, "no reallocation");
    assert!(h.sleeps.lock().unwrap().is_empty(), "no recovery");
}

#[test]
fn zero_copy_survives_a_static_screen_past_the_first_frame_deadline() {
    let Some(c) = vaapi_dmabuf_constraints(320, 180) else {
        return eprintln!("skipping: no VA-API device");
    };
    // The app starts on an idle desktop: the screen never changes. The
    // switch to dmabuf starts a fresh session whose first capture comes
    // anyway; every later one is held and repeated. Zero copy must stay.
    let mut script = ext_dmabuf(320, 180, c);
    script.static_after = Some(0);
    let mut h = harness(vaapi_config(), script);
    h.engine.start().unwrap();
    std::thread::sleep(DMABUF_FIRST_FRAME_DEADLINE + Duration::from_secs(1));
    let s = h.engine.status();
    let zero_copy = h.engine.zero_copy_in_use();
    h.engine.shutdown();
    assert!(zero_copy, "zero copy was dropped on a static screen: {s:?}");
    assert!(s.frames_captured >= 100, "{s:?}");
    assert_eq!(s.capture_generation, 1, "{s:?}");
    assert_eq!(h.script.lock().unwrap().dmabuf_uses, 1);
    assert!(h.sleeps.lock().unwrap().is_empty(), "no recovery");
}

/// Starts `script` (synthetic ext with dmabuf) and waits for the black
/// zero-copy run to restart capture; `None` when this driver's new VA
/// surfaces are not black, so the run never comes.
fn through_a_black_zero_copy_restart(script: SyntheticScript) -> Option<Harness> {
    let mut h = harness(vaapi_config(), script);
    h.engine.start().unwrap();
    let s = wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.content_sample_sequence >= 2
    });
    if s.content_suspicious_streak == 0 {
        h.engine.shutdown();
        eprintln!("skipping: this driver's new surfaces are not black");
        return None;
    }
    assert!(h.engine.zero_copy_in_use(), "zero copy started");
    wait_for(&h.engine, Duration::from_secs(30), |s| {
        s.capture_generation == 2
    });
    Some(h)
}

#[test]
fn a_black_zero_copy_feed_that_shm_shows_fine_stays_on_shm() {
    let Some(c) = vaapi_dmabuf_constraints(320, 180) else {
        return eprintln!("skipping: no VA-API device");
    };
    // The synthetic compositor completes dmabuf captures without drawing
    // into the images, so the watchdog reads the surfaces as the driver
    // allocated them; its shm frames show a picture. The content restart
    // tries shared memory, whose first sample is fine: the zero-copy feed
    // was the problem, and zero copy stays off for this capture start,
    // through later generations too (the third comes from a scripted
    // capture error 450 frames into the second; the first restarts on its
    // dark run at 360, twelve samples of 30 frames).
    let mut script = ext_dmabuf(320, 180, c);
    script
        .kinds
        .get_mut(&BackendKind::ExtImageCopy)
        .unwrap()
        .capture_error_after = Some((450, CaptureError::Failed("scripted".into())));
    let Some(mut h) = through_a_black_zero_copy_restart(script) else {
        return;
    };
    // A sample of the new generation's picture clears the flag.
    let s = wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.health_flags & flags::CONTENT_SUSPECT == 0
    });
    assert_eq!(s.capture_generation, 2, "{s:?}");
    assert!(
        !h.engine.zero_copy_in_use(),
        "the restart went back into zero copy"
    );
    let s = wait_for(&h.engine, Duration::from_secs(30), |s| {
        s.capture_generation == 3
    });
    let frames = s.frames_captured;
    wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.frames_captured >= frames + 60
    });
    let zero_copy = h.engine.zero_copy_in_use();
    h.engine.shutdown();
    assert!(!zero_copy, "a later generation went back into zero copy");
    assert_eq!(h.script.lock().unwrap().dmabuf_uses, 1, "dmabuf again");
}

#[test]
fn a_dark_scene_dark_through_shm_too_goes_back_to_zero_copy() {
    let Some(c) = vaapi_dmabuf_constraints(320, 180) else {
        return eprintln!("skipping: no VA-API device");
    };
    // A black loading screen: the shm frames are black too. The content
    // restart tries shared memory, whose first sample is still dark: the
    // darkness is real, and zero copy comes back in the same generation
    // instead of copying every frame until the picture changes.
    let mut script = ext_dmabuf(320, 180, c);
    script.black = true;
    let Some(mut h) = through_a_black_zero_copy_restart(script) else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while !h.engine.zero_copy_in_use() {
        assert!(Instant::now() < deadline, "zero copy did not come back");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Two more samples: no second restart while the picture stays dark.
    let n = h.engine.status().content_sample_sequence;
    let s = wait_for(&h.engine, Duration::from_secs(10), |s| {
        s.content_sample_sequence >= n + 2
    });
    let zero_copy = h.engine.zero_copy_in_use();
    h.engine.shutdown();
    assert!(zero_copy, "{s:?}");
    assert_eq!(s.capture_generation, 2, "{s:?}");
    assert_ne!(
        s.health_flags & flags::CONTENT_SUSPECT,
        0,
        "still dark: {s:?}"
    );
    assert_eq!(
        h.script.lock().unwrap().dmabuf_uses,
        2,
        "zero copy once more"
    );
    assert_eq!(*h.sleeps.lock().unwrap(), [250]);
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
