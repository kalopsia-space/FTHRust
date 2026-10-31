//! A store that cannot be read is never replaced by what the app made up in
//! its place, and a damaged one is copied aside before anything overwrites it.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use fthrust_core::export_presets::{ExportPreset, ExportPresetStore};
use fthrust_core::presets::PresetsStore;
use fthrust_core::settings::SettingsStore;
use fthrust_core::theme::ThemeStore;
use fthrust_core::upload::history::{Entry, History};
use fthrust_core::upload::settings::UploaderSettings;
use fthrust_core::{CoreError, FthrPaths, write_json_atomic};
use serde_json::{Map, Value, json};

fn home() -> (tempfile::TempDir, FthrPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(tmp.path());
    fs::create_dir_all(paths.root()).unwrap();
    (tmp, paths)
}

/// Takes all access away from `path`. Returns false (and puts the mode back)
/// where the mode does not bind us, as when the tests run as root.
fn make_unreadable(path: &Path) -> bool {
    fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(path).is_ok() {
        make_readable(path);
        return false;
    }
    true
}

fn make_readable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn is_protected<T: std::fmt::Debug>(r: Result<T, CoreError>) -> bool {
    matches!(r, Err(CoreError::Protected { .. }))
}

const SETTINGS: &str = r#"{"clip_length": 77, "sftp_note": "keep me"}"#;

#[test]
fn unreadable_settings_are_not_overwritten_by_defaults() {
    let (_t, p) = home();
    fs::write(p.settings_file(), SETTINGS).unwrap();
    if !make_unreadable(&p.settings_file()) {
        return;
    }
    let mut s = SettingsStore::load(p.clone()).unwrap();
    assert_eq!(
        s.get_i64("clip_length"),
        Some(30),
        "defaults for the session"
    );
    assert!(
        s.startup_issues()
            .iter()
            .any(|m| m.contains("settings.json")),
        "the problem is reported: {:?}",
        s.startup_issues()
    );
    // The app saves on every exit; that must not replace the real file.
    s.set("clip_length", json!(45));
    assert!(is_protected(s.save()));
    make_readable(&p.settings_file());
    assert_eq!(fs::read_to_string(p.settings_file()).unwrap(), SETTINGS);
    let again = SettingsStore::load(p).unwrap();
    assert_eq!(again.get_i64("clip_length"), Some(77));
}

#[test]
fn damaged_settings_are_kept_as_a_copy_and_reported() {
    let (_t, p) = home();
    fs::write(p.settings_file(), "{\"clip_length\": 77,").unwrap();
    let mut s = SettingsStore::load(p.clone()).unwrap();
    assert_eq!(
        fs::read_to_string(p.root().join("settings.json.corrupt")).unwrap(),
        "{\"clip_length\": 77,"
    );
    assert!(
        s.startup_issues().iter().any(|m| m.contains("damaged")),
        "{:?}",
        s.startup_issues()
    );
    s.save().unwrap();
    assert!(p.settings_file().exists());
}

#[test]
fn a_settings_file_that_cannot_be_set_aside_is_left_alone() {
    let (_t, p) = home();
    fs::write(p.settings_file(), "{not json").unwrap();
    // A directory in the way makes the rename fail on every platform.
    let mut name = p.settings_file().into_os_string();
    name.push(".corrupt");
    fs::create_dir_all(Path::new(&name).join("occupied")).unwrap();
    let mut s = SettingsStore::load(p.clone()).unwrap();
    assert!(is_protected(s.save()));
    assert_eq!(fs::read_to_string(p.settings_file()).unwrap(), "{not json");
}

#[test]
fn unreadable_uploader_settings_keep_their_credentials() {
    let (_t, p) = home();
    fs::create_dir_all(p.uploader_dir()).unwrap();
    let original = r#"{"upload_auth_header": "Bearer keepme", "upload_provider": "custom"}"#;
    fs::write(p.uploader_settings_file(), original).unwrap();
    if !make_unreadable(&p.uploader_settings_file()) {
        return;
    }
    let mut s = UploaderSettings::load(&p);
    s.set("upload_enabled", json!(true));
    assert!(is_protected(s.save(&p)));
    make_readable(&p.uploader_settings_file());
    assert_eq!(
        fs::read_to_string(p.uploader_settings_file()).unwrap(),
        original
    );
}

#[test]
fn damaged_uploader_settings_are_copied_before_they_are_replaced() {
    let (_t, p) = home();
    fs::create_dir_all(p.uploader_dir()).unwrap();
    fs::write(
        p.uploader_settings_file(),
        "{\"upload_auth_header\": \"Bear",
    )
    .unwrap();
    let s = UploaderSettings::load(&p);
    s.save(&p).unwrap();
    let mut name = p.uploader_settings_file().into_os_string();
    name.push(".corrupt");
    assert_eq!(
        fs::read_to_string(name).unwrap(),
        "{\"upload_auth_header\": \"Bear"
    );
}

#[test]
fn unreadable_upload_history_is_not_replaced_by_one_entry() {
    let (_t, p) = home();
    fs::create_dir_all(p.uploader_dir()).unwrap();
    let original = r#"{"/clips/old.mp4": {"status": "ok", "url": "https://x/y"}}"#;
    fs::write(p.upload_history_file(), original).unwrap();
    if !make_unreadable(&p.upload_history_file()) {
        return;
    }
    let r = History::update(&p, |h| {
        h.record(Path::new("/clips/new.mp4"), Entry::default())
    });
    assert!(is_protected(r));
    make_readable(&p.upload_history_file());
    assert_eq!(
        fs::read_to_string(p.upload_history_file()).unwrap(),
        original
    );
}

#[test]
fn damaged_upload_history_is_copied_before_the_update() {
    let (_t, p) = home();
    fs::create_dir_all(p.uploader_dir()).unwrap();
    fs::write(p.upload_history_file(), "{\"/clips/old.mp4\": {").unwrap();
    History::update(&p, |h| {
        h.record(Path::new("/clips/new.mp4"), Entry::default())
    })
    .unwrap();
    let mut name = p.upload_history_file().into_os_string();
    name.push(".corrupt");
    assert_eq!(fs::read_to_string(name).unwrap(), "{\"/clips/old.mp4\": {");
}

fn preset_values() -> Map<String, Value> {
    json!({"clip_length": 15}).as_object().unwrap().clone()
}

#[test]
fn unreadable_presets_are_not_replaced_by_the_new_one() {
    let (_t, p) = home();
    let original = r#"{"fast": {"clip_length": 10}, "slow": {"clip_length": 60}}"#;
    fs::write(p.presets_file(), original).unwrap();
    if !make_unreadable(&p.presets_file()) {
        return;
    }
    let store = PresetsStore::new(&p);
    assert!(store.list_names().unwrap().is_empty());
    assert!(is_protected(store.save("mine", &preset_values())));
    assert!(is_protected(store.delete("fast")));
    make_readable(&p.presets_file());
    assert_eq!(fs::read_to_string(p.presets_file()).unwrap(), original);
}

fn export_preset(id: &str) -> ExportPreset {
    ExportPreset {
        preset_id: id.into(),
        name: format!("Name {id}"),
        target_size_mb: 25.0,
        ..ExportPreset::catbox_adhoc()
    }
}

#[test]
fn unreadable_export_presets_are_not_replaced_by_the_new_one() {
    let (_t, p) = home();
    let store = ExportPresetStore::new(&p);
    store.save(&export_preset("keeper")).unwrap();
    let original = fs::read_to_string(p.export_presets_file()).unwrap();
    if !make_unreadable(&p.export_presets_file()) {
        return;
    }
    assert!(is_protected(store.save(&export_preset("other"))));
    assert!(is_protected(store.delete("keeper")));
    make_readable(&p.export_presets_file());
    assert_eq!(
        fs::read_to_string(p.export_presets_file()).unwrap(),
        original
    );
}

#[test]
fn damaged_export_presets_are_copied_before_they_are_replaced() {
    let (_t, p) = home();
    fs::write(p.export_presets_file(), "[{\"preset_id\": \"mine\",").unwrap();
    let store = ExportPresetStore::new(&p);
    store.save(&export_preset("fresh")).unwrap();
    let mut name = p.export_presets_file().into_os_string();
    name.push(".corrupt");
    assert_eq!(
        fs::read_to_string(name).unwrap(),
        "[{\"preset_id\": \"mine\","
    );
    assert!(store.get("fresh").unwrap().is_some());
}

#[test]
fn unreadable_theme_is_not_replaced_by_the_default_one() {
    let (_t, p) = home();
    fs::create_dir_all(p.theme_dir()).unwrap();
    let original = r##"{"palette_version": 4, "colors": {"BG": "#123456"}}"##;
    fs::write(p.theme_file(), original).unwrap();
    if !make_unreadable(&p.theme_file()) {
        return;
    }
    let store = ThemeStore::load(p.clone()).unwrap();
    assert!(
        store
            .startup_issues()
            .iter()
            .any(|m| m.contains("theme.json")),
        "{:?}",
        store.startup_issues()
    );
    assert!(is_protected(store.save()));
    make_readable(&p.theme_file());
    assert_eq!(fs::read_to_string(p.theme_file()).unwrap(), original);
}

#[test]
fn damaged_theme_is_set_aside_once_like_settings() {
    let (_t, p) = home();
    fs::create_dir_all(p.theme_dir()).unwrap();
    fs::write(p.theme_file(), "{\"colors\": {").unwrap();
    let store = ThemeStore::load(p.clone()).unwrap();
    let mut name = p.theme_file().into_os_string();
    name.push(".corrupt");
    assert_eq!(fs::read_to_string(&name).unwrap(), "{\"colors\": {");
    assert!(!p.theme_file().exists(), "moved, not copied");
    assert!(
        store
            .startup_issues()
            .iter()
            .any(|m| m.contains("theme.json was damaged")),
        "{:?}",
        store.startup_issues()
    );
    // Saving writes a fresh file, and the next load has nothing to report.
    store.save().unwrap();
    let again = ThemeStore::load(p.clone()).unwrap();
    assert!(again.startup_issues().is_empty());
    assert_eq!(fs::read_to_string(&name).unwrap(), "{\"colors\": {");
}

#[test]
fn a_damaged_theme_that_cannot_be_set_aside_is_left_alone() {
    let (_t, p) = home();
    fs::create_dir_all(p.theme_dir()).unwrap();
    fs::write(p.theme_file(), "{\"colors\": {").unwrap();
    // A directory in the way makes the rename fail everywhere.
    let mut name = p.theme_file().into_os_string();
    name.push(".corrupt");
    fs::create_dir_all(Path::new(&name).join("occupied")).unwrap();
    let store = ThemeStore::load(p.clone()).unwrap();
    assert!(is_protected(store.save()));
    assert_eq!(
        fs::read_to_string(p.theme_file()).unwrap(),
        "{\"colors\": {"
    );
}

#[test]
fn the_generic_writer_refuses_an_unreadable_file_and_copies_a_damaged_one() {
    let (_t, p) = home();
    let path = p.root().join("x.json");
    fs::write(&path, "{\"a\": 1}").unwrap();
    if make_unreadable(&path) {
        assert!(is_protected(write_json_atomic(
            &path,
            &json!({"b": 2}),
            true
        )));
        make_readable(&path);
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"a\": 1}");
    }
    fs::write(&path, "{\"a\": ").unwrap();
    write_json_atomic(&path, &json!({"b": 2}), true).unwrap();
    assert_eq!(
        fs::read_to_string(p.root().join("x.json.corrupt")).unwrap(),
        "{\"a\": "
    );
    // Valid JSON and a missing file are replaced and created without fuss.
    write_json_atomic(&path, &json!({"c": 3}), true).unwrap();
    write_json_atomic(&p.root().join("new.json"), &json!({}), true).unwrap();
}
