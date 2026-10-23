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
    let mut s = fthrust_ui::state::AppState::for_tests(d.path(), serde_json::json!({}));
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
