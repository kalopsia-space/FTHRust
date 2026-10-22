use std::collections::BTreeMap;

use fthrust_core::capture_health::HealthState;
use fthrust_core::recovery_tracker::{
    RecoverableIncident, RecoveryKind, RecoveryTracker, advance_recoverable_incidents,
};

fn healthy_poll(t: &mut RecoveryTracker, key: &str, count: u64, now: f64) -> bool {
    t.observe(key, true, 1, 1, count, HealthState::Healthy, now, false)
}

#[test]
fn needs_three_polls_over_three_seconds() {
    let mut t = RecoveryTracker::new();
    assert!(
        !healthy_poll(&mut t, "k", 1, 0.0),
        "first poll only opens the window"
    );
    assert!(
        !healthy_poll(&mut t, "k", 2, 1.0),
        "progress #1 starts the clock"
    );
    assert!(!healthy_poll(&mut t, "k", 3, 2.0), "progress #2");
    assert!(
        !healthy_poll(&mut t, "k", 4, 3.5),
        "progress #3 but only 2.5 s elapsed"
    );
    assert!(
        healthy_poll(&mut t, "k", 5, 4.0),
        "progress #4 and 3.0 s elapsed"
    );
    assert!(
        !healthy_poll(&mut t, "k", 6, 4.5),
        "window was consumed; a new one opens"
    );
}

#[test]
fn non_progressing_poll_resets_window() {
    let mut t = RecoveryTracker::new();
    healthy_poll(&mut t, "k", 1, 0.0);
    healthy_poll(&mut t, "k", 2, 1.0);
    healthy_poll(&mut t, "k", 3, 2.0);
    assert!(
        !healthy_poll(&mut t, "k", 3, 3.0),
        "same count resets the clock"
    );
    assert!(!healthy_poll(&mut t, "k", 4, 4.0));
    assert!(!healthy_poll(&mut t, "k", 5, 5.0));
    assert!(
        !healthy_poll(&mut t, "k", 6, 6.5),
        "only 2.5 s since the restart"
    );
    assert!(healthy_poll(&mut t, "k", 7, 7.0));
}

#[test]
fn non_healthy_poll_resets_window() {
    let mut t = RecoveryTracker::new();
    healthy_poll(&mut t, "k", 1, 0.0);
    healthy_poll(&mut t, "k", 2, 1.0);
    healthy_poll(&mut t, "k", 3, 2.0);
    assert!(!t.observe("k", true, 1, 1, 4, HealthState::Degraded, 3.0, false));
    assert!(
        !healthy_poll(&mut t, "k", 5, 4.0),
        "window must be rebuilt from scratch"
    );
    assert!(!healthy_poll(&mut t, "k", 6, 5.0));
    assert!(!healthy_poll(&mut t, "k", 7, 6.0));
    assert!(
        !healthy_poll(&mut t, "k", 8, 7.0),
        "polls ok but 3.0 s not yet spanned: 7.0-5.0"
    );
    assert!(healthy_poll(&mut t, "k", 9, 8.0));
    assert!(
        !t.observe("k", false, 1, 1, 10, HealthState::Healthy, 9.0, false),
        "disconnect resets"
    );
}

#[test]
fn generation_change_resets() {
    let mut t = RecoveryTracker::new();
    healthy_poll(&mut t, "k", 1, 0.0);
    healthy_poll(&mut t, "k", 2, 1.0);
    healthy_poll(&mut t, "k", 3, 2.0);
    assert!(
        !t.observe("k", true, 1, 2, 4, HealthState::Healthy, 3.0, false),
        "backend gen changed"
    );
    assert!(!t.observe("k", true, 1, 2, 5, HealthState::Healthy, 4.0, false));
    assert!(
        !t.observe("k", true, 2, 2, 6, HealthState::Healthy, 5.0, false),
        "process gen changed"
    );
}

#[test]
fn frame_count_decrease_resets() {
    let mut t = RecoveryTracker::new();
    healthy_poll(&mut t, "k", 10, 0.0);
    healthy_poll(&mut t, "k", 20, 1.0);
    healthy_poll(&mut t, "k", 30, 2.0);
    assert!(
        !healthy_poll(&mut t, "k", 5, 3.0),
        "counter went backwards: new window"
    );
    assert!(!healthy_poll(&mut t, "k", 6, 4.0));
    assert!(!healthy_poll(&mut t, "k", 7, 5.0));
    assert!(healthy_poll(&mut t, "k", 8, 7.0));
}

#[test]
fn content_suspect_counts_only_for_backend_kind() {
    assert!(RecoveryKind::CaptureBackend.allows_content_suspect());
    assert!(!RecoveryKind::EncoderInit.allows_content_suspect());
    assert!(!RecoveryKind::EngineLineage.allows_content_suspect());
    let mut t = RecoveryTracker::new();
    let suspect = HealthState::ContentSuspect;
    assert!(!t.observe("b", true, 1, 1, 1, suspect, 0.0, true));
    assert!(!t.observe("b", true, 1, 1, 2, suspect, 1.0, true));
    assert!(!t.observe("b", true, 1, 1, 3, suspect, 2.0, true));
    assert!(!t.observe("b", true, 1, 1, 4, suspect, 3.0, true));
    assert!(t.observe("b", true, 1, 1, 5, suspect, 4.0, true));
    assert!(!t.observe("e", true, 1, 1, 1, suspect, 0.0, false));
    assert!(!t.observe("e", true, 1, 1, 2, suspect, 1.0, false));
    assert!(!t.observe("e", true, 1, 1, 3, suspect, 2.0, false));
    assert!(!t.observe("e", true, 1, 1, 4, suspect, 3.0, false));
    assert!(
        !t.observe("e", true, 1, 1, 5, suspect, 4.0, false),
        "suspicion never clears non-backend incidents"
    );
}

fn incident(kind: RecoveryKind, process_generation: u64) -> RecoverableIncident {
    RecoverableIncident {
        kind,
        process_generation,
        public_error_code: "Error 001".into(),
        title: "CAPTURE FAILED".into(),
    }
}

fn advance(
    tracker: &mut RecoveryTracker,
    incidents: &mut BTreeMap<String, RecoverableIncident>,
    active_gen: Option<u64>,
    count: u64,
    state: HealthState,
    hardware_active: bool,
    now: f64,
) -> Vec<String> {
    advance_recoverable_incidents(
        tracker,
        incidents,
        active_gen,
        true,
        count,
        state,
        1,
        hardware_active,
        now,
    )
    .into_iter()
    .map(|(key, _)| key)
    .collect()
}

#[test]
fn engine_lineage_needs_newer_process_generation() {
    let mut t = RecoveryTracker::new();
    let mut incidents = BTreeMap::new();
    incidents.insert(
        "engine-process-exit".to_string(),
        incident(RecoveryKind::EngineLineage, 3),
    );
    for (i, now) in [0.0, 1.0, 2.0, 3.0, 4.0, 5.0].into_iter().enumerate() {
        let recovered = advance(
            &mut t,
            &mut incidents,
            Some(3),
            i as u64 + 1,
            HealthState::Healthy,
            false,
            now,
        );
        assert!(
            recovered.is_empty(),
            "same process generation can never clear a lineage incident"
        );
    }
    assert_eq!(incidents.len(), 1);
    let mut recovered = Vec::new();
    for (i, now) in [10.0, 11.0, 12.0, 13.0, 14.0].into_iter().enumerate() {
        recovered = advance(
            &mut t,
            &mut incidents,
            Some(4),
            i as u64 + 1,
            HealthState::Healthy,
            false,
            now,
        );
    }
    assert_eq!(recovered, vec!["engine-process-exit".to_string()]);
    assert!(incidents.is_empty(), "recovered incidents are removed");
}

#[test]
fn encoder_init_needs_hardware_active_and_newer_generation() {
    let mut t = RecoveryTracker::new();
    let mut incidents = BTreeMap::new();
    incidents.insert(
        "encoder-init".to_string(),
        incident(RecoveryKind::EncoderInit, 1),
    );
    for (i, now) in [0.0, 1.0, 2.0, 3.0, 4.0].into_iter().enumerate() {
        assert!(
            advance(
                &mut t,
                &mut incidents,
                Some(2),
                i as u64 + 1,
                HealthState::Healthy,
                false,
                now
            )
            .is_empty(),
            "software fallback is not recovery"
        );
    }
    let mut recovered = Vec::new();
    for (i, now) in [10.0, 11.0, 12.0, 13.0, 14.0].into_iter().enumerate() {
        recovered = advance(
            &mut t,
            &mut incidents,
            Some(2),
            i as u64 + 1,
            HealthState::Healthy,
            true,
            now,
        );
    }
    assert_eq!(recovered, vec!["encoder-init".to_string()]);
}

#[test]
fn no_active_process_generation_only_resets_encoder_windows() {
    let mut t = RecoveryTracker::new();
    let mut incidents = BTreeMap::new();
    incidents.insert(
        "backend".to_string(),
        incident(RecoveryKind::CaptureBackend, 1),
    );
    incidents.insert(
        "encoder-init".to_string(),
        incident(RecoveryKind::EncoderInit, 1),
    );
    let recovered = advance(
        &mut t,
        &mut incidents,
        None,
        5,
        HealthState::Healthy,
        true,
        0.0,
    );
    assert!(recovered.is_empty());
    assert_eq!(
        incidents.len(),
        2,
        "nothing is cleared while no engine process is known"
    );
}
