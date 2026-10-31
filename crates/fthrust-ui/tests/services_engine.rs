//! Plan 5 Task 9: the engine supervisor state machine and the status table.

use fthrust_core::capture_config::CaptureConfig;
use fthrust_core::capture_health::{HealthState, flags};
use fthrust_ipc::{EngineConfig, StatusSnapshot};
use fthrust_ui::services::engine::{Effect, Input, Supervisor};
use fthrust_ui::services::status::{focus_paused, style_for, text_for};
use fthrust_ui::services::{Event, Request};
use fthrust_ui::shell::error_bar::Level;
use fthrust_ui::state::StatusStyle;

fn config() -> EngineConfig {
    let d = tempfile::tempdir().unwrap();
    let s = fthrust_ui::state::AppState::for_tests(d.path(), serde_json::json!({}));
    EngineConfig {
        capture: CaptureConfig::from_settings(&s.settings).unwrap(),
        max_buffer_mb: 256,
        backend_override: None,
    }
}

fn status(frames: u64, flags: u32, generation: u32) -> StatusSnapshot {
    StatusSnapshot {
        frames_captured: frames,
        health_flags: flags,
        capture_generation: generation,
        active_codec: "h264_vaapi".into(),
        ..StatusSnapshot::default()
    }
}

fn status_events(effects: &[Effect]) -> Vec<(String, StatusStyle)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Ui(Event::Status { text, style }) => Some((text.clone(), *style)),
            _ => None,
        })
        .collect()
}

fn error_titles(effects: &[Effect]) -> Vec<(String, Option<String>, Level)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Ui(Event::ErrorBar {
                title, key, level, ..
            }) => Some((title.clone(), key.clone(), *level)),
            _ => None,
        })
        .collect()
}

#[test]
fn status_table_matches_reference() {
    assert_eq!(text_for(HealthState::Initializing), "STARTING CAPTURE");
    assert_eq!(text_for(HealthState::Healthy), "CAPTURING");
    assert_eq!(text_for(HealthState::Degraded), "CAPTURE DEGRADED");
    assert_eq!(
        text_for(HealthState::ContentSuspect),
        "CAPTURE DEGRADED — DARK / UNIFORM"
    );
    assert_eq!(text_for(HealthState::Stalled), "CAPTURE STALLED");
    assert_eq!(text_for(HealthState::Failed), "CAPTURE FAILED");
    assert_eq!(text_for(HealthState::Recovering), "RECOVERING CAPTURE");
    assert_eq!(text_for(HealthState::Stopped), "CAPTURE STOPPED");
    assert_eq!(style_for(HealthState::Healthy, false), StatusStyle::Active);
    assert_eq!(style_for(HealthState::Degraded, false), StatusStyle::Active);
    assert_eq!(
        style_for(HealthState::ContentSuspect, false),
        StatusStyle::Active
    );
    assert_eq!(
        style_for(HealthState::Initializing, false),
        StatusStyle::Idle
    );
    assert_eq!(style_for(HealthState::Recovering, false), StatusStyle::Idle);
    assert_eq!(style_for(HealthState::Stalled, false), StatusStyle::Warning);
    assert_eq!(
        style_for(HealthState::Healthy, true),
        StatusStyle::Idle,
        "idle while applying"
    );
    assert!(focus_paused(
        HealthState::Degraded,
        flags::ACTIVE | flags::PAUSED
    ));
    assert!(!focus_paused(HealthState::Healthy, flags::PAUSED));
}

#[test]
fn handshake_then_healthy_activates_config() {
    let mut sv = Supervisor::new();
    let fx = sv.handle(Input::Start(config()), 0.0);
    assert!(fx.iter().any(|e| matches!(e, Effect::Spawn)), "{fx:?}");
    assert_eq!(
        status_events(&fx),
        vec![("CONNECTING".to_string(), StatusStyle::Idle)]
    );
    let fx = sv.handle(Input::Spawned { pid: 4242 }, 0.1);
    assert!(fx.iter().any(|e| matches!(e, Effect::Connect)));
    let fx = sv.handle(
        Input::Hello {
            pid: 4242,
            version: "x".into(),
        },
        0.5,
    );
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::Send(fthrust_ipc::Command::Configure(_)))),
        "{fx:?}"
    );
    assert_eq!(
        status_events(&fx),
        vec![("STARTING CAPTURE".to_string(), StatusStyle::Idle)]
    );
    let fx = sv.handle(
        Input::Configured {
            warnings: vec!["fps 999 is out of range; using 360".into()],
        },
        0.6,
    );
    assert!(
        error_titles(&fx)
            .iter()
            .any(|(t, _, l)| t == "CAPTURE SETTINGS ADJUSTED" && *l == Level::Warning)
    );
    let mut now = 1.0;
    let mut frames = 0;
    let mut last = Vec::new();
    for _ in 0..4 {
        frames += 30;
        last = sv.handle(Input::Status(status(frames, flags::ACTIVE, 1)), now);
        now += 0.5;
    }
    assert_eq!(sv.status_text(), Some(("CAPTURING", StatusStyle::Active)));
    assert!(
        sv.active_config().is_some(),
        "the launch config was activated"
    );
    assert!(
        last.iter()
            .any(|e| matches!(e, Effect::Ui(Event::EncoderActive(c)) if c == "h264_vaapi"))
            || sv.active_codec() == Some("h264_vaapi")
    );
    assert!(sv.is_connected());
}

#[test]
fn startup_error_maps_to_error_001_or_004() {
    let mut sv = Supervisor::new();
    sv.handle(Input::Start(config()), 0.0);
    sv.handle(Input::Spawned { pid: 1 }, 0.1);
    sv.handle(
        Input::Hello {
            pid: 1,
            version: "x".into(),
        },
        0.2,
    );
    let fx = sv.handle(
        Input::StartupError {
            code: "CAPTURE_BACKEND_UNAVAILABLE".into(),
            detail: "no backend".into(),
        },
        0.3,
    );
    let errors = error_titles(&fx);
    assert!(
        errors.iter().any(|(t, _, _)| t == "CAPTURE FAILED"),
        "{errors:?}"
    );
    let mut sv = Supervisor::new();
    sv.handle(Input::Start(config()), 0.0);
    sv.handle(Input::Spawned { pid: 2 }, 0.1);
    let fx = sv.handle(
        Input::StartupError {
            code: "ENGINE_START_FAILED".into(),
            detail: "boom".into(),
        },
        0.3,
    );
    assert!(
        error_titles(&fx)
            .iter()
            .any(|(t, _, _)| t == "ENGINE NOT RESPONDING")
    );
    // No Hello within 9 s: ENGINE NOT RESPONDING and the process is stopped.
    let mut sv = Supervisor::new();
    sv.handle(Input::Start(config()), 0.0);
    sv.handle(Input::Spawned { pid: 3 }, 0.1);
    let fx = sv.handle(Input::Tick, 9.5);
    assert!(
        error_titles(&fx)
            .iter()
            .any(|(t, _, _)| t == "ENGINE NOT RESPONDING"),
        "{fx:?}"
    );
    assert!(fx.iter().any(|e| matches!(e, Effect::Kill { .. })));
    // Missing binary.
    let mut sv = Supervisor::new();
    let fx = sv.handle(
        Input::SpawnFailed {
            detail: "engine executable not found".into(),
            missing: true,
        },
        0.0,
    );
    assert!(
        error_titles(&fx)
            .iter()
            .any(|(t, k, _)| t == "ENGINE NOT FOUND" && k.as_deref() == Some("engine-launch"))
    );
    assert_eq!(
        status_events(&fx).last().map(|s| s.0.as_str()),
        Some("NO ENGINE")
    );
}

fn bring_up(sv: &mut Supervisor, pid: u32, now: &mut f64) {
    sv.handle(Input::Start(config()), *now);
    sv.handle(Input::Spawned { pid }, *now);
    sv.handle(
        Input::Hello {
            pid,
            version: "x".into(),
        },
        *now,
    );
    sv.handle(Input::Configured { warnings: vec![] }, *now);
    let mut frames = 0;
    for _ in 0..4 {
        *now += 0.5;
        frames += 30;
        sv.handle(Input::Status(status(frames, flags::ACTIVE, 1)), *now);
    }
}

#[test]
fn process_exit_pushes_engine_stopped_and_restart_recovers() {
    let mut sv = Supervisor::new();
    let mut now = 0.0;
    bring_up(&mut sv, 10, &mut now);
    // A backend failure, then the engine exits with 0.
    now += 0.5;
    let fx = sv.handle(Input::Status(status(120, flags::BACKEND_FAILED, 1)), now);
    assert!(
        error_titles(&fx)
            .iter()
            .any(|(t, k, _)| t == "CAPTURE FAILED" && k.as_deref() == Some("capture-backend"))
    );
    now += 0.5;
    let fx = sv.handle(Input::ProcessExited { code: Some(0) }, now);
    let detail = fx.iter().find_map(|e| match e {
        Effect::Ui(Event::ErrorBar {
            title, detail, key, ..
        }) if title == "ENGINE STOPPED" => Some((detail.clone(), key.clone())),
        _ => None,
    });
    assert_eq!(detail, Some(("The capture engine stopped after the capture failure above. Clips cannot be saved until it is restarted.".into(), Some("engine-process-exit".into()))));
    assert_eq!(
        status_events(&fx).last().map(|s| (s.0.as_str(), s.1)),
        Some(("ENGINE STOPPED", StatusStyle::Warning))
    );
    assert!(!sv.is_connected());
    // Save is refused while stopped (Review Focus 2).
    assert!(!sv.save_allowed());
    // Restart: a new generation comes up; the incident clears after 3 progressing polls over 3 s.
    now += 0.1;
    let fx = sv.handle(
        Input::Restart {
            config: config(),
            allow_recording_prepare: false,
            explicit: true,
        },
        now,
    );
    assert!(
        fx.iter().any(|e| matches!(e, Effect::StartAfter(_))),
        "{fx:?}"
    );
    assert_eq!(
        status_events(&fx).last().map(|s| s.0.as_str()),
        Some("RESTARTING ENGINE")
    );
    now += 1.1;
    let fx = sv.handle(Input::Tick, now);
    assert!(fx.iter().any(|e| matches!(e, Effect::Spawn)), "{fx:?}");
    sv.handle(Input::Spawned { pid: 11 }, now);
    sv.handle(
        Input::Hello {
            pid: 11,
            version: "x".into(),
        },
        now,
    );
    sv.handle(Input::Configured { warnings: vec![] }, now);
    let mut cleared = false;
    let mut frames = 0;
    for _ in 0..10 {
        now += 0.5;
        frames += 30;
        let fx = sv.handle(Input::Status(status(frames, flags::ACTIVE, 2)), now);
        if fx.iter().any(
            |e| matches!(e, Effect::Ui(Event::IncidentCleared(k)) if k == "engine-process-exit"),
        ) {
            cleared = true;
            break;
        }
    }
    assert!(
        cleared,
        "the engine-process-exit incident clears once the new engine is stable"
    );
    assert!(sv.save_allowed());
    // An unexpected exit of a healthy engine names the code.
    let fx = sv.handle(Input::ProcessExited { code: Some(137) }, now + 1.0);
    let detail = fx.iter().find_map(|e| match e {
        Effect::Ui(Event::ErrorBar { title, detail, .. }) if title == "ENGINE STOPPED" => {
            Some(detail.clone())
        }
        _ => None,
    });
    assert_eq!(
        detail.as_deref(),
        Some(
            "The capture engine exited unexpectedly (code 137). Clips cannot be saved until it is restarted."
        )
    );
}

#[test]
fn restart_deferred_while_save_busy() {
    let mut sv = Supervisor::new();
    let mut now = 0.0;
    bring_up(&mut sv, 20, &mut now);
    sv.set_save_busy(true);
    let fx = sv.handle(
        Input::Restart {
            config: config(),
            allow_recording_prepare: false,
            explicit: false,
        },
        now,
    );
    assert!(
        !fx.iter().any(|e| matches!(e, Effect::Kill { .. })),
        "no stop while a save is in flight"
    );
    assert_eq!(
        status_events(&fx).last().map(|s| s.0.as_str()),
        Some("APPLYING SETTINGS")
    );
    let fx = sv.handle(Input::Tick, now + 0.3);
    assert!(!fx.iter().any(|e| matches!(e, Effect::Kill { .. })));
    sv.set_save_busy(false);
    let fx = sv.handle(Input::Tick, now + 0.6);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::Kill { .. })),
        "{fx:?}"
    );
    // A second restart while one is pending is ignored.
    let fx = sv.handle(
        Input::Restart {
            config: config(),
            allow_recording_prepare: false,
            explicit: false,
        },
        now + 0.7,
    );
    assert!(!fx.iter().any(|e| matches!(e, Effect::Kill { .. })));
    // Recording blocks restarts.
    let mut sv = Supervisor::new();
    let mut now = 0.0;
    bring_up(&mut sv, 21, &mut now);
    sv.set_recording(true);
    let fx = sv.handle(
        Input::Restart {
            config: config(),
            allow_recording_prepare: false,
            explicit: false,
        },
        now,
    );
    assert!(
        error_titles(&fx)
            .iter()
            .any(|(t, _, l)| t == "STOP RECORDING FIRST" && *l == Level::Warning)
    );
}

#[test]
fn apply_timeout_after_30s() {
    // Only a settings apply (restart) arms the 30 s timer, as in the reference.
    let mut sv = Supervisor::new();
    let mut now = 0.0;
    bring_up(&mut sv, 30, &mut now);
    let t0 = now;
    sv.handle(
        Input::Restart {
            config: config(),
            allow_recording_prepare: false,
            explicit: false,
        },
        now,
    );
    sv.handle(Input::ProcessExited { code: Some(0) }, now + 0.1);
    now += 1.2;
    let fx = sv.handle(Input::Tick, now);
    assert!(fx.iter().any(|e| matches!(e, Effect::Spawn)), "{fx:?}");
    sv.handle(Input::Spawned { pid: 31 }, now);
    sv.handle(
        Input::Hello {
            pid: 31,
            version: "x".into(),
        },
        now,
    );
    sv.handle(Input::Configured { warnings: vec![] }, now);
    // The engine keeps recovering (never healthy, never stalled), so the
    // only way out is the 30 s apply timeout.
    let mut hit = false;
    while now < t0 + 35.0 {
        now += 0.5;
        let fx = sv.handle(
            Input::Status(status(0, flags::ACTIVE | flags::RECOVERING, 2)),
            now,
        );
        if error_titles(&fx).iter().any(|(t, k, _)| {
            t == "SETTINGS APPLY TIMED OUT" && k.as_deref() == Some("capture-apply-timeout")
        }) {
            hit = true;
            assert!(now >= t0 + 30.0 && now < t0 + 31.0, "at {now} (t0 {t0})");
            break;
        }
    }
    assert!(hit);
}

#[test]
fn silence_3s_is_disconnected() {
    let mut sv = Supervisor::new();
    let mut now = 0.0;
    bring_up(&mut sv, 40, &mut now);
    let fx = sv.handle(Input::Tick, now + 1.0);
    assert!(
        status_events(&fx).is_empty()
            || status_events(&fx).last().map(|s| s.0.as_str()) == Some("CAPTURING")
    );
    let fx = sv.handle(Input::Tick, now + 3.5);
    assert_eq!(
        status_events(&fx).last().map(|s| (s.0.as_str(), s.1)),
        Some(("DISCONNECTED", StatusStyle::Warning))
    );
    let _ = Request::RestartEngine;
}

#[test]
fn synthetic_engine_round_trip() {
    let Some(engine) = fthrust_ui::services::engine::discover_engine(None) else {
        eprintln!("fthrust-engine binary not built; skipped");
        return;
    };
    let d = tempfile::tempdir().unwrap();
    let mut s = fthrust_ui::state::AppState::for_tests(
        d.path(),
        serde_json::json!({"encoder_pref": "software"}),
    );
    let ctx = egui::Context::default();
    let mut services = fthrust_ui::services::Services::start(&s, &ctx);
    let runtime = d.path().join("rt");
    std::fs::create_dir_all(&runtime).unwrap();
    services.start_engine_with(
        engine,
        runtime.join("engine.sock"),
        vec![
            ("FTHRUST_SYNTHETIC_BACKEND".into(), "1".into()),
            ("FTHR_CAPTURE_BACKEND".into(), "synthetic".into()),
        ],
        &s,
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut capturing = false;
    while std::time::Instant::now() < deadline {
        services.tick(&mut s);
        if s.status.text == "CAPTURING" {
            capturing = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        capturing,
        "status reached CAPTURING through the synthetic engine (last: {})",
        s.status.text
    );
    assert!(s.active_encoder.is_some());
    // A restart (settings apply) relaunches the daemon and comes back healthy.
    s.engine_restart_requested = true;
    services.tick(&mut s);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut left = false;
    let mut back = false;
    while std::time::Instant::now() < deadline {
        services.tick(&mut s);
        if !left && s.status.text != "CAPTURING" {
            left = true;
        }
        if left && s.status.text == "CAPTURING" {
            back = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(left, "the restart changed the status");
    assert!(
        back,
        "the engine relaunched after the restart (last: {})",
        s.status.text
    );
    services.kill_engine_for_tests();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut stopped = false;
    while std::time::Instant::now() < deadline {
        services.tick(&mut s);
        if s.status.text == "ENGINE STOPPED" {
            stopped = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        stopped,
        "a killed engine is reported (last: {})",
        s.status.text
    );
    assert!(s.error_bar.visible());
    services.shutdown();
}

#[test]
fn status_frames_forward_health_snapshots() {
    let mut sv = Supervisor::new();
    sv.handle(Input::Start(config()), 0.0);
    sv.handle(Input::Spawned { pid: 1 }, 0.1);
    sv.handle(
        Input::Hello {
            pid: 1,
            version: "x".into(),
        },
        0.2,
    );
    sv.handle(Input::Configured { warnings: vec![] }, 0.3);
    let mut now = 0.4;
    let mut last = Vec::new();
    for frames in 1..40u64 {
        now += 0.1;
        last = sv.handle(Input::Status(status(frames * 6, flags::ACTIVE, 1)), now);
    }
    let health: Vec<_> = last
        .iter()
        .filter_map(|e| match e {
            Effect::Ui(fthrust_ui::services::Event::Health(h)) => Some(h.state),
            _ => None,
        })
        .collect();
    assert_eq!(
        health,
        vec![fthrust_core::capture_health::HealthState::Healthy],
        "{last:?}"
    );
}

#[test]
fn launch_success_publishes_active_config() {
    let mut sv = Supervisor::new();
    sv.handle(Input::Start(config()), 0.0);
    sv.handle(Input::Spawned { pid: 1 }, 0.1);
    sv.handle(
        Input::Hello {
            pid: 1,
            version: "x".into(),
        },
        0.2,
    );
    sv.handle(Input::Configured { warnings: vec![] }, 0.3);
    let mut now = 0.4;
    let mut published = None;
    for frames in 1..40u64 {
        now += 0.1;
        let fx = sv.handle(Input::Status(status(frames * 6, flags::ACTIVE, 1)), now);
        for e in fx {
            if let Effect::Ui(fthrust_ui::services::Event::ActiveConfig(c)) = e {
                published = Some(c);
            }
        }
    }
    assert_eq!(
        published,
        Some(Some(config().capture)),
        "the applied capture config is published once the launch is healthy"
    );
    let fx = sv.handle(Input::ProcessExited { code: Some(1) }, now + 1.0);
    assert!(
        fx.iter().any(|e| matches!(
            e,
            Effect::Ui(fthrust_ui::services::Event::ActiveConfig(None))
        )),
        "an exited engine clears the active config: {fx:?}"
    );
}

/// Audit 2026-10-31 tier 1 #3: quitting mid-save gives the engine time to
/// finish the clip instead of the usual two seconds.
#[test]
fn stop_mid_save_waits_for_the_engine() {
    use fthrust_ui::services::engine::{KILL_GRACE_S, SAVE_EXIT_GRACE_S};
    let grace = |fx: &[Effect]| {
        fx.iter().find_map(|e| match e {
            Effect::Kill { grace_s, .. } => Some(*grace_s),
            _ => None,
        })
    };
    let mut sv = Supervisor::new();
    let mut now = 0.0;
    bring_up(&mut sv, 30, &mut now);
    sv.set_save_busy(true);
    assert_eq!(grace(&sv.handle(Input::Stop, now)), Some(SAVE_EXIT_GRACE_S));
    let mut sv = Supervisor::new();
    let mut now = 0.0;
    bring_up(&mut sv, 31, &mut now);
    let fx = sv.handle(Input::Stop, now);
    assert_eq!(grace(&fx), Some(KILL_GRACE_S));
    // From the moment it stops, the pages see the engine as gone, so no
    // save is admitted to a daemon that is shutting down.
    assert!(
        fx.contains(&Effect::Ui(Event::EngineConnected(false))),
        "{fx:?}"
    );
}

/// The driver's shutdown waits for the process to exit on its own (here a
/// stand-in engine that takes 2.5 s and ignores SIGTERM) instead of
/// returning at once and letting the runtime's shutdown SIGKILL it.
#[test]
fn driver_shutdown_waits_for_the_engine_to_finish() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let done = d.path().join("finished");
    let script = d.path().join("slow-engine");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ntrap '' TERM\nsleep 2.5\ntouch '{}'\n",
            done.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (tx, _rx) = std::sync::mpsc::channel();
    let outbox = fthrust_ui::services::Outbox::new(tx, egui::Context::default());
    let driver = fthrust_ui::services::engine::EngineDriver::start(
        rt.handle().clone(),
        outbox,
        script,
        d.path().join("engine.sock"),
        Vec::new(),
        d.path().join("engine.log"),
    );
    driver.request(Input::Start(config()));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !driver.process_running() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(driver.process_running(), "the stand-in engine started");
    driver.supervisor().lock().unwrap().set_save_busy(true);
    let started = std::time::Instant::now();
    driver.shutdown();
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(2),
        "shutdown returned after {:?}, before the engine finished",
        started.elapsed()
    );
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
    assert!(done.is_file(), "the engine ran to completion, not killed");
}

fn config_fps(fps: u32) -> EngineConfig {
    let mut c = config();
    c.capture.fps = fps;
    c
}

fn configured_fps(fx: &[Effect]) -> Option<u32> {
    fx.iter().find_map(|e| match e {
        Effect::Send(fthrust_ipc::Command::Configure(c)) => Some(c.capture.fps),
        _ => None,
    })
}

fn restart(fps: u32) -> Input {
    Input::Restart {
        config: config_fps(fps),
        allow_recording_prepare: false,
        explicit: false,
    }
}

/// Audit 2026-10-31: settings applied while a restart is pending or
/// deferred used to be dropped (the first config won).
#[test]
fn the_newest_settings_win_while_a_restart_is_deferred() {
    let mut sv = Supervisor::new();
    let mut now = 0.0;
    bring_up(&mut sv, 40, &mut now);
    sv.set_save_busy(true);
    sv.handle(restart(90), now);
    sv.handle(restart(120), now + 0.1);
    sv.set_save_busy(false);
    let mut fx = sv.handle(Input::Tick, now + 0.7);
    // Stopped, then started again after the gap with the newest settings.
    fx.extend(sv.handle(Input::ProcessExited { code: Some(0) }, now + 0.8));
    fx.extend(sv.handle(Input::Tick, now + 2.0));
    fx.extend(sv.handle(Input::Spawned { pid: 41 }, now + 2.1));
    fx.extend(sv.handle(
        Input::Hello {
            pid: 41,
            version: "x".into(),
        },
        now + 2.2,
    ));
    assert_eq!(configured_fps(&fx), Some(120), "{fx:?}");
}

#[test]
fn settings_applied_during_a_restart_are_not_dropped() {
    // Before the new process is configured: it starts with the newest.
    let mut sv = Supervisor::new();
    let mut now = 0.0;
    bring_up(&mut sv, 50, &mut now);
    sv.handle(restart(90), now);
    sv.handle(restart(144), now + 0.1);
    let mut fx = sv.handle(Input::ProcessExited { code: Some(0) }, now + 0.2);
    fx.extend(sv.handle(Input::Tick, now + 1.5));
    fx.extend(sv.handle(Input::Spawned { pid: 51 }, now + 1.6));
    fx.extend(sv.handle(
        Input::Hello {
            pid: 51,
            version: "x".into(),
        },
        now + 1.7,
    ));
    assert_eq!(configured_fps(&fx), Some(144), "{fx:?}");
    // After it was configured: one more restart once the apply settles.
    sv.handle(restart(165), now + 1.8);
    let mut frames = 0;
    let mut t = now + 1.8;
    let mut fx = Vec::new();
    for _ in 0..6 {
        t += 0.5;
        frames += 30;
        fx.extend(sv.handle(Input::Status(status(frames, flags::ACTIVE, 1)), t));
    }
    fx.extend(sv.handle(Input::Tick, t + 0.1));
    assert!(
        fx.iter().any(|e| matches!(e, Effect::Kill { .. })),
        "the queued restart ran: {fx:?}"
    );
}

/// The engine's stderr is read to the end even past bytes that aren't
/// UTF-8 (the reader used to stop, and the engine died on EPIPE).
#[test]
fn stderr_reader_survives_bad_bytes() {
    use fthrust_ui::services::engine::{pump_stderr, startup_error_line};
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let bytes: &[u8] = b"starting\n\xff\xfe broken\nFTHR_STARTUP_ERROR: NO_OUTPUT: none found\n";
    let mut lines = Vec::new();
    rt.block_on(pump_stderr(bytes, |l| lines.push(l.to_owned())));
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[1].ends_with(" broken"));
    match startup_error_line(&lines[2]) {
        Some(Input::StartupError { code, detail }) => {
            assert_eq!(code, "NO_OUTPUT");
            assert_eq!(detail, "none found");
        }
        other => panic!("{other:?}"),
    }
}

/// Audit 2026-10-31: a crashed engine comes back on its own, a few times.
#[test]
fn a_crashed_engine_restarts_within_a_budget() {
    use fthrust_ui::services::engine::{AUTO_RESTART_DELAY_S, AUTO_RESTART_LIMIT};
    let mut sv = Supervisor::new();
    let mut now = 0.0;
    let mut pid = 60;
    bring_up(&mut sv, pid, &mut now);
    for round in 0..=AUTO_RESTART_LIMIT {
        now += 1.0;
        let fx = sv.handle(Input::ProcessExited { code: Some(139) }, now);
        assert!(
            error_titles(&fx)
                .iter()
                .any(|(t, _, _)| t == "ENGINE STOPPED"),
            "the crash is still reported"
        );
        let fx = sv.handle(Input::Tick, now + AUTO_RESTART_DELAY_S + 0.01);
        let spawned = fx.iter().any(|e| matches!(e, Effect::Spawn));
        if round < AUTO_RESTART_LIMIT {
            assert!(spawned, "round {round}: restarted on its own: {fx:?}");
            now += AUTO_RESTART_DELAY_S + 0.01;
            pid += 1;
            sv.handle(Input::Spawned { pid }, now);
            sv.handle(
                Input::Hello {
                    pid,
                    version: "x".into(),
                },
                now,
            );
        } else {
            assert!(!spawned, "the budget is spent: stays stopped");
        }
    }
}

/// A silent engine stops admitting saves at 3 s and is restarted at 15 s.
#[test]
fn a_silent_engine_is_reported_then_restarted() {
    use fthrust_ui::services::engine::HANG_RESTART_S;
    let mut sv = Supervisor::new();
    let mut now = 0.0;
    bring_up(&mut sv, 70, &mut now);
    let fx = sv.handle(Input::Tick, now + 3.1);
    assert!(
        fx.iter().any(|e| matches!(
            e,
            Effect::Ui(Event::Health(h)) if h.state != HealthState::Healthy
        )),
        "save admission learns it is silent: {fx:?}"
    );
    let fx = sv.handle(Input::Tick, now + HANG_RESTART_S + 0.1);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::Kill { .. })),
        "{fx:?}"
    );
    assert!(fx.iter().any(|e| matches!(e, Effect::StartAfter(_))));
}

#[test]
fn the_supervisor_sleeps_until_its_next_timer() {
    let mut sv = Supervisor::new();
    assert_eq!(sv.next_deadline(), None, "nothing to do while stopped");
    let mut now = 0.0;
    bring_up(&mut sv, 80, &mut now);
    let at = sv.next_deadline().expect("connected: the silence check");
    assert!(at > now && at <= now + 3.0, "{at} vs {now}");
}

/// Audit 2026-10-31: settings the engine can't start with are reported
/// instead of leaving "STARTING CAPTURE" up forever.
#[test]
fn unusable_capture_settings_are_reported_at_start() {
    let d = tempfile::tempdir().unwrap();
    let mut s =
        fthrust_ui::state::AppState::for_tests(d.path(), serde_json::json!({"framerate": 7}));
    let ctx = egui::Context::default();
    let mut services = fthrust_ui::services::Services::start(&s, &ctx);
    services.start_engine_with(
        d.path().join("no-such-engine"),
        d.path().join("engine.sock"),
        Vec::new(),
        &s,
    );
    services.tick(&mut s);
    assert_eq!(s.status.text, "CAPTURE FAILED");
    assert!(
        s.error_bar
            .entries()
            .iter()
            .any(|e| e.title == "SETTINGS APPLY FAILED"),
        "{:?}",
        s.error_bar.entries()
    );
    services.shutdown();
}
