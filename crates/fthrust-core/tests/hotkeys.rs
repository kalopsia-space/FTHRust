//! Tests for the global hotkey bindings file.

use fthrust_core::FthrPaths;
use fthrust_core::hotkeys::{
    HotkeyAction, HotkeyBindings, load_portal_cleared, normalize_keyboard_combo,
    save_portal_cleared, to_portal_trigger,
};
use serde_json::Value;
use std::collections::BTreeSet;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/hotkeys.json")).unwrap()
}

fn setup() -> (tempfile::TempDir, FthrPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());
    std::fs::create_dir_all(p.root()).unwrap();
    (tmp, p)
}

fn assert_map(b: &HotkeyBindings, expected: &Value, controller: bool) {
    for (id, want) in expected.as_object().unwrap() {
        let got = if controller {
            b.controller(id)
        } else {
            b.get(id)
        };
        assert_eq!(got, want.as_str(), "{id} controller={controller}");
    }
}

#[test]
fn defaults_match_fixture() {
    let f = fixture();
    let b = HotkeyBindings::defaults();
    assert_map(&b, &f["defaults"], false);
    assert_map(&b, &f["controller_defaults"], true);
    for a in f["actions"].as_array().unwrap() {
        let id = a.as_str().unwrap();
        let action = HotkeyAction::parse(id).unwrap();
        assert_eq!(action.id(), id);
        assert_eq!(
            action.description(),
            f["descriptions"][id].as_str().unwrap()
        );
    }
}

#[test]
fn normalize_cases_match_fixture() {
    for c in fixture()["normalize_cases"].as_array().unwrap() {
        let input = c[0].as_str().unwrap();
        assert_eq!(
            normalize_keyboard_combo(input).as_deref(),
            c[1].as_str(),
            "{input}"
        );
    }
}

#[test]
fn portal_trigger_cases_match_fixture() {
    for c in fixture()["portal_trigger_cases"].as_array().unwrap() {
        let input = c[0].as_str().unwrap();
        assert_eq!(to_portal_trigger(input), c[1].as_str().unwrap(), "{input}");
    }
}

fn check_legacy(name: &str, resave: bool) {
    let f = fixture();
    let entry = &f["legacy_files"][name];
    let (_t, p) = setup();
    std::fs::write(p.hotkeys_file(), entry["file"].to_string()).unwrap();
    let (b, needs) = HotkeyBindings::load(&p).unwrap();
    assert_map(&b, &entry["hotkeys"], false);
    assert_map(&b, &entry["controller_hotkeys"], true);
    assert_eq!(needs, resave, "{name}");
}

#[test]
fn legacy_flat() {
    check_legacy("flat", false);
}

#[test]
fn legacy_nested_keyboard_controller() {
    check_legacy("nested-keyboard-controller", true);
}

#[test]
fn legacy_top_level_keyboard_map() {
    check_legacy("legacy-top-level-keyboard-map", true);
}

#[test]
fn legacy_action_ids_parse() {
    assert_eq!(
        HotkeyAction::parse("game_capture_accept"),
        Some(HotkeyAction::ConfirmGameDetection)
    );
    assert_eq!(
        HotkeyAction::parse("game_capture_dismiss"),
        Some(HotkeyAction::DismissGameDetection)
    );
    assert_eq!(
        HotkeyAction::parse("save_custom_clip_30"),
        Some(HotkeyAction::SaveCustomClip(30))
    );
    assert_eq!(HotkeyAction::parse("save_custom_clip_4"), None);
    assert_eq!(HotkeyAction::parse("save_custom_clip_901"), None);
    assert_eq!(
        HotkeyAction::SaveCustomClip(30).description(),
        "Save 30-second clip"
    );
}

#[test]
fn missing_file_gives_defaults_without_resave() {
    let (_t, p) = setup();
    let (b, needs) = HotkeyBindings::load(&p).unwrap();
    assert_eq!(b, HotkeyBindings::defaults());
    assert!(!needs);
}

#[test]
fn non_string_value_is_dropped() {
    let (_t, p) = setup();
    std::fs::write(
        p.hotkeys_file(),
        r#"{"save_clip": 9, "save_screenshot": "F12"}"#,
    )
    .unwrap();
    let (b, needs) = HotkeyBindings::load(&p).unwrap();
    assert_eq!(b.get("save_clip"), Some("F9"));
    assert_eq!(b.get("save_screenshot"), Some("F12"));
    assert!(needs);
}

#[test]
fn extended_clip_is_dropped_and_flagged() {
    let (_t, p) = setup();
    std::fs::write(p.hotkeys_file(), r#"{"save_extended_clip": "F1"}"#).unwrap();
    let (b, needs) = HotkeyBindings::load(&p).unwrap();
    assert_eq!(b.get("save_extended_clip"), None);
    assert!(needs);
}

#[test]
fn custom_clip_out_of_range_is_dropped() {
    let (_t, p) = setup();
    std::fs::write(
        p.hotkeys_file(),
        r#"{"save_custom_clip_4": "F1", "save_custom_clip_901": "F2", "save_custom_clip_60": "f3"}"#,
    )
    .unwrap();
    let (b, _) = HotkeyBindings::load(&p).unwrap();
    assert_eq!(b.get("save_custom_clip_4"), None);
    assert_eq!(b.get("save_custom_clip_901"), None);
    assert_eq!(b.get("save_custom_clip_60"), Some("F3"));
    assert_eq!(b.custom_clip_lengths(), vec![60]);
}

#[test]
fn save_flat_and_nested_round_trip() {
    let (_t, p) = setup();
    let b = HotkeyBindings::defaults();
    b.save(&p).unwrap();
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(p.hotkeys_file()).unwrap()).unwrap();
    assert_eq!(raw["save_clip"], "F9");

    std::fs::write(
        p.hotkeys_file(),
        r#"{"save_clip": {"keyboard": "F10", "controller": "a+lb"}}"#,
    )
    .unwrap();
    let (loaded, _) = HotkeyBindings::load(&p).unwrap();
    assert_eq!(loaded.controller("save_clip"), Some("LB+A"));
    loaded.save(&p).unwrap();
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(p.hotkeys_file()).unwrap()).unwrap();
    assert_eq!(raw["save_clip"]["keyboard"], "F10");
    assert_eq!(raw["save_clip"]["controller"], "LB+A");
    assert_eq!(raw["save_screenshot"]["controller"], "");
    let (again, needs) = HotkeyBindings::load(&p).unwrap();
    assert_eq!(again, loaded);
    assert!(needs);
}

#[test]
fn sync_custom_clips_adds_and_removes() {
    let mut b = HotkeyBindings::defaults();
    b.sync_custom_clips(&[30, 60]);
    assert_eq!(b.custom_clip_lengths(), vec![30, 60]);
    assert_eq!(b.get("save_custom_clip_30"), Some(""));
    b.sync_custom_clips(&[60]);
    assert_eq!(b.custom_clip_lengths(), vec![60]);
    assert_eq!(b.get("save_clip"), Some("F9"));
}

#[test]
fn portal_cleared_round_trip_sorted() {
    let (_t, p) = setup();
    assert!(load_portal_cleared(&p).unwrap().is_empty());
    let set: BTreeSet<String> = ["stop_recording", "save_clip"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    save_portal_cleared(&p, &set).unwrap();
    let raw = std::fs::read_to_string(p.hotkeys_portal_cleared_file()).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&raw).unwrap(),
        serde_json::json!(["save_clip", "stop_recording"])
    );
    assert_eq!(load_portal_cleared(&p).unwrap(), set);
}
