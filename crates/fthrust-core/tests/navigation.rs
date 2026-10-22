//! Tests for in-app navigation bindings.

use fthrust_core::FthrPaths;
use fthrust_core::navigation::{
    BINDING_DEFINITIONS, BindingError, NavigationBindings, Scope, normalize_binding,
};
use fthrust_core::settings::SettingsStore;
use serde_json::Value;

fn fixture() -> Value {
    let raw = include_str!("fixtures/navigation-bindings.json");
    serde_json::from_str(raw).unwrap()
}

fn store() -> (tempfile::TempDir, SettingsStore) {
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());
    std::fs::create_dir_all(p.root()).unwrap();
    (tmp, SettingsStore::load(p).unwrap())
}

fn scope_str(s: Scope) -> &'static str {
    match s {
        Scope::Application => "application",
        Scope::Main => "main",
        Scope::Library => "library",
        Scope::Settings => "settings",
        Scope::Editor => "editor",
    }
}

#[test]
fn definitions_match_fixture() {
    let fx = fixture();
    let rows = fx["definitions"].as_array().unwrap();
    assert_eq!(rows.len(), BINDING_DEFINITIONS.len());
    for (def, row) in BINDING_DEFINITIONS.iter().zip(rows) {
        let row: Vec<&str> = row
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            [
                def.action,
                def.label,
                def.default,
                scope_str(def.scope),
                def.category
            ],
            [row[0], row[1], row[2], row[3], row[4]]
        );
    }
}

#[test]
fn normalize_cases_match_fixture() {
    let fx = fixture();
    for case in fx["normalize_cases"].as_array().unwrap() {
        let input = case[0].as_str().unwrap();
        let want = case[1].as_str().map(str::to_string);
        assert_eq!(normalize_binding(input), want, "input {input:?}");
    }
}

#[test]
fn conflict_same_scope_rejected() {
    let (_t, s) = store();
    let mut nb = NavigationBindings::from_settings(&s);
    match nb.set("editor_split", "Ctrl+Z") {
        Err(BindingError::Conflict(c)) => {
            assert_eq!(c.action, "editor_split");
            assert_eq!(c.other_action, "editor_undo");
        }
        other => panic!("expected conflict, got {other:?}"),
    }
}

#[test]
fn conflict_application_scope_rejected() {
    let (_t, s) = store();
    let mut nb = NavigationBindings::from_settings(&s);
    assert!(matches!(
        nb.set("library_scroll_up", "Tab+S"),
        Err(BindingError::Conflict(_))
    ));
}

#[test]
fn main_vs_editor_allowed() {
    let (_t, s) = store();
    let mut nb = NavigationBindings::from_settings(&s);
    nb.set("toggle_detection", "Ctrl+Z").unwrap();
    assert_eq!(nb.get("toggle_detection"), Some("Ctrl+Z"));
}

#[test]
fn invalid_combo_and_unknown_action() {
    let (_t, s) = store();
    let mut nb = NavigationBindings::from_settings(&s);
    assert!(matches!(
        nb.set("editor_split", "ctrl+"),
        Err(BindingError::Invalid(_))
    ));
    assert!(matches!(nb.set("nope", "X"), Err(BindingError::Invalid(_))));
}

#[test]
fn missing_category_is_enabled() {
    let (_t, s) = store();
    let nb = NavigationBindings::from_settings(&s);
    assert!(nb.is_category_enabled("Clip editor"));
    assert!(nb.is_category_enabled("No such category"));
}

#[test]
fn round_trip_through_settings() {
    let (_t, mut s) = store();
    let mut nb = NavigationBindings::from_settings(&s);
    nb.set("editor_split", "ctrl+shift+k").unwrap();
    nb.write_to(&mut s);
    let again = NavigationBindings::from_settings(&s);
    assert_eq!(again.get("editor_split"), Some("Ctrl+Shift+K"));
    assert_eq!(again.get("editor_undo"), Some("Ctrl+Z"));
    assert!(again.is_enabled());
}

#[test]
fn qt_portable_key_names_are_canonical() {
    assert_eq!(normalize_binding("pgup").as_deref(), Some("PgUp"));
    assert_eq!(normalize_binding("Delete").as_deref(), Some("Del"));
    assert_eq!(normalize_binding("BACKSPACE").as_deref(), Some("Backspace"));
    assert_eq!(
        normalize_binding("Ctrl+PgDown").as_deref(),
        Some("Ctrl+PgDown")
    );
}

#[test]
fn unknown_key_is_rejected() {
    assert_eq!(normalize_binding("foo"), None);
    assert_eq!(normalize_binding("Ctrl+Foo"), None);
}

#[test]
fn set_empty_combo_clears_without_conflict() {
    let (_t, s) = store();
    let mut nb = NavigationBindings::from_settings(&s);
    nb.set("editor_split", "").unwrap();
    nb.set("editor_delete", "").unwrap();
    assert_eq!(nb.get("editor_split"), Some(""));
}

#[test]
fn enabled_flags_round_trip_through_write_to() {
    let (_t, mut s) = store();
    let mut nb = NavigationBindings::from_settings(&s);
    nb.set_enabled(false);
    nb.set_category_enabled("Clip editor", false);
    nb.write_to(&mut s);
    let again = NavigationBindings::from_settings(&s);
    assert!(!again.is_enabled());
    assert!(!again.is_category_enabled("Clip editor"));
    assert!(again.is_category_enabled("Clips"));
}
