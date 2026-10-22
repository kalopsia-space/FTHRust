//! Tests for the `~/.fthr` path map and the runtime directory.

use fthrust_core::{CoreError, FthrPaths, runtime_dir};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn root_is_dot_fthr_under_home() {
    let p = FthrPaths::new("/home/u");
    assert_eq!(p.root(), PathBuf::from("/home/u/.fthr"));
}

#[test]
fn every_accessor_is_under_root() {
    let p = FthrPaths::new("/home/u");
    let root = p.root();
    let all = [
        p.settings_file(),
        p.presets_file(),
        p.export_presets_file(),
        p.encoder_capabilities_file(),
        p.clip_metadata_db(),
        p.legacy_clip_metadata_json(),
        p.thumbnails_dir(),
        p.theme_dir(),
        p.theme_file(),
        p.theme_icons_dir(),
        p.theme_sounds_dir(),
        p.theme_fonts_dir(),
        p.logs_dir(),
        p.app_log(),
        p.crash_log(),
        p.engine_log(),
        p.diagnostics_dir(),
        p.uploader_dir(),
        p.uploader_settings_file(),
        p.upload_history_file(),
        p.legacy_upload_history_file(),
        p.playback_cache_dir(),
        p.hotkeys_file(),
        p.hotkeys_portal_cleared_file(),
    ];
    for path in all {
        assert!(path.starts_with(&root), "{path:?} not under {root:?}");
    }
    assert_eq!(p.app_log().file_name().unwrap(), "fthr.log");
    assert_eq!(p.crash_log().file_name().unwrap(), "fthr-crash.log");
    assert_eq!(p.engine_log().file_name().unwrap(), "engine.log");
    assert_eq!(
        p.legacy_clips_directory(),
        PathBuf::from("/home/u/FTHR_Clips")
    );
}

#[test]
fn default_clips_directory_matches_fixture() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/settings-defaults.json"
    ))
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let expected = v["clips_directory"]
        .as_str()
        .unwrap()
        .replace("$HOME", "/home/u");
    let p = FthrPaths::new("/home/u");
    assert_eq!(p.default_clips_directory(), PathBuf::from(expected));
}

#[test]
fn runtime_dir_refuses_group_writable() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("fthrust");
    std::fs::create_dir(&d).unwrap();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o770)).unwrap();
    // SAFETY: all env-mutating tests hold ENV_LOCK, so no concurrent env access.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
    let r = runtime_dir();
    assert!(matches!(r, Err(CoreError::RuntimeDir(_))), "{r:?}");
}

#[test]
fn runtime_dir_refuses_symlink() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&target, tmp.path().join("fthrust")).unwrap();
    // SAFETY: all env-mutating tests hold ENV_LOCK, so no concurrent env access.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
    assert!(matches!(runtime_dir(), Err(CoreError::RuntimeDir(_))));
}

#[test]
fn runtime_dir_creates_0700() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: all env-mutating tests hold ENV_LOCK, so no concurrent env access.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
    let d = runtime_dir().unwrap();
    assert_eq!(d, tmp.path().join("fthrust"));
    let mode = std::fs::metadata(&d).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    // Idempotent on an existing, valid directory.
    assert_eq!(runtime_dir().unwrap(), d);
}
