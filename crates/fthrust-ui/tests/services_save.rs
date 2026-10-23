//! Plan 5 Task 10: the save-clip flow.

use std::path::PathBuf;

use fthrust_core::capture_health::{HealthSnapshot, HealthState};
use fthrust_core::hotkeys::HotkeyAction;
use fthrust_ipc::{Command, Event as IpcEvent, SavedClip};
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

fn stopped() -> HealthSnapshot {
    HealthSnapshot {
        state: HealthState::Stopped,
        ..healthy()
    }
}

fn connected(services: &mut Services, s: &mut AppState, snapshot: HealthSnapshot) {
    services.inject(Event::EngineConnected(true));
    services.inject(Event::Status {
        text: "CAPTURING".into(),
        style: StatusStyle::Active,
    });
    services.inject(Event::Health(snapshot));
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
fn save_builds_reference_path_and_suffixes() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.set_wall_clock_for_tests(datetime!(2027-01-15 10:00:00 UTC));
    connected(&mut services, &mut s, healthy());
    let root = s.library.clips_root.clone();
    services.inject(Event::Hotkey(HotkeyAction::SaveClip));
    services.tick(&mut s);
    let cmds = services.drain_commands_for_tests();
    let expected = root.join("Desktop/desktop_clip_from_15Jan2027_10-00-00.mp4");
    assert_eq!(
        cmds,
        vec![Command::SaveClip {
            request_id: 1,
            path: expected.clone(),
            seconds: 30
        }]
    );
    assert_eq!(s.status.text, "SAVING");
    assert!(
        expected.parent().unwrap().is_dir(),
        "the Desktop folder is created"
    );
    // Complete it, then save again in the same second: `_2`.
    std::fs::write(&expected, b"video").unwrap();
    services.inject(Event::Engine(IpcEvent::SaveStarted { request_id: 1 }));
    services.inject(Event::Engine(IpcEvent::SaveResult {
        request_id: 1,
        result: Ok(SavedClip {
            path: expected.clone(),
            bytes_written: 5,
            audio_included: true,
            audio_error: None,
            audio_tracks: vec!["Default Mix".into()],
        }),
    }));
    services.tick(&mut s);
    services.advance_clock_for_tests(1.5);
    s.set_setting("capture_mode", serde_json::json!("window"));
    s.set_setting("target_window_name", serde_json::json!("Overwatch: 2"));
    services.inject(Event::Hotkey(HotkeyAction::SaveCustomClip(90)));
    services.tick(&mut s);
    let cmds = services.drain_commands_for_tests();
    assert_eq!(
        cmds,
        vec![Command::SaveClip {
            request_id: 2,
            path: root.join("Overwatch 2/Overwatch 2_clip_from_15Jan2027_10-00-00.mp4"),
            seconds: 90
        }]
    );
    services.inject(Event::Engine(IpcEvent::SaveResult {
        request_id: 2,
        result: Err("x".into()),
    }));
    services.tick(&mut s);
    services.advance_clock_for_tests(1.5);
    s.set_setting("capture_mode", serde_json::json!("desktop"));
    services.inject(Event::Hotkey(HotkeyAction::SaveClip));
    services.tick(&mut s);
    let cmds = services.drain_commands_for_tests();
    assert_eq!(
        cmds[0],
        Command::SaveClip {
            request_id: 3,
            path: root.join("Desktop/desktop_clip_from_15Jan2027_10-00-00_2.mp4"),
            seconds: 30
        }
    );
}

#[test]
fn save_refused_when_disconnected_pushes_error_002() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.inject(Event::Hotkey(HotkeyAction::SaveClip));
    services.tick(&mut s);
    assert!(services.drain_commands_for_tests().is_empty());
    assert_eq!(errors(&s), vec!["CAPTURE UNAVAILABLE".to_string()]);
    assert!(
        s.error_bar
            .current()
            .unwrap()
            .detail
            .contains("not connected")
    );
}

#[test]
fn save_rejected_by_admission_when_stopped() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    connected(&mut services, &mut s, stopped());
    services.inject(Event::Hotkey(HotkeyAction::SaveClip));
    services.tick(&mut s);
    assert!(services.drain_commands_for_tests().is_empty());
    assert_eq!(errors(&s), vec!["CLIP NOT SAVED".to_string()]);
    let e = s.error_bar.current().unwrap();
    assert_eq!(
        e.detail,
        "Capture is not receiving new frames. Restart capture before saving."
    );
    assert_eq!(e.actions[0].label, "RESTART CAPTURE");
    assert!(services.cards_for_tests().contains(&CardKind::Error {
        detail: String::new()
    }));
}

#[test]
fn spam_guard_within_1s_shows_saving_only() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    connected(&mut services, &mut s, healthy());
    services.inject(Event::Hotkey(HotkeyAction::SaveClip));
    services.tick(&mut s);
    services.advance_clock_for_tests(0.2);
    services.inject(Event::Hotkey(HotkeyAction::SaveClip));
    services.tick(&mut s);
    assert_eq!(
        services.drain_commands_for_tests().len(),
        1,
        "one command for two presses"
    );
    assert_eq!(s.status.text, "SAVING");
    assert!(errors(&s).is_empty());
    // After the busy notice the status returns to the engine's text.
    services.inject(Event::Engine(IpcEvent::SaveStarted { request_id: 1 }));
    services.inject(Event::Engine(IpcEvent::SaveResult {
        request_id: 1,
        result: Err("disk".into()),
    }));
    services.tick(&mut s);
    assert_eq!(s.status.text, "SAVE FAILED");
    services.advance_clock_for_tests(3.1);
    services.tick(&mut s);
    assert_eq!(s.status.text, "CAPTURING");
}

#[test]
fn save_result_error_pushes_clip_was_not_saved_and_status_recovers() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    connected(&mut services, &mut s, healthy());
    services.inject(Event::Hotkey(HotkeyAction::SaveClip));
    services.tick(&mut s);
    let cmd = services.drain_commands_for_tests().remove(0);
    let Command::SaveClip { path, .. } = cmd else {
        panic!("{cmd:?}")
    };
    services.inject(Event::Engine(IpcEvent::SaveStarted { request_id: 1 }));
    services.inject(Event::Engine(IpcEvent::SaveResult {
        request_id: 1,
        result: Err("No space left on device".into()),
    }));
    services.tick(&mut s);
    assert_eq!(errors(&s), vec!["CLIP WAS NOT SAVED".to_string()]);
    let e = s.error_bar.current().unwrap();
    assert_eq!(e.detail, "No space left on device");
    assert_eq!(e.actions[0].label, "OPEN FOLDER");
    assert_eq!(s.status.text, "SAVE FAILED");
    assert!(!path.exists());
    assert!(s.library.records.is_empty(), "no card for a missing file");
    assert!(
        services
            .cards_for_tests()
            .iter()
            .any(|c| matches!(c, CardKind::Error { .. }))
    );
    services.advance_clock_for_tests(3.1);
    services.tick(&mut s);
    assert_eq!(
        (s.status.text.as_str(), s.status.style),
        ("CAPTURING", StatusStyle::Active)
    );
    // A reported success without a file is also a failure.
    services.advance_clock_for_tests(1.5);
    services.inject(Event::Hotkey(HotkeyAction::SaveClip));
    services.tick(&mut s);
    services.drain_commands_for_tests();
    services.inject(Event::Engine(IpcEvent::SaveResult {
        request_id: 2,
        result: Ok(SavedClip {
            path: PathBuf::from("/nowhere/x.mp4"),
            bytes_written: 0,
            audio_included: false,
            audio_error: None,
            audio_tracks: vec![],
        }),
    }));
    services.tick(&mut s);
    assert!(s.error_bar.entries().iter().any(|e| {
        e.detail
            .contains("reported success but no clip file was written")
    }));
}

#[test]
fn completed_save_upserts_library_and_marks_ready() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    connected(&mut services, &mut s, healthy());
    services.inject(Event::Hotkey(HotkeyAction::SaveClip));
    services.tick(&mut s);
    let Command::SaveClip { path, .. } = services.drain_commands_for_tests().remove(0) else {
        panic!()
    };
    assert!(
        services
            .cards_for_tests()
            .iter()
            .any(|c| matches!(c, CardKind::Clip { seconds: 30, .. })),
        "immediate feedback card"
    );
    std::fs::write(&path, b"video").unwrap();
    services.inject(Event::Engine(IpcEvent::SaveStarted { request_id: 1 }));
    services.inject(Event::Engine(IpcEvent::SaveResult {
        request_id: 1,
        result: Ok(SavedClip {
            path: path.clone(),
            bytes_written: 5,
            audio_included: true,
            audio_error: None,
            audio_tracks: vec!["Default Mix".into()],
        }),
    }));
    services.tick(&mut s);
    assert_eq!(s.library.records.len(), 1);
    assert_eq!(s.library.records[0].path, path);
    assert!(s.readiness.can_access(&path));
    assert!(s.library_dirty);
    assert_eq!(
        s.status.text, "CAPTURING",
        "status restored after a completed save"
    );
    assert!(errors(&s).is_empty());
    // An audio error is a warning, not a failure.
    services.advance_clock_for_tests(1.5);
    services.inject(Event::Hotkey(HotkeyAction::SaveClip));
    services.tick(&mut s);
    let Command::SaveClip { path: p2, .. } = services.drain_commands_for_tests().remove(0) else {
        panic!()
    };
    std::fs::write(&p2, b"video").unwrap();
    services.inject(Event::Engine(IpcEvent::SaveResult {
        request_id: 2,
        result: Ok(SavedClip {
            path: p2.clone(),
            bytes_written: 5,
            audio_included: false,
            audio_error: Some("AAC encoder unavailable".into()),
            audio_tracks: vec![],
        }),
    }));
    services.tick(&mut s);
    assert_eq!(errors(&s), vec!["CLIP SAVED WITHOUT AUDIO".to_string()]);
    assert_eq!(s.library.records.len(), 2);
}

#[test]
fn slow_notice_at_60s_then_late_success_accepted() {
    // The state machine: no acknowledgement within 1 s → `SAVE SLOW…` (a
    // notice, not a verdict); no result within 60 s → failed; a late result
    // before that is accepted.
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    connected(&mut services, &mut s, healthy());
    services.inject(Event::Hotkey(HotkeyAction::SaveClip));
    services.tick(&mut s);
    let Command::SaveClip { path, .. } = services.drain_commands_for_tests().remove(0) else {
        panic!()
    };
    services.advance_clock_for_tests(1.2);
    services.tick(&mut s);
    assert_eq!(
        (s.status.text.as_str(), s.status.style),
        ("SAVE SLOW…", StatusStyle::Warning)
    );
    assert!(errors(&s).is_empty(), "the slow notice is not a failure");
    services.advance_clock_for_tests(10.0);
    std::fs::write(&path, b"video").unwrap();
    services.inject(Event::Engine(IpcEvent::SaveResult {
        request_id: 1,
        result: Ok(SavedClip {
            path: path.clone(),
            bytes_written: 5,
            audio_included: true,
            audio_error: None,
            audio_tracks: vec![],
        }),
    }));
    services.tick(&mut s);
    assert_eq!(s.library.records.len(), 1, "late success is accepted");
    assert_eq!(s.status.text, "CAPTURING");
    // Nothing at all within 60 s is a failure.
    services.advance_clock_for_tests(1.5);
    services.inject(Event::Hotkey(HotkeyAction::SaveClip));
    services.tick(&mut s);
    services.drain_commands_for_tests();
    services.advance_clock_for_tests(61.0);
    // One tick reports the (long overdue) slow notice, the next gives up.
    services.tick(&mut s);
    services.tick(&mut s);
    assert_eq!(errors(&s), vec!["CLIP WAS NOT SAVED".to_string()]);
    assert!(
        s.error_bar
            .current()
            .unwrap()
            .detail
            .contains("never reported")
    );
}

#[test]
fn engine_loss_fails_the_in_flight_save() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.tokens.reduce_motion = true;
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.inject(Event::EngineConnected(true));
    services.inject(Event::Health(healthy()));
    services.tick(&mut s);
    services.save_clip(&mut s, 30);
    services.inject(Event::Engine(IpcEvent::SaveStarted { request_id: 1 }));
    services.tick(&mut s);
    assert!(services.save_busy(), "a save is in flight");
    services.inject(Event::EngineConnected(false));
    services.tick(&mut s);
    assert!(
        !services.save_busy(),
        "the save is over the moment the engine is gone"
    );
    let e = s.error_bar.current().unwrap();
    assert_eq!(e.title, "CLIP WAS NOT SAVED");
    assert!(e.detail.contains("engine stopped"), "{}", e.detail);
}
