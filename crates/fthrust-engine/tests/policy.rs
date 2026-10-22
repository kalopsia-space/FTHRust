use std::cell::RefCell;
use std::time::Duration;

use fthrust_engine::policy::{
    CaptureRecoveryPolicy, EncodeFailurePolicy, backend_candidates, next_encode_deadline,
    recovery_order, should_repeat_last_frame, wait_for_recovery_backoff,
};
use fthrust_ipc::BackendKind;
use fthrust_wayland::dispatch::WaitResult;

const MS: u64 = 1;

#[test]
fn candidates_and_recovery_order() {
    let cands = backend_candidates(true);
    assert_eq!(
        cands,
        [
            BackendKind::WlrScreencopy,
            BackendKind::ExtImageCopy,
            BackendKind::PortalScreenCast
        ]
    );
    assert!(backend_candidates(false).is_empty(), "no X11 fallback");
    assert_eq!(
        recovery_order(BackendKind::WlrScreencopy, &cands),
        [BackendKind::WlrScreencopy, BackendKind::ExtImageCopy]
    );
    assert_eq!(
        recovery_order(BackendKind::ExtImageCopy, &cands),
        [BackendKind::ExtImageCopy, BackendKind::WlrScreencopy]
    );
    assert_eq!(
        recovery_order(BackendKind::PortalScreenCast, &cands),
        [
            BackendKind::PortalScreenCast,
            BackendKind::WlrScreencopy,
            BackendKind::ExtImageCopy
        ]
    );
    assert_eq!(
        recovery_order(BackendKind::Synthetic, &[BackendKind::Synthetic]),
        [BackendKind::Synthetic]
    );
}

#[test]
fn recovery_backoffs_exhaustion_and_reset() {
    let fps = 60u64;
    let mut p = CaptureRecoveryPolicy::new(fps * 10, 3);
    let d = p.on_generation_failed(0, false);
    assert_eq!(
        (d.retry, d.exhausted, d.attempt, d.backoff),
        (true, false, 1, Duration::from_millis(250 * MS))
    );
    let d = p.on_generation_failed(0, false);
    assert_eq!(
        (d.retry, d.attempt, d.backoff),
        (true, 2, Duration::from_millis(750))
    );
    let d = p.on_generation_failed(0, false);
    assert_eq!(
        (d.retry, d.attempt, d.backoff),
        (true, 3, Duration::from_millis(1500))
    );
    let d = p.on_generation_failed(0, false);
    assert_eq!((d.retry, d.exhausted, d.attempt), (false, true, 4));
    let mut p = CaptureRecoveryPolicy::new(fps * 10, 3);
    p.on_generation_failed(0, false);
    p.on_generation_failed(0, false);
    let d = p.on_generation_failed(fps * 10, false);
    assert_eq!(
        (d.attempt, d.backoff),
        (1, Duration::from_millis(250)),
        "a healthy generation resets the budget"
    );
    let d = p.on_generation_failed(fps * 10 - 1, false);
    assert_eq!(d.attempt, 2, "one frame short does not reset");
}

#[test]
fn transient_failures_wait_without_budget() {
    let mut p = CaptureRecoveryPolicy::new(600, 3);
    for _ in 0..5 {
        let d = p.on_generation_failed(0, true);
        assert_eq!(
            (d.retry, d.exhausted, d.backoff),
            (true, false, Duration::from_millis(2000))
        );
        assert_eq!(p.attempts(), 0);
    }
    let d = p.on_generation_failed(0, false);
    assert_eq!(d.attempt, 1);
}

#[test]
fn encode_failure_threshold() {
    let mut p = EncodeFailurePolicy::new(60);
    for i in 1..30 {
        assert!(!p.on_encode_result(false), "failure {i}");
    }
    assert!(
        p.on_encode_result(false),
        "30th consecutive failure at 60 fps"
    );
    let mut p = EncodeFailurePolicy::new(4);
    for _ in 0..4 {
        assert!(!p.on_encode_result(false));
    }
    assert!(p.on_encode_result(false), "never fewer than five");
    let mut p = EncodeFailurePolicy::new(60);
    for _ in 0..29 {
        p.on_encode_result(false);
    }
    assert!(!p.on_encode_result(true));
    assert_eq!(p.consecutive_failures(), 0, "success resets");
}

#[test]
fn encode_deadline_reanchors_after_two_periods() {
    let frame = 16_666_667i64;
    let next = 1_000_000_000i64;
    assert_eq!(
        next_encode_deadline(next, frame, next + frame),
        next,
        "slightly late keeps the cadence"
    );
    assert_eq!(
        next_encode_deadline(next, frame, next + 2 * frame),
        next,
        "exactly two periods keeps it"
    );
    let late = next + 2 * frame + 1;
    assert_eq!(
        next_encode_deadline(next, frame, late),
        late,
        "later than two periods re-anchors"
    );
}

#[test]
fn backoff_sleeps_in_slices_and_honours_cancellation() {
    let sleeps = RefCell::new(Vec::new());
    let ok = wait_for_recovery_backoff(
        Duration::from_millis(120),
        || true,
        |d| sleeps.borrow_mut().push(d.as_millis()),
    );
    assert!(ok);
    assert_eq!(*sleeps.borrow(), [50, 50, 20]);
    let sleeps = RefCell::new(Vec::new());
    let calls = RefCell::new(0);
    let ok = wait_for_recovery_backoff(
        Duration::from_millis(120),
        || {
            *calls.borrow_mut() += 1;
            *calls.borrow() <= 1
        },
        |d| sleeps.borrow_mut().push(d.as_millis()),
    );
    assert!(!ok, "cancelled");
    assert_eq!(sleeps.borrow().len(), 1);
}

#[test]
fn repeat_last_frame_only_on_timeout_with_a_frame() {
    assert!(should_repeat_last_frame(WaitResult::Timeout, true));
    assert!(!should_repeat_last_frame(WaitResult::Timeout, false));
    for r in [
        WaitResult::EventReceived,
        WaitResult::Cancelled,
        WaitResult::Disconnected,
        WaitResult::Error,
    ] {
        assert!(!should_repeat_last_frame(r, true));
    }
}
