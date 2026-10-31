//! Thumbnail results wake the UI.

#[path = "../../fthrust-media/tests/support/mod.rs"]
mod support;

use std::path::PathBuf;
use std::time::Duration;

use fthrust_ui::library::thumbs::{FAILED_RETRY_S, ThumbFailures, ThumbImage, ThumbPool};

#[test]
fn thumbnail_results_request_a_repaint() {
    let tmp = tempfile::tempdir().unwrap();
    let png = tmp.path().join("shot.png");
    image::RgbaImage::from_pixel(32, 32, image::Rgba([9, 8, 7, 255]))
        .save(&png)
        .unwrap();
    let ctx = egui::Context::default();
    let pool = ThumbPool::start(tmp.path().join("cache"));
    pool.set_repaint(ctx.clone());
    // A fresh context may already carry egui's initial repaint request:
    // run one pass so the flag reflects only the worker's wake-up.
    let mut out = ctx.run_ui(egui::RawInput::default(), |_ui| {});
    out.textures_delta.clear();
    assert!(pool.request(&png, "fp1", false));
    let result = pool
        .results()
        .recv_timeout(Duration::from_secs(10))
        .expect("a result");
    assert_eq!(result.path, png);
    assert!(ctx.has_requested_repaint(), "the worker woke the UI");
}

#[test]
fn video_thumbnails_arrive_decoded_so_the_ui_thread_reads_no_file() {
    let tmp = tempfile::tempdir().unwrap();
    let clip = tmp.path().join("clip.mp4");
    if support::write_test_clip(&clip, 1, false).is_none() {
        return;
    }
    let pool = ThumbPool::start(tmp.path().join("cache"));
    assert!(pool.request(&clip, "fp", true));
    let result = pool
        .results()
        .recv_timeout(Duration::from_secs(30))
        .expect("a result");
    match result.image {
        Some(ThumbImage::Video(cached, Some(rgba))) => {
            let on_disk = image::open(&cached.jpg).unwrap();
            assert!(rgba.width() > 0 && rgba.height() > 0);
            assert_eq!(
                (rgba.width(), rgba.height()),
                (on_disk.width(), on_disk.height()),
                "the pixels are the cached JPEG's"
            );
        }
        other => panic!("expected a decoded poster, got {other:?}"),
    }
}

#[test]
fn a_damaged_cached_poster_is_rebuilt_with_its_details_and_pixels() {
    let tmp = tempfile::tempdir().unwrap();
    let clip = tmp.path().join("clip.mp4");
    if support::write_test_clip(&clip, 1, false).is_none() {
        return;
    }
    let pool = ThumbPool::start(tmp.path().join("cache"));
    let wait = |pool: &ThumbPool| {
        pool.results()
            .recv_timeout(Duration::from_secs(30))
            .expect("a result")
    };
    assert!(pool.request(&clip, "fp", true));
    let Some(ThumbImage::Video(first, Some(_))) = wait(&pool).image else {
        panic!("expected a poster");
    };
    // The cached JPEG is damaged (a disk error, a truncated write).
    std::fs::write(&first.jpg, b"damaged").unwrap();
    assert!(pool.request(&clip, "fp", true));
    match wait(&pool).image {
        Some(ThumbImage::Video(again, Some(rgba))) => {
            assert_eq!(again.meta, first.meta, "the same details");
            assert!(rgba.width() > 0, "pixels again");
            assert!(
                image::open(&again.jpg).is_ok(),
                "the cache file was rebuilt, not left damaged"
            );
        }
        other => panic!("expected a rebuilt poster, got {other:?}"),
    }
}

#[test]
fn retain_drops_the_queued_jobs_of_cards_that_left_the_page() {
    let tmp = tempfile::tempdir().unwrap();
    // No workers: every job stays queued.
    let pool = ThumbPool::start_with(tmp.path().join("cache"), 0);
    let paths: Vec<PathBuf> = (0..5)
        .map(|i| tmp.path().join(format!("clip_{i}.mp4")))
        .collect();
    for p in &paths {
        assert!(pool.request(p, "fp", true));
    }
    assert_eq!(pool.inflight(), 5);
    let dropped = pool.retain(&paths[3..]);
    assert_eq!(
        dropped,
        paths[..3].to_vec(),
        "the stale jobs, in queue order"
    );
    assert_eq!(pool.inflight(), 2);
    assert!(
        pool.request(&paths[0], "fp", true),
        "a dropped clip can be requested again"
    );
}

#[test]
fn a_failed_thumbnail_is_tried_again_when_the_file_changes_or_after_a_while() {
    let path = PathBuf::from("/clips/a.mp4");
    let mut failed = ThumbFailures::default();
    assert!(!failed.blocks(&path, "10|1", 0.0), "nothing recorded");
    failed.insert(path.clone(), "10|1".into(), 100.0);
    assert!(failed.contains(&path));
    assert!(failed.blocks(&path, "10|1", 101.0), "unchanged and recent");
    assert!(
        failed.blocks(&path, "10|1", 100.0 + FAILED_RETRY_S - 0.1),
        "still held just before the retry time"
    );
    assert!(!failed.blocks(&path, "10|1", 100.0 + FAILED_RETRY_S), "due");
    assert!(!failed.contains(&path), "the expired entry is gone");
    failed.insert(path.clone(), "10|1".into(), 200.0);
    assert!(
        !failed.blocks(&path, "11|2", 201.0),
        "the file was rewritten: try at once"
    );
    assert!(failed.is_empty());
}
