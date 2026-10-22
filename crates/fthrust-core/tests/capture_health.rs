use fthrust_core::capture_health::{
    CaptureHealthMonitor, ContentSuspicionTracker, HealthState, evaluate_save_admission, flags,
};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/capture-health.json")).unwrap()
}

#[test]
fn flags_and_thresholds_match_fixture() {
    let f = fixture();
    assert_eq!(f["flags"]["ACTIVE"], flags::ACTIVE);
    assert_eq!(f["flags"]["RECOVERING"], flags::RECOVERING);
    assert_eq!(f["flags"]["BACKEND_FAILED"], flags::BACKEND_FAILED);
    assert_eq!(f["flags"]["CONTENT_SUSPECT"], flags::CONTENT_SUSPECT);
    assert_eq!(f["flags"]["PAUSED"], flags::PAUSED);
    assert_eq!(
        f["flags"]["RECONFIGURE_REQUIRED"],
        flags::RECONFIGURE_REQUIRED
    );
    assert_eq!(
        f["thresholds"]["warning_after_s"],
        CaptureHealthMonitor::WARNING_AFTER_S
    );
    assert_eq!(
        f["thresholds"]["stalled_after_s"],
        CaptureHealthMonitor::STALLED_AFTER_S
    );
    assert_eq!(
        f["thresholds"]["recovery_budget_reset_s"],
        CaptureHealthMonitor::RECOVERY_BUDGET_RESET_S
    );
    let states: Vec<&str> = f["states"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        states,
        HealthState::ALL
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn observe_sequences_match_fixture() {
    let f = fixture();
    for (name, seq) in f["observe_sequences"].as_object().unwrap() {
        let mut monitor = CaptureHealthMonitor::new();
        for (i, step) in seq.as_array().unwrap().iter().enumerate() {
            let input = step["input"].as_array().unwrap();
            let snap = monitor.observe(
                input[0].as_bool().unwrap(),
                input[1].as_u64().unwrap(),
                input[2].as_u64().unwrap() as u32,
                input[3].as_u64().unwrap(),
                input[4].as_f64().unwrap(),
            );
            let e = &step["expected"];
            let ctx = format!("{name} step {i}");
            assert_eq!(snap.state.as_str(), e["state"].as_str().unwrap(), "{ctx}");
            assert_eq!(
                snap.previous_state.as_str(),
                e["previous_state"].as_str().unwrap(),
                "{ctx}"
            );
            assert_eq!(snap.reason, e["reason"].as_str().unwrap(), "{ctx}");
            assert_eq!(snap.changed, e["changed"].as_bool().unwrap(), "{ctx}");
            assert_eq!(
                snap.request_recovery,
                e["request_recovery"].as_bool().unwrap(),
                "{ctx}"
            );
            assert!(
                (snap.seconds_since_progress - e["seconds_since_progress"].as_f64().unwrap()).abs()
                    < 1e-9,
                "{ctx}"
            );
            assert!(
                (snap.fresh_buffer_seconds - e["fresh_buffer_seconds"].as_f64().unwrap()).abs()
                    < 1e-9,
                "{ctx}"
            );
            assert_eq!(
                snap.save_allowed(),
                e["save_allowed"].as_bool().unwrap(),
                "{ctx}"
            );
        }
    }
}

#[test]
fn stall_requests_recovery_once_per_incident() {
    let mut m = CaptureHealthMonitor::new();
    m.observe(true, 10, flags::ACTIVE, 1, 0.0);
    let s = m.observe(true, 10, flags::ACTIVE, 1, 8.5);
    assert_eq!(s.state, HealthState::Stalled);
    assert!(s.request_recovery);
    assert!(!m.observe(true, 10, flags::ACTIVE, 1, 9.0).request_recovery);
    // Recover briefly, then stall again: the budget is spent.
    m.observe(true, 20, flags::ACTIVE, 1, 10.0);
    let again = m.observe(true, 20, flags::ACTIVE, 1, 19.0);
    assert_eq!(again.state, HealthState::Stalled);
    assert!(!again.request_recovery);
}

#[test]
fn budget_restored_after_30s_healthy() {
    let mut m = CaptureHealthMonitor::new();
    m.observe(true, 10, flags::ACTIVE, 1, 0.0);
    assert!(m.observe(true, 10, flags::ACTIVE, 1, 8.5).request_recovery);
    let mut t = 9.0;
    let mut count = 20;
    while t < 45.0 {
        let s = m.observe(true, count, flags::ACTIVE, 1, t);
        assert_eq!(s.state, HealthState::Healthy, "t={t}");
        t += 0.5;
        count += 1;
    }
    // `count` was incremented after the last observe; repeat the last value.
    let s = m.observe(true, count - 1, flags::ACTIVE, 1, t + 8.5);
    assert_eq!(s.state, HealthState::Stalled);
    assert!(
        s.request_recovery,
        "30 healthy seconds restore the recovery budget"
    );
}

#[test]
fn new_generation_rebaselines() {
    let mut m = CaptureHealthMonitor::new();
    m.observe(true, 100, flags::ACTIVE, 1, 0.0);
    m.observe(true, 110, flags::ACTIVE, 1, 0.5);
    let s = m.observe(true, 900, flags::ACTIVE, 2, 1.0);
    assert_eq!(s.state, HealthState::Initializing);
    assert_eq!(s.reason, "waiting for first frame");
    assert_eq!(s.fresh_buffer_seconds, 0.0);
    let s = m.observe(true, 900, flags::ACTIVE, 2, 1.5);
    assert_eq!(
        s.state,
        HealthState::Initializing,
        "same count after rebaseline is not progress"
    );
    let s = m.observe(true, 910, flags::ACTIVE, 2, 2.0);
    assert_eq!(s.state, HealthState::Healthy);
}

#[test]
fn backend_failed_is_failed_and_blocks_save() {
    let mut m = CaptureHealthMonitor::new();
    m.observe(true, 10, flags::ACTIVE, 1, 0.0);
    let s = m.observe(true, 20, flags::ACTIVE | flags::BACKEND_FAILED, 1, 0.5);
    assert_eq!(s.state, HealthState::Failed);
    assert_eq!(s.reason, "capture backend failed");
    assert!(!s.save_allowed());
    let admission = evaluate_save_admission(Some(&s), 30);
    assert!(!admission.allowed);
    assert_eq!(admission.duration_seconds, 0);
    assert_eq!(
        admission.reason,
        "Capture is not receiving new frames. Restart capture before saving."
    );
    let recovering = m.observe(true, 20, flags::RECOVERING, 1, 1.0);
    assert_eq!(
        evaluate_save_admission(Some(&recovering), 30).reason,
        "Capture is recovering. Try again in a moment."
    );
    assert_eq!(
        evaluate_save_admission(None, 30).reason,
        "Capture health is not available yet."
    );
    let healthy = m.observe(true, 30, flags::ACTIVE, 1, 1.5);
    let ok = evaluate_save_admission(Some(&healthy), 0);
    assert!(ok.allowed);
    assert_eq!(ok.duration_seconds, 1, "requested seconds are floored at 1");
}

#[test]
fn paused_is_degraded_with_paused_flag() {
    let mut m = CaptureHealthMonitor::new();
    m.observe(true, 10, flags::ACTIVE, 1, 0.0);
    let s = m.observe(true, 10, flags::ACTIVE | flags::PAUSED, 1, 5.0);
    assert_eq!(s.state, HealthState::Degraded);
    assert_eq!(s.reason, "capture is intentionally paused");
    assert_eq!(s.seconds_since_progress, 0.0);
    assert!(s.save_allowed());
}

#[test]
fn content_suspect_needs_fresh_frames() {
    let mut m = CaptureHealthMonitor::new();
    let s = m.observe(true, 0, flags::ACTIVE | flags::CONTENT_SUSPECT, 1, 0.0);
    assert_eq!(
        s.state,
        HealthState::Initializing,
        "no frames yet: suspicion cannot apply"
    );
    let s = m.observe(true, 10, flags::ACTIVE | flags::CONTENT_SUSPECT, 1, 0.5);
    assert_eq!(s.state, HealthState::ContentSuspect);
    assert_eq!(
        s.reason,
        "fresh frames are persistently black-like or uniform"
    );
    assert!(s.save_allowed());
    let s = m.observe(true, 10, flags::ACTIVE | flags::CONTENT_SUSPECT, 1, 1.0);
    assert_eq!(s.state, HealthState::ContentSuspect);
    assert_eq!(
        s.reason,
        "fresh content remains persistently black-like or uniform"
    );
}

#[test]
fn suspicion_after_12_black_samples() {
    let mut t = ContentSuspicionTracker::new();
    for _ in 0..11 {
        assert!(!t.observe_luma(2.0, 1.0));
    }
    assert!(
        t.observe_luma(2.0, 1.0),
        "12th consecutive black-like sample"
    );
    assert!(
        !t.observe_luma(120.0, 900.0),
        "normal content resets the streak"
    );
    for _ in 0..11 {
        assert!(
            !t.observe_luma(128.0, 1.5),
            "uniform (variance <= 2) counts too"
        );
    }
    assert!(t.observe_luma(128.0, 1.5));
    assert!(
        !t.observe_luma(5.0, 6.5),
        "mean <= 8 but variance > 6 is neither black-like nor uniform"
    );
}

#[test]
fn disconnect_then_reconnect_reinitialises() {
    let mut m = CaptureHealthMonitor::new();
    m.observe(true, 10, flags::ACTIVE, 1, 0.0);
    let s = m.observe(false, 0, 0, 1, 0.5);
    assert_eq!(s.state, HealthState::Stopped);
    let s = m.observe(true, 10, flags::ACTIVE, 1, 1.0);
    assert_eq!(
        s.state,
        HealthState::Healthy,
        "same generation keeps its baseline after a disconnect"
    );
    m.reset(true);
    assert_eq!(m.state(), HealthState::Initializing);
    let s = m.observe(true, 10, flags::ACTIVE, 1, 1.5);
    assert_eq!(s.previous_state, HealthState::Initializing);
    assert_eq!(
        s.state,
        HealthState::Healthy,
        "a non-zero count after reset is the first frame"
    );
    assert_eq!(
        s.fresh_buffer_seconds, 0.0,
        "reset discards the accumulated buffer age"
    );
}
