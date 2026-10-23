//! Plan 5 Task 4: the foreign-toplevel model.

use fthrust_wayland::toplevels::{Toplevel, ToplevelEvent, ToplevelTracker};

fn states(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

#[test]
fn toplevel_tracker_publishes_only_after_done() {
    let mut t = ToplevelTracker::new();
    t.apply(ToplevelEvent::Title {
        id: 7,
        title: "Overwatch 2".into(),
    });
    t.apply(ToplevelEvent::State {
        id: 7,
        state: states(&[3]),
    });
    assert!(t.snapshot().is_empty(), "nothing published before done");
    t.apply(ToplevelEvent::Done { id: 7 });
    assert_eq!(
        t.snapshot(),
        vec![Toplevel {
            id: 7,
            title: "Overwatch 2".into(),
            app_id: String::new(),
            fullscreen: true,
            activated: false,
            minimized: false
        }]
    );
    // A later state change is only visible after the next done.
    t.apply(ToplevelEvent::State {
        id: 7,
        state: states(&[]),
    });
    assert!(t.snapshot()[0].fullscreen);
    t.apply(ToplevelEvent::Done { id: 7 });
    assert!(!t.snapshot()[0].fullscreen);
}

#[test]
fn toplevel_fullscreen_and_activated_states_parse() {
    let mut t = ToplevelTracker::new();
    t.apply(ToplevelEvent::AppId {
        id: 1,
        app_id: "steam_app_1".into(),
    });
    t.apply(ToplevelEvent::State {
        id: 1,
        state: states(&[2, 3]),
    });
    t.apply(ToplevelEvent::Done { id: 1 });
    t.apply(ToplevelEvent::Title {
        id: 2,
        title: "Terminal".into(),
    });
    t.apply(ToplevelEvent::State {
        id: 2,
        state: states(&[0, 1]),
    });
    t.apply(ToplevelEvent::Done { id: 2 });
    let snap = t.snapshot();
    assert_eq!(snap.len(), 2);
    assert!(snap[0].activated && snap[0].fullscreen && !snap[0].minimized);
    assert!(!snap[1].activated && !snap[1].fullscreen && snap[1].minimized);
    assert_eq!(t.active().map(|a| a.app_id.as_str()), Some("steam_app_1"));
    // Truncated arrays are ignored, not panicked on.
    t.apply(ToplevelEvent::State {
        id: 2,
        state: vec![1, 2, 3],
    });
    t.apply(ToplevelEvent::Done { id: 2 });
    assert!(t.snapshot()[1].minimized);
}

#[test]
fn toplevel_closed_removes() {
    let mut t = ToplevelTracker::new();
    t.apply(ToplevelEvent::Title {
        id: 3,
        title: "a".into(),
    });
    t.apply(ToplevelEvent::Done { id: 3 });
    t.apply(ToplevelEvent::Title {
        id: 4,
        title: "b".into(),
    });
    t.apply(ToplevelEvent::Done { id: 4 });
    t.apply(ToplevelEvent::Closed { id: 3 });
    assert_eq!(
        t.snapshot().iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![4]
    );
    t.apply(ToplevelEvent::Closed { id: 99 });
    assert_eq!(t.snapshot().len(), 1);
    assert!(t.active().is_none());
}
