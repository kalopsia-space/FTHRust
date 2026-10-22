//! Plan 5 Task 3: the library thumbnail cache (`ui/clip_grid.py` `_ThumbnailWorker`).

mod support;

use std::sync::atomic::AtomicBool;

use fthrust_media::thumbnail::library::{cache_key, image_thumbnail, load_or_build};

#[test]
fn thumbnail_cache_miss_writes_jpg_meta_and_hit_skips_probe() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("clip.mp4");
    if support::write_test_clip(&clip, 2, false).is_none() {
        eprintln!("no software encoder; skipped");
        return;
    }
    let cache = d.path().join("thumbs");
    let cancel = AtomicBool::new(false);
    let first = load_or_build(&cache, &clip, &cancel)
        .unwrap()
        .expect("thumbnail built");
    assert!(first.jpg.exists() && first.jpg.extension().unwrap() == "jpg");
    let key = cache_key(&clip).unwrap();
    assert_eq!(
        first.jpg.file_name().unwrap().to_str().unwrap(),
        format!("{key}.jpg")
    );
    assert!(cache.join(format!("{key}.meta-v3")).exists());
    assert_eq!((first.meta.width, first.meta.height), (160, 90));
    assert!(
        (first.meta.duration - 2.0).abs() < 0.2,
        "{}",
        first.meta.duration
    );
    assert!((first.duration_s - first.meta.duration).abs() < f64::EPSILON);
    let img = image::open(&first.jpg).unwrap();
    assert_eq!(
        (img.width(), img.height()),
        (160, 90),
        "fitted within 640×360, never enlarged"
    );
    let mtime = std::fs::metadata(&first.jpg).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let started = std::time::Instant::now();
    let second = load_or_build(&cache, &clip, &cancel).unwrap().unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_millis(50),
        "cache hit does not decode"
    );
    assert_eq!(
        std::fs::metadata(&second.jpg).unwrap().modified().unwrap(),
        mtime
    );
    assert!(
        !std::fs::read_dir(&cache).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-"))
    );
}

#[test]
fn thumbnail_failed_marker_short_circuits_for_30s() {
    let d = tempfile::tempdir().unwrap();
    let bogus = d.path().join("broken.mp4");
    std::fs::write(&bogus, b"this is not a video").unwrap();
    let cache = d.path().join("thumbs");
    let cancel = AtomicBool::new(false);
    assert!(load_or_build(&cache, &bogus, &cancel).unwrap().is_none());
    let key = cache_key(&bogus).unwrap();
    let failed = cache.join(format!("{key}.failed"));
    assert!(failed.exists(), "negative cache marker written");
    let marker_mtime = std::fs::metadata(&failed).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(load_or_build(&cache, &bogus, &cancel).unwrap().is_none());
    assert_eq!(
        std::fs::metadata(&failed).unwrap().modified().unwrap(),
        marker_mtime,
        "no second probe within 30 s"
    );
    // Cancellation returns without touching the cache.
    let cancelled = AtomicBool::new(true);
    let other = d.path().join("other.mp4");
    std::fs::write(&other, b"x").unwrap();
    assert!(load_or_build(&cache, &other, &cancelled).unwrap().is_none());
    assert!(
        !cache
            .join(format!("{}.failed", cache_key(&other).unwrap()))
            .exists()
    );
}

#[test]
fn image_thumbnail_fits_within_640x360() {
    let d = tempfile::tempdir().unwrap();
    let png = d.path().join("shot.png");
    image::RgbaImage::from_pixel(1920, 1080, image::Rgba([10, 20, 30, 255]))
        .save(&png)
        .unwrap();
    let thumb = image_thumbnail(&png).unwrap();
    assert_eq!((thumb.width(), thumb.height()), (640, 360));
    let small = d.path().join("small.png");
    image::RgbaImage::from_pixel(100, 50, image::Rgba([1, 2, 3, 255]))
        .save(&small)
        .unwrap();
    assert_eq!(image_thumbnail(&small).unwrap().dimensions(), (100, 50));
}
