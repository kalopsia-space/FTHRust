//! Leftovers of crashed saves, recordings, exports and screenshots.

use fthrust_core::clip_files::{
    STALE_PARTIAL_MAX_AGE, cleanup_stale_partials, cleanup_stale_partials_in,
};
use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime};

const DAY: Duration = Duration::from_secs(24 * 3600);
const HEX32: &str = "0123456789abcdef0123456789abcdef";

fn touch(p: &Path, hours_old: u64) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    File::create(p).unwrap();
    age(p, hours_old);
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

fn sweep(root: &Path) -> usize {
    cleanup_stale_partials(root, SystemTime::now(), DAY).unwrap()
}

#[test]
fn the_default_age_is_a_day() {
    assert_eq!(STALE_PARTIAL_MAX_AGE, DAY);
}

#[test]
fn old_hidden_export_staging_files_are_removed_and_fresh_ones_kept() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let old = r.join(format!(
        ".desktop_clip_from_01Jan2026_10-00-00_export_abc.partial.{HEX32}.mp4"
    ));
    let old_mkv = r.join(format!(".x_share_10-00-00_abcd1234.partial.{HEX32}.mkv"));
    let sidecar = r.join(format!(
        ".x_clip_from_a.mp4.fthr-audio.partial.{HEX32}.json"
    ));
    let fresh = r.join(format!(".y.partial.{HEX32}.mp4"));
    // Lookalikes that are not the app's: no 32-hex token, not hidden, no stem.
    let short = r.join(".z.partial.deadbeef.mp4");
    let visible = r.join(format!("z.partial.{HEX32}.mp4"));
    let nameless = r.join(format!("..partial.{HEX32}.mp4"));
    for p in [
        &old, &old_mkv, &sidecar, &fresh, &short, &visible, &nameless,
    ] {
        touch(p, if *p == fresh { 1 } else { 30 });
    }
    assert_eq!(sweep(r), 3);
    assert!(!old.exists() && !old_mkv.exists() && !sidecar.exists());
    assert!(fresh.exists() && short.exists() && visible.exists() && nameless.exists());
}

#[test]
fn recording_partials_and_manifests_are_covered_but_the_recording_is_not() {
    let d = tempfile::tempdir().unwrap();
    let rec = d.path().join("Recordings");
    let partial = rec.join("FTHR_recording_2026-10-31_17-35-01.mp4.partial");
    let orphan = rec.join("FTHR_recording_2026-10-31_17-35-01.mp4.fthr-audio.json");
    let kept_manifest = rec.join("FTHR_recording_2026-10-31_18-00-00.mp4.fthr-audio.json");
    let recording = rec.join("FTHR_recording_2026-10-31_18-00-00.mp4");
    // A crashed recording is a recoverable fragmented MP4, not a leftover.
    let crashed = rec.join("FTHR_recording_2026-10-30_09-00-00.mp4");
    let not_ours = rec.join("holiday.mp4.partial");
    for p in [
        &partial,
        &orphan,
        &kept_manifest,
        &recording,
        &crashed,
        &not_ours,
    ] {
        touch(p, 30);
    }
    assert_eq!(sweep(d.path()), 2);
    assert!(!partial.exists() && !orphan.exists());
    assert!(kept_manifest.exists() && recording.exists() && crashed.exists());
    assert!(not_ours.exists());
}

#[test]
fn staged_screenshots_are_removed_when_old() {
    let d = tempfile::tempdir().unwrap();
    let shots = d.path().join("Screenshots");
    let old = shots.join(".screenshot_from_20261031_173501_000123.1a2b3c4d.png.partial");
    let fresh = shots.join(".screenshot_from_20261031_173502_000123.5e6f7a8b.png.partial");
    let user = shots.join(".holiday.png.partial");
    touch(&old, 30);
    touch(&fresh, 1);
    touch(&user, 30);
    assert_eq!(sweep(d.path()), 1);
    assert!(!old.exists() && fresh.exists() && user.exists());
}

#[test]
fn a_finished_clip_is_never_touched() {
    let d = tempfile::tempdir().unwrap();
    let clip = d
        .path()
        .join("Desktop/desktop_clip_from_31Oct2026_17-35-01.mp4");
    let shot = d
        .path()
        .join("Screenshots/screenshot_from_20261031_173501_000123.png");
    touch(&clip, 24 * 90);
    touch(&shot, 24 * 90);
    assert_eq!(sweep(d.path()), 0);
    assert!(clip.exists() && shot.exists());
}

#[test]
fn finalize_directories_go_only_when_nothing_in_them_is_recent() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path().join("Desktop");
    let dead = r.join(".fthr-finalize-0123456789ab");
    let live = r.join(".fthr-finalize-ba9876543210");
    let fresh_dir = r.join(".fthr-finalize-aaaaaaaaaaaa");
    let user = r.join(".fthr-notes");
    touch(&dead.join("camera.mkv"), 30);
    touch(&dead.join("output.mp4"), 30);
    touch(&live.join("camera.mkv"), 30);
    touch(&live.join("output.mp4"), 1); // still being written
    touch(&fresh_dir.join("output.mp4"), 1);
    touch(&user.join("x.txt"), 30);
    // A directory's own mtime changes only when entries come and go.
    for dir in [&dead, &live, &user] {
        let t = SystemTime::now() - Duration::from_secs(30 * 3600);
        File::open(dir).unwrap().set_modified(t).unwrap();
    }
    assert_eq!(sweep(d.path()), 1);
    assert!(!dead.exists());
    assert!(live.join("output.mp4").exists() && fresh_dir.exists() && user.exists());
}

#[test]
fn symlinks_are_not_followed_into_other_folders() {
    let d = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join(format!(".v.partial.{HEX32}.mp4"));
    touch(&victim, 30);
    std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap();
    std::os::unix::fs::symlink(&victim, d.path().join(format!(".w.partial.{HEX32}.mp4"))).unwrap();
    assert_eq!(sweep(d.path()), 0);
    assert!(victim.exists());
}

#[test]
fn several_roots_are_swept_once_each() {
    let d = tempfile::tempdir().unwrap();
    let clips = d.path().join("clips");
    let inside = clips.join("Recordings");
    let elsewhere = d.path().join("Videos/Recordings");
    let a = clips.join(format!(".a.partial.{HEX32}.mp4"));
    let b = inside.join("FTHR_recording_2026-10-31_17-35-01.mp4.partial");
    let c = elsewhere.join("FTHR_recording_2026-10-31_17-35-02.mp4.partial");
    for p in [&a, &b, &c] {
        touch(p, 30);
    }
    let missing = d.path().join("not-there");
    let roots = [clips.clone(), inside, elsewhere, missing];
    assert_eq!(cleanup_stale_partials_in(&roots, SystemTime::now(), DAY), 3);
    assert!(!a.exists() && !b.exists() && !c.exists());
}
