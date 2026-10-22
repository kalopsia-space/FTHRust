//! Tests for the settings store.

use fthrust_core::settings::{
    SettingsStore, clips_subdirectories, defaults, merge_one_level, prepare_clips_storage,
};
use fthrust_core::{CoreError, FthrPaths};
use serde_json::{Map, Value, json};
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup() -> (tempfile::TempDir, FthrPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());
    std::fs::create_dir_all(p.root()).unwrap();
    (tmp, p)
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[test]
fn fresh_store_equals_fixture_defaults() {
    let (tmp, p) = setup();
    let s = SettingsStore::load(p).unwrap();
    let raw = include_str!("fixtures/settings-defaults.json")
        .replace("$HOME", tmp.path().to_str().unwrap());
    let expected: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(expected.as_object().unwrap().len(), 84);
    assert_eq!(Value::Object(s.data().clone()), expected);
}

#[test]
fn save_writes_indent2_and_skips_underscore_keys() {
    let (_t, p) = setup();
    let mut s = SettingsStore::load(p.clone()).unwrap();
    s.set("_runtime", json!(1));
    s.save().unwrap();
    let raw = std::fs::read_to_string(p.settings_file()).unwrap();
    assert!(!raw.contains("_runtime"));
    assert!(raw.ends_with("}\n"));
    assert!(raw.contains("\n  \"clip_length\": 30"));
}

#[test]
fn garbage_file_is_quarantined() {
    let (_t, p) = setup();
    std::fs::write(p.settings_file(), "{not json").unwrap();
    let s = SettingsStore::load(p.clone()).unwrap();
    let q = p.root().join("settings.json.corrupt");
    assert_eq!(std::fs::read_to_string(q).unwrap(), "{not json");
    assert_eq!(s.data(), &defaults(&p));
}

#[test]
fn array_file_is_quarantined() {
    let (_t, p) = setup();
    std::fs::write(p.settings_file(), "[1,2]").unwrap();
    let s = SettingsStore::load(p.clone()).unwrap();
    let q = p.root().join("settings.json.corrupt");
    assert_eq!(std::fs::read_to_string(q).unwrap(), "[1,2]");
    assert!(!p.settings_file().exists());
    assert_eq!(s.data(), &defaults(&p));
}

#[test]
fn one_level_merge_keeps_sibling_children() {
    let (_t, p) = setup();
    std::fs::write(p.settings_file(), r#"{"source_volumes": {"game": 50}}"#).unwrap();
    let s = SettingsStore::load(p).unwrap();
    let sv = s.get("source_volumes").unwrap();
    assert_eq!(sv["game"], json!(50));
    assert_eq!(sv["browser"], json!(100));
}

#[test]
fn one_level_merge_does_not_recurse_two_levels() {
    let base = obj(json!({"nb": {"x": {"a": 1, "b": 2}, "y": 3}}));
    let over = obj(json!({"nb": {"x": {"deep": 1}}}));
    let m = merge_one_level(base, over);
    assert_eq!(m["nb"], json!({"x": {"deep": 1}, "y": 3}));
}

#[test]
fn unknown_keys_survive_round_trip() {
    let (_t, p) = setup();
    std::fs::write(p.settings_file(), r#"{"future_key": [1, 2]}"#).unwrap();
    let mut s = SettingsStore::load(p.clone()).unwrap();
    s.save().unwrap();
    let s2 = SettingsStore::load(p).unwrap();
    assert_eq!(s2.get("future_key"), Some(&json!([1, 2])));
}

#[test]
fn getters_and_get_or_default() {
    let (_t, p) = setup();
    let mut s = SettingsStore::load(p).unwrap();
    assert_eq!(s.get_i64("clip_length"), Some(30));
    assert_eq!(s.get_bool("close_to_tray"), Some(true));
    assert_eq!(s.get_str("camera_size"), Some("medium"));
    assert_eq!(s.get_f64("camera_overlay_rect"), None);
    assert_eq!(s.remove("clip_length"), Some(json!(30)));
    assert_eq!(s.get("clip_length"), None);
    assert_eq!(s.get_or_default("clip_length"), &json!(30));
}

#[test]
fn clips_directory_expands_env_and_tilde() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: all env-mutating tests hold ENV_LOCK, so no concurrent env access.
    unsafe { std::env::set_var("FTHR_T", "sub") };
    let (tmp, p) = setup();
    let mut s = SettingsStore::load(p).unwrap();
    s.set("clips_directory", json!("~/vids/$FTHR_T/../x/${FTHR_T}"));
    assert_eq!(s.clips_directory().unwrap(), tmp.path().join("vids/x/sub"));
}

#[test]
fn unknown_env_var_is_left_untouched() {
    let (_t, p) = setup();
    let mut s = SettingsStore::load(p).unwrap();
    s.set("clips_directory", json!("/a/$FTHR_SURELY_UNSET_9/b"));
    assert_eq!(
        s.clips_directory().unwrap(),
        std::path::PathBuf::from("/a/$FTHR_SURELY_UNSET_9/b")
    );
}

#[test]
fn relative_clips_directory_is_invalid() {
    let (_t, p) = setup();
    let mut s = SettingsStore::load(p).unwrap();
    s.set("clips_directory", json!("relative/dir"));
    assert!(matches!(s.clips_directory(), Err(CoreError::Invalid(_))));
    s.set("clips_directory", json!(""));
    assert!(matches!(s.clips_directory(), Err(CoreError::Invalid(_))));
}

#[test]
fn recording_directory_derives_from_clips_when_missing() {
    let (_t, p) = setup();
    let mut s = SettingsStore::load(p).unwrap();
    s.set("clips_directory", json!("/data/clips"));
    s.remove("recording_directory");
    assert_eq!(
        s.recording_directory().unwrap(),
        std::path::PathBuf::from("/data/clips/Recordings")
    );
    s.set("recording_directory", json!(""));
    assert_eq!(
        s.recording_directory().unwrap(),
        std::path::PathBuf::from("/data/clips/Recordings")
    );
}

#[test]
fn prepare_clips_storage_creates_five_subdirs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("clips");
    prepare_clips_storage(&root).unwrap();
    for d in clips_subdirectories() {
        assert!(root.join(d).is_dir(), "{d}");
    }
    assert_eq!(clips_subdirectories().len(), 5);
}
