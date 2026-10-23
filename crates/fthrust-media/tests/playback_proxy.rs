//! Plan 6a Task 3: the seek-proxy cache.

mod support;

use std::sync::atomic::AtomicBool;

use fthrust_media::playback::proxy::{
    KeyframeSample, cache_path, discard, needs_seek_proxy, prepare, probe, prune,
};
use support::write_gop_clip;

fn k(pts_s: f64, keyframe: bool) -> KeyframeSample {
    KeyframeSample { pts_s, keyframe }
}

#[test]
fn needs_proxy_only_for_long_keyframe_gaps() {
    assert!(needs_seek_proxy(&[
        k(0.0, true),
        k(1.0, false),
        k(2.0, true),
        k(3.0, false),
        k(4.5, false),
        k(4.6, true)
    ]));
    assert!(!needs_seek_proxy(&[
        k(0.0, true),
        k(1.5, true),
        k(3.0, true)
    ]));
    assert!(!needs_seek_proxy(&[
        k(0.5, false),
        k(1.0, true),
        k(2.0, true)
    ]));
    assert!(!needs_seek_proxy(&[]));
    assert!(
        needs_seek_proxy(&[k(0.0, true), k(2.2, false)]),
        "one keyframe and a frame 2.2 s later"
    );
}

#[test]
fn cache_path_changes_with_identity() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a.mp4");
    let b = d.path().join("b.mp4");
    std::fs::write(&a, b"aaaa").unwrap();
    std::fs::write(&b, b"bbbbbb").unwrap();
    let cache = d.path().join("cache");
    let pa = cache_path(&cache, &a).unwrap();
    let pb = cache_path(&cache, &b).unwrap();
    assert_ne!(pa, pb);
    assert_eq!(pa, cache_path(&cache, &a).unwrap());
    let name = pa.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(name.len(), 64 + 4);
    assert!(name.ends_with(".mp4") && name[..64].chars().all(|c| c.is_ascii_hexdigit()));
    assert!(pa.starts_with(&cache));
}

#[test]
fn prepare_builds_a_proxy_for_a_long_gop_clip() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("long.mp4");
    let Some(()) = write_gop_clip(&clip, 4, None) else {
        return;
    };
    let cache = d.path().join("cache");
    let p = probe(&clip, 8.0).unwrap();
    let keyframes: Vec<f64> = p
        .samples
        .iter()
        .filter(|s| s.keyframe)
        .map(|s| s.pts_s)
        .collect();
    assert!(
        needs_seek_proxy(&p.samples),
        "a long GOP has one keyframe; found keyframes at {keyframes:?}"
    );
    assert!(!p.hdr);
    assert!((p.duration_s - 4.0).abs() < 0.2, "{}", p.duration_s);
    let cancel = AtomicBool::new(false);
    let proxy = prepare(&cache, &clip, &cancel);
    assert_ne!(proxy, clip, "a proxy was built");
    assert!(proxy.starts_with(&cache) && proxy.exists());
    let pp = probe(&proxy, 8.0).unwrap();
    assert!(!needs_seek_proxy(&pp.samples), "the proxy seeks well");
    let mut last = None;
    for s in pp.samples.iter().filter(|s| s.keyframe) {
        if let Some(l) = last {
            assert!(s.pts_s - l <= 1.1, "keyframe gap {}", s.pts_s - l);
        }
        last = Some(s.pts_s);
    }
    assert!((pp.duration_s - p.duration_s).abs() <= 0.2);
    assert_eq!(
        fthrust_media::playback::best_audio_stream(&proxy),
        None,
        "the proxy is video-only; audio always decodes the original"
    );
    assert!(
        std::fs::read_dir(&cache).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".partial")),
        "no partial left behind"
    );
    let mtime = std::fs::metadata(&proxy).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert_eq!(prepare(&cache, &clip, &cancel), proxy, "cache hit");
    assert_eq!(
        std::fs::metadata(&proxy).unwrap().modified().unwrap(),
        mtime,
        "not rebuilt"
    );
    discard(&cache, &clip, &proxy);
    assert!(!proxy.exists());
}

#[test]
fn prepare_keeps_the_source_for_short_gop_and_cancel() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("short.mp4");
    let Some(()) = write_gop_clip(&clip, 2, Some(30)) else {
        return;
    };
    let cache = d.path().join("cache");
    let cancel = AtomicBool::new(false);
    assert_eq!(prepare(&cache, &clip, &cancel), clip);
    assert!(!cache.exists() || std::fs::read_dir(&cache).unwrap().count() == 0);
    let long = d.path().join("long.mp4");
    let Some(()) = write_gop_clip(&long, 3, None) else {
        return;
    };
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        prepare(&cache, &long, &cancelled),
        long,
        "cancelled before the build"
    );
}

#[test]
fn partials_are_pruned_and_entries_capped() {
    let d = tempfile::tempdir().unwrap();
    let cache = d.path().join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    for i in 0..9 {
        let p = cache.join(format!("{:064x}.mp4", i + 1));
        std::fs::write(&p, vec![0u8; 10]).unwrap();
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(100 - i);
        filetime_set(&p, t);
    }
    // A partial from a crashed build (old) goes; one still being written stays.
    let stale = cache.join("deadbeef.123-456.partial.mp4");
    std::fs::write(&stale, b"x").unwrap();
    filetime_set(
        &stale,
        std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600),
    );
    let live = cache.join("deadbeef.123-457.partial.mp4");
    std::fs::write(&live, b"x").unwrap();
    prune(&cache, 8, 2 * 1024 * 1024 * 1024);
    let names: Vec<String> = std::fs::read_dir(&cache)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 9, "{names:?}");
    assert!(!stale.exists(), "stale partial pruned");
    assert!(
        live.exists(),
        "a fresh partial is another build in progress"
    );
    assert!(
        !names.contains(&format!("{:064x}.mp4", 1)),
        "the oldest entry went"
    );
}

fn filetime_set(p: &std::path::Path, t: std::time::SystemTime) {
    let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
    f.set_modified(t).unwrap();
}

#[test]
fn prepare_recovers_from_a_crashed_build() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("long.mp4");
    let Some(()) = write_gop_clip(&clip, 3, None) else {
        return;
    };
    let cache = d.path().join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let target = fthrust_media::playback::proxy::cache_path(&cache, &clip).unwrap();
    let stem = target.file_stem().unwrap().to_string_lossy().into_owned();
    // What a crash leaves: a partial of this clip from a dead process.
    let stale = cache.join(format!("{stem}.4242-1.partial.mp4"));
    std::fs::write(&stale, b"garbage").unwrap();
    filetime_set(
        &stale,
        std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600),
    );
    let cancel = AtomicBool::new(false);
    let proxy = prepare(&cache, &clip, &cancel);
    assert_eq!(proxy, target, "the proxy is built despite the leftover");
    assert!(!stale.exists());
    // A truncated final file (crash between write and validation) is rebuilt, not used.
    std::fs::write(&target, b"short").unwrap();
    let proxy = prepare(&cache, &clip, &cancel);
    assert_eq!(proxy, target);
    assert!(std::fs::metadata(&target).unwrap().len() > 1000, "rebuilt");
}
