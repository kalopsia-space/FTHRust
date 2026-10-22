use fthrust_core::thumbnails::{
    MetaV3, failed_marker_is_fresh, paths_for, prune, prune_with_limits, temp_path_for,
    thumbnail_key,
};
use md5::{Digest, Md5};
use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime};

fn set_age(path: &Path, now: SystemTime, secs: u64) {
    let f = File::options().write(true).open(path).unwrap();
    f.set_modified(now - Duration::from_secs(secs)).unwrap();
}

#[test]
fn key_matches_python_md5() {
    let key = thumbnail_key(Path::new("/a/b.mp4"), 10, 20);
    let expected = Md5::digest(b"/a/b.mp4|10|20|aspect-v4")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(key, expected);
    assert_eq!(key, "c7371e0462033ef582c7e776e54f5220");
}

#[test]
fn key_uses_raw_path_bytes() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let key = thumbnail_key(Path::new(OsStr::from_bytes(b"/a/\xff.mp4")), 10, 20);
    let expected: String = Md5::digest(b"/a/\xff.mp4|10|20|aspect-v4")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(key, expected);
}

#[test]
fn meta_round_trip_six_decimals() {
    let m = MetaV3 {
        duration: 12.5,
        width: 1920,
        height: 1080,
        fps: 29.97,
        video_bitrate: 4_000_000,
        total_bitrate: 4_128_000,
    };
    let text = m.to_text();
    assert_eq!(text, "12.500000 1920 1080 29.970000 4000000 4128000");
    assert_eq!(MetaV3::parse(&text), Some(m));
    assert_eq!(MetaV3::parse("1.0 2 3"), None);
    assert_eq!(MetaV3::parse("a 1 2 3.0 4 5"), None);
    for bad in [
        "0 1 2 3.0 4 5",
        "-1.0 1 2 3.0 4 5",
        "inf 1 2 3.0 4 5",
        "NaN 1 2 3.0 4 5",
        "1.0 0 2 3.0 4 5",
        "1.0 1 0 3.0 4 5",
        "1.0 1 2 -1.0 4 5",
        "1.0 1 2 inf 4 5",
        "1.0 1 2 3.0 -4 5",
        "1.0 1 2 3.0 4 -5",
    ] {
        assert_eq!(MetaV3::parse(bad), None, "{bad}");
    }
}

#[test]
fn prune_evicts_oldest_groups_together() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    for (key, age) in [("old", 300), ("mid", 200), ("new", 100)] {
        let p = paths_for(dir.path(), key);
        fs::write(&p.jpg, b"0123456789").unwrap();
        fs::write(&p.meta, b"m").unwrap();
        set_age(&p.jpg, now, age);
        set_age(&p.meta, now, age);
    }
    let mut last = None;
    prune_with_limits(dir.path(), now, &mut last, u64::MAX, 2).unwrap();
    let old = paths_for(dir.path(), "old");
    assert!(!old.jpg.exists() && !old.meta.exists());
    assert!(paths_for(dir.path(), "mid").jpg.exists());
    assert!(paths_for(dir.path(), "new").meta.exists());
    assert_eq!(last, Some(now));

    // Byte limit: each remaining group is 11 bytes; allow only one.
    let mut last = None;
    prune_with_limits(dir.path(), now, &mut last, 15, 100).unwrap();
    assert!(!paths_for(dir.path(), "mid").jpg.exists());
    assert!(!paths_for(dir.path(), "mid").meta.exists());
    assert!(paths_for(dir.path(), "new").jpg.exists());
}

#[test]
fn prune_rate_limited() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let p = paths_for(dir.path(), "k");
    fs::write(&p.jpg, b"x").unwrap();
    let mut last = Some(now - Duration::from_secs(10));
    prune_with_limits(dir.path(), now, &mut last, 0, 1).unwrap();
    assert!(p.jpg.exists());
    assert_eq!(last, Some(now - Duration::from_secs(10)));
    let mut last = Some(now - Duration::from_secs(31));
    prune_with_limits(dir.path(), now, &mut last, 0, 1).unwrap();
    assert!(!p.jpg.exists());
    assert_eq!(last, Some(now));
    // Default-limit entry point is a no-op on a small cache.
    fs::write(&p.jpg, b"x").unwrap();
    let mut last = None;
    prune(dir.path(), now, &mut last).unwrap();
    assert!(p.jpg.exists());
}

#[test]
fn failed_marker_expires() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let p = paths_for(dir.path(), "k");
    assert!(!failed_marker_is_fresh(&p.failed, now));
    fs::write(&p.failed, b"").unwrap();
    set_age(&p.failed, now, 5);
    assert!(failed_marker_is_fresh(&p.failed, now));
    set_age(&p.failed, now, 31);
    assert!(!failed_marker_is_fresh(&p.failed, now));
    let t = temp_path_for(&p.meta);
    let s = t.to_string_lossy();
    assert!(s.starts_with(&*p.meta.to_string_lossy()));
    assert!(s.contains(&format!(".tmp-{}-", std::process::id())));
}
