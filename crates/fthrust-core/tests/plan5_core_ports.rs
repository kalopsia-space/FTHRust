//! Plan 5 Task 1: clip readiness, screenshot staging, game rules, instance lock
//! (assertions ported from `reference/tests/test_clip_readiness.py`,
//! `test_screenshot_save.py`, `test_single_instance.py`).

use fthrust_core::clip_readiness::{ClipReadinessRegistry, ReadinessState};
use fthrust_core::game_rules::{
    CustomGameRule, is_blacklisted, matching_rule, normalise_rules, to_value,
};
use fthrust_core::screenshot_save::{
    MAX_PNG_BYTES, publish_staged_png, reserve_screenshot_paths, validate_png_bytes,
};
use fthrust_core::single_instance::{ACTIVATE, InstanceLock, LockResult, activation_socket_path};
use serde_json::json;
use time::macros::datetime;

#[test]
fn readiness_untracked_existing_is_ready() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("old.mp4");
    std::fs::write(&clip, b"old complete clip").unwrap();
    let r = ClipReadinessRegistry::new();
    assert!(r.can_access(&clip));
    assert_eq!(r.state(&clip), ReadinessState::Ready);
    assert_eq!(
        r.state(&d.path().join("missing.mp4")),
        ReadinessState::FinalizationFailed
    );
}

#[test]
fn readiness_finalizing_then_warning_is_ready_with_warning() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("clip.mp4");
    std::fs::write(&clip, b"base").unwrap();
    let mut r = ClipReadinessRegistry::new();
    assert_eq!(r.engine_committed(&clip, true), ReadinessState::Finalizing);
    assert!(!r.can_access(&clip));
    r.record_warning(&clip, "watermark failed");
    r.complete(&clip);
    assert_eq!(r.state(&clip), ReadinessState::ReadyWithWarning);
    assert!(r.can_access(&clip));
    assert_eq!(r.warnings(&clip), vec!["watermark failed".to_string()]);
    // A usable base after an optional failure stays accessible; an unusable one never is.
    r.finalization_failed(&clip, "microphone mix failed", true);
    assert_eq!(r.state(&clip), ReadinessState::ReadyWithWarning);
    let other = d.path().join("other.mp4");
    r.engine_committed(&other, true);
    r.finalization_failed(&other, "file missing", false);
    assert_eq!(r.state(&other), ReadinessState::FinalizationFailed);
    assert!(!r.can_access(&other));
    r.forget(&other);
    assert!(!r.is_tracked(&other));
    // Ready without finalization.
    let quick = d.path().join("quick.mp4");
    assert_eq!(r.engine_committed(&quick, false), ReadinessState::Ready);
    r.ready(&other);
    assert_eq!(r.state(&other), ReadinessState::Ready);
}

#[test]
fn screenshot_reserve_names_and_collision_suffix_01() {
    let d = tempfile::tempdir().unwrap();
    let now = datetime!(2026-08-23 12:34:56.123456 UTC);
    let first = reserve_screenshot_paths(d.path(), now).unwrap();
    assert_eq!(
        first.final_path.file_name().unwrap(),
        "screenshot_from_20260823_123456_123456.png"
    );
    assert!(first.staged.exists(), "staged file is reserved on disk");
    assert!(
        first
            .staged
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(".png.partial")
    );
    assert_eq!(first.staged.parent().unwrap(), d.path());
    std::fs::remove_file(&first.staged).unwrap();
    std::fs::write(&first.final_path, b"existing").unwrap();
    let second = reserve_screenshot_paths(d.path(), now).unwrap();
    assert_eq!(
        second.final_path.file_name().unwrap(),
        "screenshot_from_20260823_123456_123456_01.png"
    );
    let third = reserve_screenshot_paths(d.path(), now).unwrap();
    assert_ne!(
        third.staged, second.staged,
        "every reservation gets its own staging file"
    );
    let nested = d.path().join("Screenshots");
    assert!(
        reserve_screenshot_paths(&nested, now).is_ok(),
        "the directory is created"
    );
    let bad = d.path().join("file-not-dir");
    std::fs::write(&bad, b"x").unwrap();
    assert_eq!(
        reserve_screenshot_paths(&bad, now).unwrap_err().code,
        "OUTPUT_DIRECTORY_UNAVAILABLE"
    );
}

#[test]
fn screenshot_publish_refuses_existing() {
    let d = tempfile::tempdir().unwrap();
    let now = datetime!(2026-08-23 00:00:00 UTC);
    let paths = reserve_screenshot_paths(d.path(), now).unwrap();
    std::fs::write(&paths.staged, b"png bytes").unwrap();
    std::fs::write(&paths.final_path, b"external file").unwrap();
    let err = publish_staged_png(&paths.staged, &paths.final_path).unwrap_err();
    assert_eq!(err.code, "OUTPUT_COLLISION");
    assert_eq!(std::fs::read(&paths.final_path).unwrap(), b"external file");
    assert!(
        paths.staged.is_file(),
        "the staged file is left for the caller"
    );
    let paths2 = reserve_screenshot_paths(d.path(), now).unwrap();
    std::fs::write(&paths2.staged, b"png bytes").unwrap();
    publish_staged_png(&paths2.staged, &paths2.final_path).unwrap();
    assert!(!paths2.staged.exists());
    assert_eq!(std::fs::read(&paths2.final_path).unwrap(), b"png bytes");
}

#[test]
fn screenshot_png_limits() {
    let png = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
    let mut bytes = Vec::new();
    png.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    assert_eq!(validate_png_bytes(&bytes).unwrap(), (2, 2));
    assert_eq!(
        validate_png_bytes(b"not a png").unwrap_err().code,
        "INVALID_IMAGE"
    );
    let huge = vec![0u8; MAX_PNG_BYTES as usize + 1];
    assert_eq!(validate_png_bytes(&huge).unwrap_err().code, "INVALID_IMAGE");
}

#[test]
fn game_rules_round_trip_and_match() {
    let raw = json!([
        {"title_contains": " Overwatch ", "exe_path": "", "exe_name": ""},
        {"title_contains": "overwatch"},
        {"app_id": "steam_app_2357570", "crop_profile": {"x": 0.1, "y": 0.2, "w": 0.5, "h": 0.5}},
        {"exe_path": "C:/Games/game.exe"},
        {"title_contains": "   "},
        "junk"
    ]);
    let rules = normalise_rules(&raw);
    assert_eq!(
        rules.len(),
        3,
        "trimmed, de-duplicated case-insensitively, empties dropped: {rules:?}"
    );
    assert_eq!(
        rules[0],
        CustomGameRule {
            title_contains: "Overwatch".into(),
            ..CustomGameRule::default()
        }
    );
    assert_eq!(rules[1].app_id, "steam_app_2357570");
    let crop = rules[1].crop.unwrap();
    assert_eq!(
        (crop.x, crop.y, crop.w, crop.h, crop.enabled),
        (0.1, 0.2, 0.5, 0.5, true)
    );
    assert_eq!(
        rules[2].exe_name, "game.exe",
        "exe_name is derived from exe_path"
    );
    assert_eq!(
        matching_rule(&rules, "Overwatch 2", "").map(|r| r.title_contains.as_str()),
        Some("Overwatch")
    );
    assert_eq!(
        matching_rule(&rules, "Some Game", "STEAM_APP_2357570").map(|r| r.app_id.as_str()),
        Some("steam_app_2357570")
    );
    assert!(
        matching_rule(&rules, "Firefox", "org.mozilla.firefox").is_none(),
        "exe-only rules never match on Wayland"
    );
    assert!(is_blacklisted(&rules, "overwatch 2", "x"));
    let back = to_value(&rules);
    assert_eq!(back[0]["title_contains"], "Overwatch");
    assert_eq!(back[1]["app_id"], "steam_app_2357570");
    assert_eq!(back[1]["crop_profile"]["w"], 0.5);
    assert!(
        back[0].get("app_id").is_none() || back[0]["app_id"] == "",
        "empty fields stay out or empty"
    );
}

#[test]
fn instance_lock_second_acquire_is_held() {
    let d = tempfile::tempdir().unwrap();
    let lock = d.path().join("fthr.lock");
    let first = InstanceLock::acquire(&lock);
    let LockResult::Acquired(held) = first else {
        panic!("first acquire")
    };
    // flock is per open file description: a second open+flock from this process is refused.
    assert!(matches!(InstanceLock::acquire(&lock), LockResult::Held));
    drop(held);
    assert!(matches!(
        InstanceLock::acquire(&lock),
        LockResult::Acquired(_)
    ));
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap().trim(),
        std::process::id().to_string()
    );
    assert_eq!(ACTIVATE, b"activate\n");
    assert_eq!(
        activation_socket_path(std::path::Path::new("/run/user/1000/fthrust")),
        std::path::PathBuf::from("/run/user/1000/fthrust/instance.sock")
    );
}

#[test]
fn instance_lock_fail_open_on_unwritable_dir() {
    let lock = std::path::Path::new("/proc/no-such-dir/fthr.lock");
    assert!(matches!(
        InstanceLock::acquire(lock),
        LockResult::Unavailable
    ));
}

#[test]
fn screenshot_publish_moves_the_staged_file() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("Screenshots");
    let paths =
        reserve_screenshot_paths(&dir, time::macros::datetime!(2026-10-23 12:00:00 UTC)).unwrap();
    std::fs::write(&paths.staged, b"png").unwrap();
    publish_staged_png(&paths.staged, &paths.final_path).unwrap();
    assert!(paths.final_path.exists());
    assert!(!paths.staged.exists(), "no .partial left behind");
    // The copy fallback (filesystems without links or no-replace renames)
    // behaves the same.
    let paths2 =
        reserve_screenshot_paths(&dir, time::macros::datetime!(2026-10-23 12:00:01 UTC)).unwrap();
    std::fs::write(&paths2.staged, b"png2").unwrap();
    fthrust_core::screenshot_save::publish_by_copy(&paths2.staged, &paths2.final_path).unwrap();
    assert_eq!(std::fs::read(&paths2.final_path).unwrap(), b"png2");
    assert!(!paths2.staged.exists());
    std::fs::write(&paths2.staged, b"again").unwrap();
    let err = fthrust_core::screenshot_save::publish_by_copy(&paths2.staged, &paths2.final_path)
        .unwrap_err();
    assert_eq!(err.code, "OUTPUT_COLLISION");
    assert_eq!(std::fs::read(&paths2.final_path).unwrap(), b"png2");
}
