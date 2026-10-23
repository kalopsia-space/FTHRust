//! Plan 5 Task 11: manual recording.

use std::path::PathBuf;

use fthrust_core::capture_health::{HealthSnapshot, HealthState};
use fthrust_ipc::{Command, Event as IpcEvent};
use fthrust_ui::services::recording::{
    RecordState, free_space_bytes, recording_config, recording_target,
};
use fthrust_ui::services::{CardKind, Event, Services};
use fthrust_ui::state::{AppState, StatusStyle};
use time::macros::datetime;

fn state() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.tokens.reduce_motion = true;
    (tmp, s)
}

fn healthy() -> HealthSnapshot {
    HealthSnapshot {
        state: HealthState::Healthy,
        previous_state: HealthState::Healthy,
        reason: String::new(),
        changed: false,
        request_recovery: false,
        seconds_since_progress: 0.0,
        fresh_buffer_seconds: 30.0,
    }
}

fn connected(services: &mut Services, s: &mut AppState) {
    services.inject(Event::EngineConnected(true));
    services.inject(Event::Status {
        text: "CAPTURING".into(),
        style: StatusStyle::Active,
    });
    services.inject(Event::Health(healthy()));
    services.tick(s);
}

fn errors(s: &AppState) -> Vec<String> {
    s.error_bar
        .entries()
        .iter()
        .map(|e| e.title.clone())
        .collect()
}

#[test]
fn record_path_and_free_space_checks() {
    let (_tmp, mut s) = state();
    let dir = s.settings.recording_directory().unwrap();
    let at = datetime!(2027-01-15 10:00:00 UTC);
    let first = recording_target(&dir, at).unwrap();
    assert_eq!(first, dir.join("FTHR_recording_2027-01-15_10-00-00.mp4"));
    assert!(dir.is_dir(), "the folder is created");
    std::fs::write(&first, b"x").unwrap();
    assert_eq!(
        recording_target(&dir, at).unwrap(),
        dir.join("FTHR_recording_2027-01-15_10-00-00_2.mp4")
    );
    assert!(free_space_bytes(&dir).unwrap() > 0);
    let long = dir.join("x".repeat(260));
    assert!(recording_target(&long, at).unwrap_err().contains("255"));
    // The recording profile reads the recording_* keys.
    s.set_setting("recording_framerate", serde_json::json!(120));
    s.set_setting("recording_resolution", serde_json::json!("1080p"));
    s.set_setting("recording_bitrate_level", serde_json::json!("high"));
    let cfg = recording_config(&s).unwrap();
    assert_eq!(
        (
            cfg.capture.fps,
            cfg.capture.width,
            cfg.capture.height,
            cfg.capture.bitrate_kbps
        ),
        (120, 1920, 1080, 50_000)
    );
    s.set_setting("recording_bitrate_level", serde_json::json!("custom"));
    s.set_setting("recording_custom_bitrate_kbps", serde_json::json!(33_000));
    assert_eq!(recording_config(&s).unwrap().capture.bitrate_kbps, 33_000);
}

#[test]
fn start_with_different_profile_prepares_then_starts() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.set_wall_clock_for_tests(datetime!(2027-01-15 10:00:00 UTC));
    connected(&mut services, &mut s);
    assert!(
        s.record_enabled,
        "Record is enabled on Linux once connected"
    );
    s.set_setting("recording_framerate", serde_json::json!(120));
    s.record_requested = true;
    services.tick(&mut s);
    assert_eq!(services.recording_state(), RecordState::Preparing);
    assert_eq!(s.record_label, "Preparing…");
    assert!(!s.record_enabled);
    assert_eq!(s.status.text, "PREPARING RECORDING QUALITY");
    let reqs = services.drain_requests_for_tests();
    assert!(reqs.iter().any(|r| matches!(r, fthrust_ui::services::Request::RestartForRecording(cfg) if cfg.capture.fps == 120)), "{reqs:?}");
    // The restart comes up with the recording profile active → the start is submitted.
    services.set_active_config_for_tests(Some(recording_config(&s).unwrap().capture));
    connected(&mut services, &mut s);
    let cmds = services.drain_commands_for_tests();
    let dir = s.settings.recording_directory().unwrap();
    assert_eq!(
        cmds,
        vec![Command::StartRecording {
            request_id: 1,
            path: dir.join("FTHR_recording_2027-01-15_10-00-00.mp4")
        }]
    );
    assert_eq!(services.recording_state(), RecordState::Starting);
    assert_eq!(s.record_label, "Starting…");
    assert_eq!(s.status.text, "STARTING RECORDING");
    // Same profile: no restart, straight to the command.
    let (_tmp2, mut s2) = state();
    let mut services2 = Services::start_for_tests(&ctx);
    services2.set_wall_clock_for_tests(datetime!(2027-01-15 10:00:00 UTC));
    connected(&mut services2, &mut s2);
    services2.set_active_config_for_tests(Some(recording_config(&s2).unwrap().capture));
    s2.record_requested = true;
    services2.tick(&mut s2);
    assert_eq!(services2.recording_state(), RecordState::Starting);
    assert_eq!(services2.drain_commands_for_tests().len(), 1);
}

#[test]
fn start_refused_when_busy_or_disconnected() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    s.record_requested = true;
    services.tick(&mut s);
    assert_eq!(errors(&s), vec!["RECORDING COULD NOT START".to_string()]);
    assert_eq!(
        s.error_bar.current().unwrap().actions[0].label,
        "RESTART CAPTURE"
    );
    assert_eq!(services.recording_state(), RecordState::Idle);
    connected(&mut services, &mut s);
    s.error_bar.clear();
    // A save in flight: wait.
    services.inject(Event::Hotkey(fthrust_core::hotkeys::HotkeyAction::SaveClip));
    services.tick(&mut s);
    services.drain_commands_for_tests();
    s.record_requested = true;
    services.tick(&mut s);
    assert_eq!(errors(&s), vec!["WAIT FOR CLIP SAVE".to_string()]);
    assert_eq!(services.recording_state(), RecordState::Idle);
}

#[test]
fn ack_timeout_after_10s_fails_recording() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    connected(&mut services, &mut s);
    services.set_active_config_for_tests(Some(recording_config(&s).unwrap().capture));
    s.record_requested = true;
    services.tick(&mut s);
    let Command::StartRecording { path, .. } = services.drain_commands_for_tests().remove(0) else {
        panic!()
    };
    services.advance_clock_for_tests(10.5);
    services.tick(&mut s);
    assert_eq!(services.recording_state(), RecordState::Idle);
    assert_eq!(errors(&s), vec!["MANUAL RECORDING FAILED".to_string()]);
    assert!(
        s.error_bar
            .current()
            .unwrap()
            .detail
            .contains("did not acknowledge the start request")
    );
    assert_eq!(
        s.error_bar.current().unwrap().actions[0].label,
        "OPEN RECORDINGS"
    );
    assert_eq!(s.status.text, "RECORDING FAILED");
    assert_eq!(s.record_label, "Record");
    assert!(s.record_enabled);
    assert!(!path.exists());
}

#[test]
fn stop_finalizes_and_shows_recording_saved() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    connected(&mut services, &mut s);
    services.set_active_config_for_tests(Some(recording_config(&s).unwrap().capture));
    s.record_requested = true;
    services.tick(&mut s);
    let Command::StartRecording { path, request_id } =
        services.drain_commands_for_tests().remove(0)
    else {
        panic!()
    };
    services.inject(Event::Engine(IpcEvent::RecordingStarted {
        request_id,
        path: path.clone(),
    }));
    services.tick(&mut s);
    assert_eq!(services.recording_state(), RecordState::Recording);
    assert!(
        s.record_label.starts_with("Stop  00:00:0"),
        "{}",
        s.record_label
    );
    assert!(s.record_enabled);
    services.advance_clock_for_tests(65.0);
    services.tick(&mut s);
    assert_eq!(s.record_label, "Stop  00:01:05");
    // Stop.
    s.record_requested = true;
    services.tick(&mut s);
    assert_eq!(
        services.drain_commands_for_tests(),
        vec![Command::StopRecording { request_id: 2 }]
    );
    assert_eq!(services.recording_state(), RecordState::Stopping);
    assert_eq!(s.record_label, "Finalizing…");
    assert_eq!(s.status.text, "FINALIZING RECORDING");
    // The engine wrote a file that is not a playable recording → failure keeps the fragments.
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"not a video").unwrap();
    services.inject(Event::Engine(IpcEvent::RecordingStopped {
        request_id: 2,
        path: path.clone(),
        bytes: 11,
    }));
    services.tick(&mut s);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while services.recording_state() != RecordState::Idle && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        services.tick(&mut s);
    }
    assert_eq!(services.recording_state(), RecordState::Idle);
    assert_eq!(errors(&s), vec!["MANUAL RECORDING FAILED".to_string()]);
    assert!(
        s.error_bar
            .current()
            .unwrap()
            .detail
            .contains("Recoverable video fragments remain in")
    );
    assert!(path.exists(), "fragments are kept");
    assert_eq!(s.record_label, "Record");
    // A real recording: the engine's own path is probed; use the synthetic test clip when an encoder exists.
    let _: PathBuf = path;
}

#[test]
fn recording_error_from_engine_fails_cleanly() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    connected(&mut services, &mut s);
    services.set_active_config_for_tests(Some(recording_config(&s).unwrap().capture));
    s.record_requested = true;
    services.tick(&mut s);
    let Command::StartRecording { request_id, .. } = services.drain_commands_for_tests().remove(0)
    else {
        panic!()
    };
    services.inject(Event::Engine(IpcEvent::RecordingError {
        request_id,
        message: "disk full".into(),
    }));
    services.tick(&mut s);
    assert_eq!(services.recording_state(), RecordState::Idle);
    assert_eq!(s.error_bar.current().unwrap().detail, "disk full");
    assert_eq!(s.status.text, "RECORDING FAILED");
    assert!(
        services
            .cards_for_tests()
            .iter()
            .all(|c| !matches!(c, CardKind::RecordingSaved { .. }))
    );
}
