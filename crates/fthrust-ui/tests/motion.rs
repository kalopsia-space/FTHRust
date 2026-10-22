use fthrust_ui::motion::{Ease, SmoothScroll, Transition};

#[test]
fn easings_match_qt_curves() {
    assert!((Ease::InOutSine.apply(0.25) - 0.1464).abs() < 1e-3);
    assert!((Ease::InOutSine.apply(0.5) - 0.5).abs() < 1e-6);
    assert!((Ease::OutCubic.apply(0.5) - 0.875).abs() < 1e-6);
    assert!((Ease::InCubic.apply(0.5) - 0.125).abs() < 1e-6);
    assert!((Ease::OutBack.apply(1.0) - 1.0).abs() < 1e-4);
    assert!(Ease::OutBack.apply(0.7) > 1.0, "overshoots");
    assert_eq!(Ease::Linear.apply(2.0), 1.0, "clamped");
}

#[test]
fn transition_progress_and_reduce_motion() {
    let tr = Transition::new(10.0, 220, Ease::Linear);
    assert_eq!(tr.progress(10.0, false), 0.0);
    assert!((tr.progress(10.11, false) - 0.5).abs() < 1e-3);
    assert_eq!(tr.progress(10.3, false), 1.0);
    assert!(tr.running(10.1, false));
    assert!(!tr.running(10.3, false));
    assert_eq!(
        tr.progress(10.0, true),
        1.0,
        "reduced motion jumps to the end"
    );
    assert!(!tr.running(10.0, true));
}

#[test]
fn smooth_scroll_converges_with_tau_35ms() {
    let mut s = SmoothScroll::default();
    s.wheel(-120.0, 20.0, 3.0);
    assert_eq!(
        s.target, 60.0,
        "one notch down = singleStep × wheelScrollLines"
    );
    assert!(s.tick(0.035));
    let gap = s.target - s.pos;
    assert!(
        (gap - 60.0 * (-1.0f32).exp()).abs() < 0.05,
        "after τ the gap is 1/e: {gap}"
    );
    let mut ticks = 0;
    while s.tick(0.008) {
        ticks += 1;
        assert!(ticks < 200);
    }
    assert_eq!(s.pos, s.target, "stops exactly on target when |Δ| < 0.5");
    assert!(ticks < 40, "settles within ~0.3 s: {ticks} ticks");
}
