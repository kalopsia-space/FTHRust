use fthrust_core::save_state::{
    EngineEvent, Outcome, OutcomeKind, RejectReason, SaveState, SaveStateMachine,
};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/save-state.json")).unwrap()
}

fn kind_name(k: OutcomeKind) -> &'static str {
    match k {
        OutcomeKind::Accepted => "ACCEPTED",
        OutcomeKind::Completed => "COMPLETED",
        OutcomeKind::Failed => "FAILED",
        OutcomeKind::TimeoutNotice => "TIMEOUT_NOTICE",
    }
}

/// Drives the machine through one fixture scenario, asserting the state,
/// the cumulative outcome kinds and the terminal-result count after each step.
fn run(name: &str) {
    let steps = fixture()[name].clone();
    let mut sm = SaveStateMachine::with_defaults();
    let mut kinds: Vec<&'static str> = Vec::new();
    let mut results = 0usize;
    let record = |o: Option<Outcome>, kinds: &mut Vec<&'static str>, results: &mut usize| {
        if let Some(o) = o {
            kinds.push(kind_name(o.kind));
            if matches!(o.kind, OutcomeKind::Completed | OutcomeKind::Failed) {
                *results += 1;
            }
        }
    };
    for step in steps.as_array().unwrap() {
        let t = step["t"].as_f64().unwrap();
        let label = step["step"].as_str().unwrap();
        let outcome = if label.starts_with("submit B") {
            // The poller drains A's pending CLIP_SAVED before writing B.
            let o = sm.on_event(EngineEvent::ClipSaved, "", t);
            record(o, &mut kinds, &mut results);
            assert!(sm.submit("/clips/b.mp4", 30, t).is_ok());
            None
        } else if label.starts_with("submit") {
            assert!(sm.submit("/clips/a.mp4", 30, t).is_ok());
            None
        } else if label.contains("written, unread") {
            None
        } else if label.contains("CLIP_SAVED") {
            sm.on_event(EngineEvent::ClipSaved, "", t)
        } else if label.contains("SAVE_STARTED") {
            sm.on_event(EngineEvent::SaveStarted, "", t)
        } else if label.contains("ERROR_OCCURRED") {
            sm.on_event(EngineEvent::ErrorOccurred, "disk full", t)
        } else if label.starts_with("pump")
            || label == "still encoding"
            || label == "ack timeout"
            || label == "completion deadline"
        {
            sm.on_tick(t)
        } else {
            panic!("unknown fixture step {label:?}");
        };
        record(outcome, &mut kinds, &mut results);
        assert_eq!(
            sm.state().as_str(),
            step["state"].as_str().unwrap(),
            "{name}: {label}"
        );
        let expected: Vec<&str> = step["outcome_kinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(kinds, expected, "{name}: {label}");
        assert_eq!(
            results,
            step["results"].as_u64().unwrap() as usize,
            "{name}: {label}"
        );
    }
}

#[test]
fn t1_normal_save_started_then_clip_saved() {
    run("T1");
}

#[test]
fn t2_clip_saved_without_save_started_succeeds() {
    run("T2");
}

#[test]
fn t3_immediate_failure_reports_once() {
    run("T3");
}

#[test]
fn t4_accepted_then_failure() {
    run("T4");
}

#[test]
fn t5_no_response_times_out_then_fails() {
    run("T5");
}

#[test]
fn t5_late_success_after_timeout() {
    run("T5_late_success");
}

#[test]
fn t6_old_clip_saved_is_processed_before_the_next_save() {
    run("T6");
}

#[test]
fn busy_rejects_second_submit() {
    let mut sm = SaveStateMachine::with_defaults();
    assert!(sm.submit("/clips/a.mp4", 30, 1000.0).is_ok());
    assert!(sm.is_busy());
    assert_eq!(
        sm.submit("/clips/b.mp4", 30, 1000.1).unwrap_err(),
        RejectReason::Busy
    );
    assert_eq!(sm.current().map(|op| op.op_id), Some(1));
}

#[test]
fn late_success_after_timeout_notice_is_completed() {
    let mut sm = SaveStateMachine::with_defaults();
    assert!(sm.submit("/clips/a.mp4", 30, 1000.0).is_ok());
    let notice = sm.on_tick(1001.5).unwrap();
    assert_eq!(notice.kind, OutcomeKind::TimeoutNotice);
    assert_eq!(sm.state(), SaveState::TimedOut);
    assert!(!sm.is_busy(), "a timed-out save must not block the hotkey");

    let late = sm.on_event(EngineEvent::ClipSaved, "", 1001.9).unwrap();
    assert_eq!(late.kind, OutcomeKind::Completed);
    assert!(late.late);
    assert_eq!(sm.state(), SaveState::EngineCompleted);
    assert!(sm.on_event(EngineEvent::ClipSaved, "", 1002.0).is_none());
    assert!(sm.on_tick(2000.0).is_none());
}

#[test]
fn error_detail_is_carried_and_duplicate_ack_is_silent() {
    let mut sm = SaveStateMachine::with_defaults();
    sm.submit("/clips/a.mp4", 30, 0.0).unwrap();
    assert_eq!(
        sm.on_event(EngineEvent::SaveStarted, "", 0.1).unwrap().kind,
        OutcomeKind::Accepted
    );
    assert!(sm.on_event(EngineEvent::SaveStarted, "", 0.2).is_none());
    let failed = sm
        .on_event(EngineEvent::ErrorOccurred, "disk full", 0.3)
        .unwrap();
    assert_eq!(failed.kind, OutcomeKind::Failed);
    assert_eq!(failed.detail, "disk full");
    assert_eq!(sm.current().unwrap().error_detail, "disk full");
}

#[test]
fn cancel_active_releases_single_flight_without_result() {
    let mut sm = SaveStateMachine::with_defaults();
    sm.submit("/clips/a.mp4", 30, 0.0).unwrap();
    sm.on_event(EngineEvent::SaveStarted, "", 0.1);
    assert!(sm.cancel_active(0.2).is_none());
    assert!(!sm.is_busy());
    assert!(sm.on_event(EngineEvent::ClipSaved, "", 0.3).is_none());
}
