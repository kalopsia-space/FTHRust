//! Parity tests for settings migrations against Python-produced fixtures.

use fthrust_core::FthrPaths;
use fthrust_core::settings::SettingsStore;
use serde_json::Value;
use std::path::PathBuf;

fn case_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/settings-migrations")
        .join(name)
}

/// Loads `input.json` as `settings.json` and compares with `expected.json`.
fn run_case(name: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(tmp.path());
    std::fs::create_dir_all(paths.root()).unwrap();
    std::fs::copy(case_dir(name).join("input.json"), paths.settings_file()).unwrap();
    let store = SettingsStore::load(paths).unwrap();
    let raw = std::fs::read_to_string(case_dir(name).join("expected.json"))
        .unwrap()
        .replace("$HOME", tmp.path().to_str().unwrap());
    let expected: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(Value::Object(store.data().clone()), expected, "case {name}");
}

#[test]
fn camera_legacy() {
    run_case("camera-legacy");
}

#[test]
fn image_overlay_singular() {
    run_case("image-overlay-singular");
}

#[test]
fn legacy_no_clips_dir() {
    run_case("legacy-no-clips-dir");
}

#[test]
fn nav_v1() {
    run_case("nav-v1");
}

#[test]
fn one_level_merge() {
    run_case("one-level-merge");
}

#[test]
fn recording_seed() {
    run_case("recording-seed");
}

#[test]
fn retired_keys() {
    run_case("retired-keys");
}

#[test]
fn unknown_keys_kept() {
    run_case("unknown-keys-kept");
}

#[test]
fn migration_rewrites_file_once() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(tmp.path());
    std::fs::create_dir_all(paths.root()).unwrap();
    let file = paths.settings_file();
    std::fs::copy(case_dir("retired-keys").join("input.json"), &file).unwrap();
    let first = SettingsStore::load(paths.clone()).unwrap();
    let bytes1 = std::fs::read(&file).unwrap();
    let on_disk: Value = serde_json::from_slice(&bytes1).unwrap();
    assert!(on_disk.get("auto_crop_enabled").is_none());
    assert!(on_disk["hotkeys"].get("save_extended_clip").is_none());
    assert!(on_disk.get("keyboard_overlay_enabled").is_none());
    assert_eq!(on_disk["navigation_bindings_version"], 5);
    let second = SettingsStore::load(paths).unwrap();
    assert_eq!(first.data(), second.data());
    assert_eq!(std::fs::read(&file).unwrap(), bytes1);
}
