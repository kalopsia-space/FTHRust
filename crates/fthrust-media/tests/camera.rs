//! The camera ring (notes-overlays-reference sections 3, 5, 8), driven by a
//! synthetic `lavfi` camera.

use std::time::{Duration, Instant};

use fthrust_media::camera::{CameraRecorder, Source, list_devices};

fn wait_until(mut f: impl FnMut() -> bool, secs: f64) -> bool {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// `(frames, first pts s, last pts s)` of the segment's video stream, every
/// frame decoded.
fn segment(path: &std::path::Path) -> (usize, f64, f64) {
    use ffmpeg_next as ff;
    ff::init().unwrap();
    let mut ictx = ff::format::input(&path).unwrap();
    let s = ictx.streams().best(ff::media::Type::Video).unwrap();
    let (index, tb) = (s.index(), s.time_base());
    assert_eq!(s.parameters().id(), ff::codec::Id::MJPEG);
    let mut dec = ff::codec::context::Context::from_parameters(s.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    let (mut n, mut first, mut last) = (0, f64::MAX, f64::MIN);
    let mut frame = ff::frame::Video::empty();
    for (st, p) in ictx.packets() {
        if st.index() != index {
            continue;
        }
        let t = p.pts().unwrap() as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator());
        first = first.min(t);
        last = last.max(t);
        dec.send_packet(&p).unwrap();
        while dec.receive_frame(&mut frame).is_ok() {
            n += 1;
        }
    }
    (n, first, last)
}

#[test]
fn segment_has_real_timestamps() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = CameraRecorder::start(Source::synthetic(320, 240, 15)).expect("synthetic camera");
    assert!(
        wait_until(|| rec.frames() >= 20, 5.0),
        "frames: {}",
        rec.frames()
    );
    let end = Instant::now();
    let out = tmp.path().join("cam.mkv");
    let written = rec.write_segment(end, 1.0, &out).unwrap();
    rec.stop();
    let (n, first, last) = segment(&out);
    assert_eq!(n, written);
    // About one second at 15 fps, placed on the clip's timeline.
    assert!((12..=17).contains(&n), "{n} frames");
    assert!((0.0..0.2).contains(&first), "first {first}");
    assert!(last > 0.75 && last <= 1.0, "last {last}");
}

#[test]
fn ring_keeps_its_window() {
    let rec =
        CameraRecorder::start_keeping(Source::synthetic(160, 120, 30), Duration::from_millis(300))
            .expect("synthetic camera");
    assert!(wait_until(|| rec.frames() >= 25, 5.0));
    let span = rec.span().unwrap();
    rec.stop();
    assert!(span <= Duration::from_millis(340), "{span:?}");
}

#[test]
fn latest_jpeg_advances() {
    let rec = CameraRecorder::start(Source::synthetic(160, 120, 30)).unwrap();
    assert!(wait_until(|| rec.latest_jpeg().is_some(), 5.0));
    let (seq, jpeg) = rec.latest_jpeg().unwrap();
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "a JPEG");
    assert!(wait_until(
        || rec.latest_jpeg().is_some_and(|(s, _)| s > seq),
        5.0
    ));
    rec.stop();
}

#[test]
fn bad_device_fails_cleanly() {
    let err =
        CameraRecorder::start(Source::v4l2("/dev/fthrust-no-such-camera".as_ref())).unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn stop_joins_and_nothing_to_write_after() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = CameraRecorder::start(Source::synthetic(160, 120, 30)).unwrap();
    assert!(wait_until(|| rec.frames() > 0, 5.0));
    let started = Instant::now();
    rec.stop();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!rec.is_running());
    // The ring is cleared on stop: nothing to write.
    assert!(
        rec.write_segment(Instant::now(), 1.0, &tmp.path().join("x.mkv"))
            .is_err()
    );
}

#[test]
fn list_devices_does_not_panic() {
    for d in list_devices() {
        assert!(!d.name.is_empty());
        assert!(d.path.to_string_lossy().starts_with("/dev/"));
    }
}
