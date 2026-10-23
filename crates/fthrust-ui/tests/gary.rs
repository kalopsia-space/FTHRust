//! Mic level and Gary mode (notes-overlays-reference sections 9–11).

use std::time::{Duration, Instant};

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::overlay::gary::{GaryState, picture_rect};
use fthrust_ui::services::mic_level::Meter;

#[test]
fn level_is_rms_times_four() {
    let mut m = Meter::default();
    let t0 = Instant::now();
    // Stereo, channel 0 a constant 0.1 (rms 0.1), channel 1 loud and ignored.
    let block: Vec<f32> = (0..480).flat_map(|_| [0.1f32, 0.9]).collect();
    m.feed(&block, 2, 1.0, t0);
    let (level, peak) = m.read(t0);
    assert!((level - 0.4).abs() < 1e-4, "{level}");
    assert!((peak - 0.4).abs() < 1e-4);
    // Gain applies before the RMS, and the level clamps at 1.
    m.feed(&block, 2, 4.0, t0);
    assert_eq!(m.read(t0).0, 1.0);
}

#[test]
fn stale_level_is_zero() {
    let mut m = Meter::default();
    let t0 = Instant::now();
    m.feed(&[0.2f32; 960], 1, 1.0, t0);
    assert!(m.read(t0 + Duration::from_millis(300)).0 > 0.0);
    assert_eq!(m.read(t0 + Duration::from_millis(400)).0, 0.0);
}

#[test]
fn gary_rises_and_falls_at_reference_rates() {
    let mut g = GaryState::default();
    let mut roll = || 5;
    // A loud mic (level 1.0) with thresholds 55/85: up in six ticks.
    let mut ticks = 0;
    while g.tick(1.0, true, (55, 85), &mut roll) < 1.0 {
        ticks += 1;
        assert!(ticks < 10);
    }
    assert_eq!(ticks, 5);
    // Silence: down in about 15 ticks.
    let mut ticks = 0;
    while g.tick(0.0, true, (55, 85), &mut roll) > 0.0 {
        ticks += 1;
        assert!(ticks < 20);
    }
    assert_eq!(ticks, 15);
}

#[test]
fn cat_rolls_once_per_appearance() {
    let mut g = GaryState::default();
    let mut next = 0u32;
    let mut rolls = 0;
    let mut roll = || {
        rolls += 1;
        let r = next % 10;
        next += 1;
        r
    };
    let mut cats = 0;
    for _ in 0..10 {
        // Appear, hold, disappear: one roll per appearance.
        for _ in 0..8 {
            g.tick(1.0, true, (55, 85), &mut roll);
        }
        if g.cat() {
            cats += 1;
        }
        for _ in 0..20 {
            g.tick(0.0, true, (55, 85), &mut roll);
        }
    }
    assert_eq!(rolls, 10);
    assert_eq!(cats, 1);
}

#[test]
fn disabled_hides_at_once() {
    let mut g = GaryState::default();
    let mut roll = || 5;
    for _ in 0..8 {
        g.tick(1.0, true, (55, 85), &mut roll);
    }
    assert_eq!(g.tick(1.0, false, (55, 85), &mut roll), 0.0);
}

#[test]
fn picture_geometry() {
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(2000.0, 1000.0));
    // Full intensity: fit a square image in 58 % × 76 % of the screen.
    let r = picture_rect(screen, Vec2::new(3072.0, 3072.0), 1.0);
    assert!(
        (r.width() - 760.0).abs() < 0.5 && (r.height() - 760.0).abs() < 0.5,
        "{r:?}"
    );
    assert!((r.center() - screen.center()).length() < 0.5);
    // At the threshold the picture is 55 % of that box.
    let r = picture_rect(screen, Vec2::new(3072.0, 3072.0), 0.0);
    assert!((r.width() - 418.0).abs() < 0.5, "{r:?}");
}
