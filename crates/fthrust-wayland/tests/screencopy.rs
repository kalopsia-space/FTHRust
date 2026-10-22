use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fthrust_wayland::backend::{BackendError, CaptureFailure, FailureKind};
use fthrust_wayland::connection::{ConnectError, WaylandConnection};
use fthrust_wayland::frame::{BgraFrame, normalize};
use fthrust_wayland::screencopy::WlrScreencopy;

fn compositor_has(interface: &str) -> Option<bool> {
    match WaylandConnection::connect(Instant::now() + Duration::from_secs(5), &|| true) {
        Ok(c) => Some(c.core().global(interface).is_some()),
        Err(ConnectError::NoDisplay) => None,
        Err(e) => panic!("connect: {e}"),
    }
}

#[test]
fn captures_frames_on_a_wlr_compositor() {
    match compositor_has("zwlr_screencopy_manager_v1") {
        None => return eprintln!("skipping: no WAYLAND_DISPLAY"),
        Some(false) => return eprintln!("skipping: no zwlr_screencopy_manager_v1"),
        Some(true) => {}
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let mut backend = WlrScreencopy::open("", false, cancel.clone()).unwrap();
    let (nw, nh) = backend.native_size();
    assert!(nw > 0 && nh > 0);
    assert!(!backend.selected_output().is_empty());
    let mut last_ts = 0i64;
    let mut out = BgraFrame::default();
    for i in 0..5 {
        let view = backend.capture().unwrap();
        assert_eq!((view.width, view.height), (nw, nh), "frame {i}");
        assert!(
            view.timestamp_ns > last_ts,
            "strictly increasing timestamps"
        );
        last_ts = view.timestamp_ns;
        normalize(&view, &mut out).unwrap();
        assert_eq!((out.width, out.height), (nw, nh));
    }
    assert!(
        out.data.iter().any(|b| *b != 0),
        "a real desktop is not all zero"
    );
    assert!(
        backend.cursor_requested(),
        "the cursor is composited into the frame (fix)"
    );
}

#[test]
fn strict_missing_output_is_output_absent() {
    match compositor_has("zwlr_screencopy_manager_v1") {
        None => return eprintln!("skipping: no WAYLAND_DISPLAY"),
        Some(false) => return eprintln!("skipping: no zwlr_screencopy_manager_v1"),
        Some(true) => {}
    }
    let cancel = Arc::new(AtomicBool::new(false));
    match WlrScreencopy::open("no-such-output-xyz", true, cancel.clone()) {
        Err(BackendError {
            kind: FailureKind::OutputAbsent,
            reason,
        }) => assert!(reason.contains("no-such-output-xyz"), "{reason}"),
        other => panic!("expected OutputAbsent, got {:?}", other.map(|_| ())),
    }
    // Non-strict falls back to the first output.
    let backend = WlrScreencopy::open("no-such-output-xyz", false, cancel).unwrap();
    assert!(!backend.selected_output().is_empty());
}

#[test]
fn cancel_during_open_is_cancelled() {
    if compositor_has("zwlr_screencopy_manager_v1") != Some(true) {
        return eprintln!("skipping: no wlr compositor");
    }
    let cancel = Arc::new(AtomicBool::new(true));
    match WlrScreencopy::open("", false, cancel.clone()) {
        Err(BackendError {
            kind: FailureKind::Cancelled,
            ..
        }) => {}
        other => panic!("expected Cancelled, got {:?}", other.map(|_| ())),
    }
    cancel.store(false, Ordering::SeqCst);
    let mut backend = WlrScreencopy::open("", false, cancel.clone()).unwrap();
    cancel.store(true, Ordering::SeqCst);
    assert!(matches!(backend.capture(), Err(CaptureFailure::Cancelled)));
}
