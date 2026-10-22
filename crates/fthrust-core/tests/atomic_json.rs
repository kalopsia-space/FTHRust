//! Tests for atomic JSON read/write.

use fthrust_core::{CoreError, read_json, write_json_atomic};
use serde_json::Value;

#[test]
fn pretty_write_matches_python_indent2() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.json");
    let value: Value = serde_json::from_str(r#"{"b":1,"a":[1,2]}"#).unwrap();
    write_json_atomic(&path, &value, true).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\n  \"b\": 1,\n  \"a\": [\n    1,\n    2\n  ]\n}\n"
    );
}

#[test]
fn compact_write_has_no_whitespace() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.json");
    let value: Value = serde_json::from_str(r#"{"b": 1, "a": [1, 2]}"#).unwrap();
    write_json_atomic(&path, &value, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"b":1,"a":[1,2]}"#
    );
}

#[test]
fn write_leaves_no_tmp_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    write_json_atomic(&path, &serde_json::json!({"x": 1}), true).unwrap();
    assert!(!dir.path().join("settings.json.tmp").exists());
    assert!(path.exists());
}

#[test]
fn write_replaces_existing_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.json");
    write_json_atomic(&path, &serde_json::json!({"x": 1}), true).unwrap();
    write_json_atomic(&path, &serde_json::json!({"y": 2}), true).unwrap();
    assert_eq!(
        read_json(&path).unwrap().unwrap(),
        serde_json::json!({"y": 2})
    );
}

#[test]
fn read_missing_is_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_json(&dir.path().join("nope.json")).unwrap().is_none());
}

#[test]
fn read_garbage_is_json_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    std::fs::write(&path, "not json {").unwrap();
    assert!(matches!(read_json(&path), Err(CoreError::Json { .. })));
}
