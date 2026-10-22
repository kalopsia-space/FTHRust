use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fthrust_wayland::backend::{BackendError, CaptureFailure, FailureKind};
use fthrust_wayland::connection::{ConnectError, WaylandConnection};
use fthrust_wayland::ext_capture::ExtImageCopy;
use fthrust_wayland::frame::{BgraFrame, normalize};

fn ext_available() -> Option<bool> {
    match WaylandConnection::connect(Instant::now() + Duration::from_secs(5), &|| true) {
        Ok(c) => Some(
            c.core()
                .global("ext_image_copy_capture_manager_v1")
                .is_some()
                && c.core()
                    .global("ext_output_image_capture_source_manager_v1")
                    .is_some(),
        ),
        Err(ConnectError::NoDisplay) => None,
        Err(e) => panic!("connect: {e}"),
    }
}

#[test]
fn captures_frames_on_ext_compositor() {
    match ext_available() {
        None => return eprintln!("skipping: no WAYLAND_DISPLAY"),
        Some(false) => return eprintln!("skipping: no ext-image-copy-capture"),
        Some(true) => {}
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let fps = 30;
    let mut backend = ExtImageCopy::open("", false, fps, cancel.clone()).unwrap();
    let (nw, nh) = backend.native_size();
    assert!(nw > 0 && nh > 0);
    assert!(!backend.selected_output().is_empty());
    assert!(backend.cursor_requested());
    let mut out = BgraFrame::default();
    let mut last_ts = 0i64;
    for i in 0..5 {
        let view = backend.capture().unwrap();
        assert_eq!((view.width, view.height), (nw, nh), "frame {i}");
        assert!(view.timestamp_ns > last_ts);
        last_ts = view.timestamp_ns;
        normalize(&view, &mut out).unwrap();
    }
    assert!(out.data.iter().any(|b| *b != 0));
    assert!(backend.last_frame_available());
    // Static or not, pacing on the last delivery keeps at least ~fps frames
    // per second flowing.
    let started = Instant::now();
    let mut frames = 0;
    while started.elapsed() < Duration::from_secs(1) {
        backend.capture().unwrap();
        frames += 1;
    }
    assert!(
        frames >= fps * 8 / 10,
        "{frames} frames in 1 s at {fps} fps"
    );
}

#[test]
fn strict_missing_output_is_output_absent() {
    if ext_available() != Some(true) {
        return eprintln!("skipping: no ext-image-copy-capture");
    }
    let cancel = Arc::new(AtomicBool::new(false));
    match ExtImageCopy::open("no-such-output-xyz", true, 30, cancel.clone()) {
        Err(BackendError {
            kind: FailureKind::OutputAbsent,
            reason,
        }) => assert!(reason.contains("no-such-output-xyz"), "{reason}"),
        other => panic!("expected OutputAbsent, got {:?}", other.map(|_| ())),
    }
    let cancel2 = Arc::new(AtomicBool::new(true));
    assert!(matches!(
        ExtImageCopy::open("", false, 30, cancel2),
        Err(BackendError {
            kind: FailureKind::Cancelled,
            ..
        })
    ));
    let mut backend = ExtImageCopy::open("", false, 30, cancel.clone()).unwrap();
    cancel.store(true, Ordering::SeqCst);
    assert!(matches!(backend.capture(), Err(CaptureFailure::Cancelled)));
}
