mod support;

use fthrust_media::thumbnail::{
    POSTER_MAX_HEIGHT, POSTER_MAX_WIDTH, extract_frame, fit_within, poster,
};

#[test]
fn fit_within_matches_scale_filter_semantics() {
    // scale=w='min(640,iw)':h='min(360,ih)':force_original_aspect_ratio=decrease
    assert_eq!((POSTER_MAX_WIDTH, POSTER_MAX_HEIGHT), (640, 360));
    assert_eq!(fit_within(1920, 1080, 640, 360), (640, 360));
    assert_eq!(fit_within(2560, 1080, 640, 360), (640, 270));
    assert_eq!(fit_within(800, 1200, 640, 360), (240, 360));
    assert_eq!(fit_within(100, 50, 640, 360), (100, 50), "never upscaled");
    assert_eq!(fit_within(640, 360, 640, 360), (640, 360));
    assert_eq!(fit_within(1, 1, 640, 360), (1, 1));
    assert_eq!(
        fit_within(0, 0, 640, 360),
        (1, 1),
        "degenerate input still yields a pixel"
    );
}

#[test]
fn thumbnail_from_written_clip_has_expected_size() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("clip.mp4");
    if support::write_test_clip(&path, 3, false).is_none() {
        return;
    }
    let img = poster(&path).unwrap();
    assert_eq!(
        (img.width(), img.height()),
        (160, 90),
        "160×90 is already within 640×360"
    );
    let small = extract_frame(&path, 0.0, 80, 360).unwrap();
    assert_eq!((small.width(), small.height()), (80, 45));
    // A later frame differs from the first one (the synthetic gradient moves).
    let later = extract_frame(&path, 2.0, 640, 360).unwrap();
    assert_eq!((later.width(), later.height()), (160, 90));
    let diff = img
        .pixels()
        .zip(later.pixels())
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        diff > 1000,
        "frames at 0 s and 2 s should differ ({diff} px)"
    );
    assert!(img.pixels().all(|p| p[3] == 255), "opaque");
    // Past the end: the last decodable frame is returned rather than an error.
    assert!(extract_frame(&path, 60.0, 640, 360).is_ok());
}

#[test]
fn thumbnail_errors_are_errors() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(poster(&tmp.path().join("missing.mp4")).is_err());
    let not_media = tmp.path().join("text.mp4");
    std::fs::write(&not_media, b"this is not a video").unwrap();
    assert!(poster(&not_media).is_err());
}
