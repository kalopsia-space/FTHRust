//! Credentials stay private: JSON files are written 0600 (a stricter mode is
//! kept) and `~/.fthr/uploader` is 0700.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use fthrust_core::upload::history::{Entry, History};
use fthrust_core::upload::settings::UploaderSettings;
use fthrust_core::{FthrPaths, write_json_atomic};
use serde_json::json;

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// The umask most desktops run with; the tests must not depend on the
/// environment's.
fn usual_umask() {
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o022));
}

fn home() -> (tempfile::TempDir, FthrPaths) {
    usual_umask();
    let tmp = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(tmp.path());
    fs::create_dir_all(paths.root()).unwrap();
    (tmp, paths)
}

#[test]
fn new_json_files_are_owner_only() {
    usual_umask();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    write_json_atomic(&path, &json!({"upload_auth_header": "Bearer x"}), true).unwrap();
    assert_eq!(mode(&path), 0o600);
    let compact = dir.path().join("hotkeys-portal-cleared.json");
    write_json_atomic(&compact, &json!([]), false).unwrap();
    assert_eq!(mode(&compact), 0o600);
}

#[test]
fn every_save_resets_a_wide_mode() {
    usual_umask();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    write_json_atomic(&path, &json!({"a": 1}), true).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    write_json_atomic(&path, &json!({"a": 2}), true).unwrap();
    assert_eq!(mode(&path), 0o600, "a chmod 644 does not survive a save");
}

#[test]
fn a_stricter_existing_mode_is_kept() {
    usual_umask();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
    write_json_atomic(&path, &json!({"a": 3}), true).unwrap();
    assert_eq!(mode(&path), 0o400);
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\n  \"a\": 3\n}\n");
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, ["settings.json"], "no temporary file is left");
}

#[test]
fn uploader_settings_live_in_a_private_directory() {
    let (_tmp, paths) = home();
    let mut settings = UploaderSettings::default();
    settings.set("upload_auth_header", json!("Bearer secret"));
    settings.set("catbox_userhash", json!("0123456789"));
    settings.save(&paths).unwrap();
    assert_eq!(mode(&paths.uploader_dir()), 0o700);
    assert_eq!(mode(&paths.uploader_settings_file()), 0o600);
    // A save after somebody loosened it puts it back.
    fs::set_permissions(
        paths.uploader_settings_file(),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    settings.save(&paths).unwrap();
    assert_eq!(mode(&paths.uploader_settings_file()), 0o600);
}

#[test]
fn an_existing_world_readable_uploader_directory_is_tightened() {
    let (_tmp, paths) = home();
    fs::create_dir_all(paths.uploader_dir()).unwrap();
    fs::set_permissions(paths.uploader_dir(), fs::Permissions::from_mode(0o755)).unwrap();
    UploaderSettings::default().save(&paths).unwrap();
    assert_eq!(mode(&paths.uploader_dir()), 0o700);
}

#[test]
fn startup_tightens_an_old_profile_but_does_not_create_the_directory() {
    let (_tmp, paths) = home();
    paths.restrict_private_dirs();
    assert!(
        !paths.uploader_dir().exists(),
        "nothing is created just to restrict it"
    );
    fs::create_dir_all(paths.uploader_dir()).unwrap();
    fs::set_permissions(paths.uploader_dir(), fs::Permissions::from_mode(0o755)).unwrap();
    paths.restrict_private_dirs();
    assert_eq!(mode(&paths.uploader_dir()), 0o700);
}

#[test]
fn upload_history_is_private_too() {
    let (_tmp, paths) = home();
    History::update(&paths, |h| {
        h.record(Path::new("/clips/a.mp4"), Entry::default());
    })
    .unwrap();
    assert_eq!(mode(&paths.uploader_dir()), 0o700);
    assert_eq!(mode(&paths.upload_history_file()), 0o600);
}

#[test]
fn the_legacy_history_copy_is_private() {
    let (_tmp, paths) = home();
    fs::write(paths.legacy_upload_history_file(), "{}").unwrap();
    fs::set_permissions(
        paths.legacy_upload_history_file(),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    History::migrate_legacy(&paths).unwrap();
    assert_eq!(mode(&paths.upload_history_file()), 0o600);
    assert_eq!(mode(&paths.uploader_dir()), 0o700);
}
