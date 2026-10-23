//! UI drive bench Task 1: the `FTHRUST_UI_DUMP` state dump.

use std::time::Duration;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::fidelity::{Marker, TextRun};
use fthrust_ui::fonts::Weight;
use fthrust_ui::shell::error_bar::Level;
use fthrust_ui::state::{AppState, DialogKind, Page};
use fthrust_ui::ui_dump::{Writer, from_env, snapshot};

fn marker() -> Marker {
    Marker {
        name: "QPushButton[Record]".into(),
        rect: Rect::from_min_size(Pos2::new(12.0, 8.0), Vec2::new(96.0, 36.0)),
        samples: Vec::new(),
        text: Some(TextRun {
            content: "Record".into(),
            px: 14.0,
            weight: Weight::W600,
        }),
        icon: None,
    }
}

#[test]
fn snapshot_carries_page_status_markers_and_dialog() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = AppState::for_tests(tmp.path(), serde_json::json!({}));
    state.page = Page::Settings;
    state.status.text = "CAPTURING".into();
    state.error_bar.push(
        "CLIP WAS NOT SAVED",
        "disk full",
        Level::Error,
        vec![],
        None,
        true,
    );
    state.dialogs.push_modal(DialogKind::Message {
        title: "Hello".into(),
        message: "x".into(),
    });
    let v = snapshot(&state, &[marker()], 1.0, 7, 2.5);
    assert_eq!(v["frame"], 7);
    assert_eq!(v["time"], 2.5);
    assert_eq!(v["pixels_per_point"], 1.0);
    assert_eq!(v["page"], "settings");
    assert_eq!(v["status"], "CAPTURING");
    assert_eq!(v["markers"][0]["name"], "QPushButton[Record]");
    assert_eq!(v["markers"][0]["x"], 12.0);
    assert_eq!(v["markers"][0]["w"], 96.0);
    assert_eq!(v["markers"][0]["text"], "Record");
    assert_eq!(v["error_bar"]["title"], "CLIP WAS NOT SAVED");
    assert_eq!(v["error_bar"]["level"], "error");
    assert_eq!(v["dialog"], "Hello");
    let plain = AppState::for_tests(tmp.path(), serde_json::json!({}));
    let v = snapshot(&plain, &[], 1.0, 1, 0.0);
    assert_eq!(v["page"], "clips");
    assert!(v["error_bar"].is_null());
    assert!(v["dialog"].is_null());
    assert_eq!(v["markers"].as_array().unwrap().len(), 0);
}

#[test]
fn writer_is_atomic_and_skips_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("ui.json");
    let mut w = Writer::new(path.clone());
    let state = AppState::for_tests(tmp.path(), serde_json::json!({}));
    let v1 = snapshot(&state, &[marker()], 1.0, 1, 0.0);
    assert!(w.write(&v1).unwrap());
    let back: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(back["frame"], 1);
    assert!(
        !path.with_extension("json.tmp").exists(),
        "no temp file left behind"
    );
    let v1b = snapshot(&state, &[marker()], 1.0, 2, 1.0);
    assert!(
        !w.write(&v1b).unwrap(),
        "same content apart from frame/time is skipped"
    );
    std::thread::sleep(Duration::from_millis(60));
    let mut changed = state;
    changed.status.text = "SAVING".into();
    let v2 = snapshot(&changed, &[marker()], 1.0, 3, 1.1);
    assert!(w.write(&v2).unwrap());
    changed.status.text = "SAVE FAILED".into();
    let v3 = snapshot(&changed, &[marker()], 1.0, 4, 1.12);
    assert!(!w.write(&v3).unwrap(), "throttled within 50 ms");
    std::thread::sleep(Duration::from_millis(60));
    assert!(w.write(&v3).unwrap());
    let back: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(back["status"], "SAVE FAILED");
}

#[test]
fn from_env_is_none_without_variable() {
    // Safety: tests in this file run in one process; the variable is private to them.
    unsafe { std::env::remove_var("FTHRUST_UI_DUMP") };
    assert!(from_env().is_none());
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("FTHRUST_UI_DUMP", tmp.path().join("ui.json")) };
    assert!(from_env().is_some());
    unsafe { std::env::remove_var("FTHRUST_UI_DUMP") };
}
