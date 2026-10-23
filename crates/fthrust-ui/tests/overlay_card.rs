//! Plan 5 Task 15: the capture card overlay.

mod common;

use egui::{Pos2, Vec2};
use fthrust_ui::overlay::capture_card::notification_body;
use fthrust_ui::overlay::capture_card::{
    COMPACT_H, COMPACT_W, CardModel, FULL_H, FULL_W, HOLD_S, SLIDE_IN_S, SLIDE_OUT_S, content,
    paint,
};
use fthrust_ui::overlay::surface::{pick_output, scale_for};
use fthrust_ui::services::CardKind;
use fthrust_ui::state::AppState;
use fthrust_wayland::outputs::OutputInfo;

fn clip() -> CardKind {
    CardKind::Clip {
        seconds: 30,
        fps: 60,
        resolution: "1920x1080".into(),
    }
}

#[test]
fn card_timeline_values() {
    let mut m = CardModel::default();
    assert!(m.frame(0.0).is_none(), "nothing shown");
    m.show(clip(), None, 0.0);
    let (w, h) = m.size();
    assert_eq!((w, h), (FULL_W, FULL_H));
    let f = m.frame(0.0).unwrap();
    assert!(
        f.x_offset >= w + 10.0 - 0.01,
        "starts off-screen: {}",
        f.x_offset
    );
    assert_eq!(f.shimmer, -1.0);
    assert_eq!(f.progress, 1.0);
    let f = m.frame(0.1).unwrap();
    assert_eq!(f.shimmer, -1.0, "shimmer waits 150 ms");
    let f = m.frame(0.425).unwrap();
    assert!((f.shimmer - 1.5).abs() < 0.01, "{}", f.shimmer);
    let f = m.frame(SLIDE_IN_S).unwrap();
    assert!(f.x_offset.abs() < 0.01, "arrived: {}", f.x_offset);
    let f = m.frame(0.3).unwrap();
    assert!(
        f.x_offset < 0.0,
        "OutBack overshoots past the resting point: {}",
        f.x_offset
    );
    let f = m.frame(SLIDE_IN_S + 1.2).unwrap();
    // Progress starts 30 ms after the slide-in like the reference.
    let expected = (1.0 - (1.2 - 0.03) / HOLD_S) as f32;
    assert!(
        (f.progress - expected).abs() < 0.01,
        "{} vs {expected}",
        f.progress
    );
    let f = m.frame(SLIDE_IN_S + HOLD_S).unwrap();
    assert!(
        f.x_offset.abs() < 0.01,
        "still resting at the end of the hold"
    );
    assert!(f.progress.abs() < 0.02);
    let f = m.frame(SLIDE_IN_S + HOLD_S + SLIDE_OUT_S * 0.5).unwrap();
    assert!(
        f.x_offset > 0.0 && f.x_offset < w + 10.0,
        "sliding out: {}",
        f.x_offset
    );
    assert!(
        m.frame(SLIDE_IN_S + HOLD_S + SLIDE_OUT_S + 0.001).is_none(),
        "gone"
    );
    // A custom hold and a compact kind.
    m.show(CardKind::Screenshot, Some(500), 10.0);
    assert_eq!(m.size(), (COMPACT_W, COMPACT_H));
    assert!(
        m.frame(10.0 + SLIDE_IN_S + 0.5 + SLIDE_OUT_S + 0.001)
            .is_none()
    );
    // Showing again restarts the timeline.
    m.show(clip(), None, 20.0);
    assert!(m.frame(20.0).unwrap().x_offset >= w + 10.0 - 0.01);
}

#[test]
fn card_content_matches_reference() {
    let c = content(&clip());
    assert_eq!(c.headline, "CLIP CAPTURED");
    assert!(!c.compact);
    assert_eq!(
        c.stats,
        vec![
            ("30s".to_string(), "Duration".to_string()),
            ("60 FPS".into(), "Framerate".into()),
            ("1920x1080".into(), "Resolution".into())
        ]
    );
    let c = content(&CardKind::Screenshot);
    assert_eq!((c.headline.as_str(), c.compact), ("SCREENSHOT SAVED", true));
    let c = content(&CardKind::Error { detail: "x".into() });
    assert_eq!(
        (c.headline.as_str(), c.detail.as_str()),
        ("CAPTURE FAILED", "x")
    );
    let c = content(&CardKind::Upload {
        filename: "a.mp4".into(),
    });
    assert_eq!(
        (c.headline.as_str(), c.detail.as_str()),
        ("CLIP UPLOADED", "a.mp4")
    );
    let c = content(&CardKind::UploadFailed { detail: "d".into() });
    assert_eq!(c.headline, "UPLOAD FAILED");
    let c = content(&CardKind::RecordingSaved {
        filename: "r.mp4".into(),
    });
    assert_eq!(
        (c.headline.as_str(), c.detail.as_str()),
        (
            "MANUAL RECORDING SAVED",
            "r.mp4  ·  Ready to trim and export."
        )
    );
    let c = content(&CardKind::RecordingSaved {
        filename: String::new(),
    });
    assert_eq!(c.detail, "Ready to trim and export.");
    let c = content(&CardKind::Prompt { text: "p".into() });
    assert_eq!(
        (c.headline.as_str(), c.detail.as_str()),
        ("CAPTURE UPDATE", "p")
    );
    let c = content(&CardKind::Capturing {
        source: String::new(),
    });
    assert_eq!(
        (c.headline.as_str(), c.detail.as_str()),
        ("NOW CAPTURING", "Desktop")
    );
    assert_eq!(
        fthrust_ui::services::sounds::sound_key_for(&clip()),
        Some("clip_captured")
    );
    assert_eq!(
        fthrust_ui::services::sounds::sound_key_for(&CardKind::Screenshot),
        Some("screenshot_captured")
    );
    assert_eq!(
        fthrust_ui::services::sounds::sound_key_for(&CardKind::Startup),
        Some("startup")
    );
    assert_eq!(
        fthrust_ui::services::sounds::sound_key_for(&CardKind::Prompt { text: "p".into() }),
        None
    );
}

#[test]
fn card_paint_matches_reference_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    let tokens = s.tokens.clone();
    let mut model = CardModel::default();
    model.show(clip(), None, 0.0);
    // Past the shimmer sweep (0.15–0.70 s) so the background samples clean.
    let frame = model.frame(SLIDE_IN_S + 0.5).unwrap();
    let mut layout = None;
    let rendered = common::render(Vec2::new(FULL_W, FULL_H), 1.0, &tokens, |ui| {
        let rect = egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(FULL_W, FULL_H));
        layout = Some(paint(ui.painter(), ui.ctx(), &tokens, &frame, rect, 1.0));
    });
    let layout = layout.unwrap();
    let cc = &tokens.capture_card;
    assert_eq!(
        rendered.pixel(Pos2::new(1.0, 60.0)),
        Some(cc.accent),
        "3 px accent rail"
    );
    assert_eq!(
        rendered.pixel(Pos2::new(200.0, 100.0)),
        Some(cc.bg),
        "card background"
    );
    assert_eq!(layout.divider_y, Some(47.0));
    assert_eq!(layout.stats_x, vec![13.0, 118.0, 223.0]);
    assert_eq!(
        layout.headline_rect,
        egui::Rect::from_min_size(Pos2::new(55.0, 12.0), Vec2::new(FULL_W - 55.0 - 12.0, 28.0))
    );
    assert_eq!(
        layout.icon_rect,
        egui::Rect::from_min_size(Pos2::new(20.0, 17.0), Vec2::splat(26.0))
    );
    assert_eq!(
        rendered.pixel(Pos2::new(150.0, FULL_H - 1.0)),
        Some(cc.progress_fill),
        "progress fill near the start of the hold"
    );
    assert_eq!(
        rendered.pixel(Pos2::new(FULL_W - 1.0, FULL_H - 1.0)),
        Some(cc.progress_track),
        "progress track at the far end"
    );
    // Compact layout.
    let mut model = CardModel::default();
    model.show(CardKind::Screenshot, None, 0.0);
    // Past the shimmer sweep (0.15–0.70 s) so the background samples clean.
    let frame = model.frame(SLIDE_IN_S + 0.5).unwrap();
    let mut layout = None;
    let _ = common::render(Vec2::new(COMPACT_W, COMPACT_H), 1.0, &tokens, |ui| {
        let rect = egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(COMPACT_W, COMPACT_H));
        layout = Some(paint(ui.painter(), ui.ctx(), &tokens, &frame, rect, 1.0));
    });
    let layout = layout.unwrap();
    assert_eq!(layout.divider_y, None);
    assert_eq!(
        layout.headline_rect,
        egui::Rect::from_min_size(
            Pos2::new(45.0, 8.0),
            Vec2::new(COMPACT_W - 45.0 - 12.0, 25.0)
        )
    );
    assert_eq!(
        layout.icon_rect,
        egui::Rect::from_min_size(Pos2::new(15.0, 13.0), Vec2::splat(20.0))
    );
}

#[test]
fn fallback_output_when_named_output_missing() {
    let o = |name: &str, h: u32, refresh: u32| OutputInfo {
        name: name.into(),
        height: h,
        width: 1920,
        refresh_mhz: refresh,
        done: true,
        ..Default::default()
    };
    let outputs = vec![
        o("DP-1", 1440, 144_000),
        o("HDMI-A-1", 1080, 60_000),
        o("DP-2", 1080, 165_000),
    ];
    assert_eq!(
        pick_output(&outputs, "DP-9").map(|o| o.name.as_str()),
        Some("DP-2"),
        "highest refresh when the named one is missing"
    );
    assert_eq!(
        pick_output(&outputs, "HDMI-A-1").map(|o| o.name.as_str()),
        Some("HDMI-A-1")
    );
    assert_eq!(
        pick_output(&outputs, "auto").map(|o| o.name.as_str()),
        Some("DP-2")
    );
    assert_eq!(
        pick_output(&outputs, "").map(|o| o.name.as_str()),
        Some("DP-2")
    );
    assert!(pick_output(&[], "DP-1").is_none());
    assert_eq!(scale_for(1080), 1.0);
    assert_eq!(scale_for(2160), 1.0);
    assert!((scale_for(900) - 900.0 / 1080.0).abs() < 1e-6);
    assert_eq!(scale_for(480), 0.65);
}

#[test]
#[ignore = "needs a layer-shell compositor and a GPU"]
fn overlay_live_shows_card() {
    let tmp = tempfile::tempdir().unwrap();
    let s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    let overlay =
        fthrust_ui::overlay::Overlay::start(s.tokens.clone(), None, fthrust_ui::assets_dir());
    overlay.show(clip(), None);
    std::thread::sleep(std::time::Duration::from_millis(3500));
    assert!(overlay.last_error().is_none(), "{:?}", overlay.last_error());
    assert!(
        overlay.frames_rendered() > 60,
        "rendered {} frames",
        overlay.frames_rendered()
    );
    overlay.shutdown();
}

#[test]
fn notification_body_carries_the_card_stats() {
    assert_eq!(
        notification_body(&content(&clip())),
        "30s · 60 FPS · 1920x1080"
    );
    assert_eq!(
        notification_body(&content(&CardKind::Capturing {
            source: "Hades".into()
        })),
        "Hades"
    );
    assert_eq!(notification_body(&content(&CardKind::Screenshot)), "");
}
