//! Tests for the capture presets store.

use fthrust_core::FthrPaths;
use fthrust_core::presets::PresetsStore;
use serde_json::{Map, Value, json};

fn setup() -> (tempfile::TempDir, FthrPaths, PresetsStore) {
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());
    std::fs::create_dir_all(p.root()).unwrap();
    let s = PresetsStore::new(&p);
    (tmp, p, s)
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[test]
fn round_trip() {
    let (_t, _p, s) = setup();
    let v = map(json!({"clip_length": 30, "framerate": 60, "audio_capture_enabled": true}));
    s.save("a", &v).unwrap();
    assert_eq!(s.load("a").unwrap(), Some(v));
    assert_eq!(s.load("missing").unwrap(), None);
}

#[test]
fn list_names_sorted() {
    let (_t, _p, s) = setup();
    for n in ["zeta", "alpha", "mid"] {
        s.save(n, &map(json!({"clip_length": 1}))).unwrap();
    }
    assert_eq!(s.list_names().unwrap(), ["alpha", "mid", "zeta"]);
}

#[test]
fn delete_missing_false() {
    let (_t, _p, s) = setup();
    s.save("a", &map(json!({"clip_length": 1}))).unwrap();
    assert!(!s.delete("nope").unwrap());
    assert!(s.delete("a").unwrap());
    assert!(s.list_names().unwrap().is_empty());
}

#[test]
fn load_strips_retired_keys_and_rewrites_file() {
    let (_t, p, s) = setup();
    std::fs::write(
        p.presets_file(),
        r#"{"a": {"clip_length": 30, "extended_clip_length": 90, "multiband_audio_enabled": true,
        "audio_categories": ["x"], "auto_crop_enabled": true, "framerate": 60}}"#,
    )
    .unwrap();
    let got = s.load("a").unwrap().unwrap();
    assert_eq!(
        Value::Object(got),
        json!({"clip_length": 30, "framerate": 60})
    );
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(p.presets_file()).unwrap()).unwrap();
    assert_eq!(on_disk, json!({"a": {"clip_length": 30, "framerate": 60}}));
}

#[test]
fn save_keeps_only_preset_keys() {
    let (_t, p, s) = setup();
    s.save(
        "a",
        &map(json!({"clip_length": 5, "junk": 1, "auto_crop_enabled": true})),
    )
    .unwrap();
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(p.presets_file()).unwrap()).unwrap();
    assert_eq!(on_disk, json!({"a": {"clip_length": 5}}));
}

#[test]
fn unreadable_file_is_empty() {
    let (_t, p, s) = setup();
    std::fs::write(p.presets_file(), "{not json").unwrap();
    assert!(s.list_names().unwrap().is_empty());
    assert_eq!(s.load("a").unwrap(), None);
}
