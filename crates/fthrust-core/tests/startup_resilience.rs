//! A full or read-only disk must not stop the app from opening: loading the
//! stores carries on and reports what it could not write.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use fthrust_core::FthrPaths;
use fthrust_core::settings::SettingsStore;
use fthrust_core::theme::ThemeStore;

#[test]
fn a_failed_migration_save_does_not_stop_settings_loading() {
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());
    fs::create_dir_all(p.root()).unwrap();
    // A retired key makes the load rewrite the file.
    fs::write(
        p.settings_file(),
        r#"{"clip_length": 45, "auto_crop_enabled": true}"#,
    )
    .unwrap();
    fs::set_permissions(p.root(), fs::Permissions::from_mode(0o555)).unwrap();
    let still_writable = fs::write(p.root().join("probe"), "").is_ok();
    if still_writable {
        // Running as root: the mode does not bind us.
        fs::remove_file(p.root().join("probe")).unwrap();
        fs::set_permissions(p.root(), fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let loaded = SettingsStore::load(p.clone());
    fs::set_permissions(p.root(), fs::Permissions::from_mode(0o755)).unwrap();
    let store = loaded.expect("the app opens although the migration cannot be saved");
    assert_eq!(store.get_i64("clip_length"), Some(45));
    assert!(
        store.get("auto_crop_enabled").is_none(),
        "the migrated settings are used from memory"
    );
    assert!(
        store
            .startup_issues()
            .iter()
            .any(|m| m.contains("could not be saved")),
        "the failure is reported: {:?}",
        store.startup_issues()
    );
}

#[test]
fn a_theme_folder_that_cannot_be_created_does_not_stop_the_theme_loading() {
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());
    // ~/.fthr is a file, so no folder below it can be created (also as root).
    fs::write(p.root(), "not a directory").unwrap();
    let store = ThemeStore::load(p.clone()).expect("the app opens without a theme folder");
    assert!(!store.has_any_customization(), "the stock theme is used");
    assert!(
        store
            .startup_issues()
            .iter()
            .any(|m| m.contains("theme folder")),
        "the failure is reported: {:?}",
        store.startup_issues()
    );
}
