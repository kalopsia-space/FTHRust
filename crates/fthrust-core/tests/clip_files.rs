use fthrust_core::clip_files::*;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use time::macros::datetime;

fn fixture() -> serde_json::Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/clip-files.json"
    );
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn completed_video_cases_match_fixture() {
    let f = fixture();
    assert_eq!(f["partial_marker"], PARTIAL_MARKER);
    for c in f["completed_video_cases"].as_array().unwrap() {
        let name = c[0].as_str().unwrap();
        assert_eq!(
            is_completed_video_path(Path::new(name)),
            c[1].as_bool().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn sanitize_cases_match_fixture() {
    for c in fixture()["sanitize_cases"].as_array().unwrap() {
        let (i, o) = (c[0].as_str().unwrap(), c[1].as_str().unwrap());
        assert_eq!(sanitize_folder_name(i), o, "{i:?}");
    }
}

#[test]
fn replay_path_desktop_format() {
    let p = replay_clip_path(Path::new("/c"), None, datetime!(2026-08-25 14:03:07 UTC));
    assert_eq!(
        p,
        PathBuf::from("/c/Desktop/desktop_clip_from_25Aug2026_14-03-07.mp4")
    );
}

#[test]
fn replay_path_game_folder() {
    let p = replay_clip_path(
        Path::new("/c"),
        Some("Over:watch"),
        datetime!(2026-08-25 14:03:07 UTC),
    );
    assert_eq!(
        p,
        PathBuf::from("/c/Overwatch/Overwatch_clip_from_25Aug2026_14-03-07.mp4")
    );
}

#[test]
fn unique_path_suffixes() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.mp4");
    assert_eq!(unique_path(p.clone()), p);
    File::create(&p).unwrap();
    assert_eq!(unique_path(p.clone()), d.path().join("a_2.mp4"));
    File::create(d.path().join("a_2.mp4")).unwrap();
    assert_eq!(unique_path(p), d.path().join("a_3.mp4"));
}

#[test]
fn other_names() {
    let at = datetime!(2026-08-25 14:03:07.000123 UTC);
    assert_eq!(
        recording_path(Path::new("/r"), at),
        PathBuf::from("/r/FTHR_recording_2026-08-25_14-03-07.mp4")
    );
    let s = share_path(Path::new("/c"), "x", ShareKind::Discord, at, true);
    let n = s.file_name().unwrap().to_str().unwrap();
    assert!(
        s.starts_with("/c/Shared") && n.starts_with("x_discord_14-03-07_") && n.ends_with(".mkv")
    );
    assert_eq!(n.len(), "x_discord_14-03-07_".len() + 8 + 4);
    let s = share_path(Path::new("/c"), "x", ShareKind::Share, at, false);
    assert!(s.to_str().unwrap().contains("x_share_14-03-07_") && s.extension().unwrap() == "mp4");
    assert!(is_fthr_post_processing_dir("Exported") && is_fthr_post_processing_dir(".fthr-tmp"));
    assert!(!is_fthr_post_processing_dir("Desktop"));
}

#[test]
fn screenshot_path_has_microseconds() {
    let p = screenshot_path(Path::new("/c"), datetime!(2026-08-25 14:03:07.000123 UTC));
    assert_eq!(
        p,
        PathBuf::from("/c/Screenshots/screenshot_from_20260825_140307_000123.png")
    );
}

fn age(p: &Path, hours: u64) {
    let t = SystemTime::now() - Duration::from_secs(hours * 3600);
    File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

#[test]
fn cleanup_removes_only_old_fthr_partials() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let old = r.join("x_clip_from_a.mp4.partial");
    let fresh = r.join("x_clip_from_b.mp4.partial");
    let other = r.join("foo.mp4.partial");
    let orphan = r.join("x_clip_from_c.mp4.fthr-audio.json");
    let paired = r.join("x_clip_from_d.mp4.fthr-audio.json");
    for p in [
        &old,
        &fresh,
        &other,
        &orphan,
        &paired,
        &r.join("x_clip_from_d.mp4"),
    ] {
        File::create(p).unwrap();
    }
    for p in [&old, &other, &orphan, &paired] {
        age(p, 25);
    }
    let n = cleanup_stale_partials(r, SystemTime::now(), Duration::from_secs(24 * 3600)).unwrap();
    assert_eq!(n, 2);
    assert!(!old.exists() && !orphan.exists());
    assert!(fresh.exists() && other.exists() && paired.exists());
}

#[test]
fn cleanup_does_not_follow_symlinks() {
    let d = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("x_clip_from_a.mp4.partial");
    File::create(&victim).unwrap();
    age(&victim, 25);
    std::os::unix::fs::symlink(&victim, d.path().join("y_clip_from_a.mp4.partial")).unwrap();
    std::os::unix::fs::symlink(outside.path(), d.path().join("dir")).unwrap();
    let n = cleanup_stale_partials(d.path(), SystemTime::now(), Duration::from_secs(24 * 3600))
        .unwrap();
    assert_eq!(n, 0);
    assert!(victim.exists());
}
