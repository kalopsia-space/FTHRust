mod common;

use egui::{Pos2, Vec2, ViewportCommand};
use fthrust_core::window_geometry::{Rect as GRect, Screen};
use fthrust_ui::fonts::Weight;
use fthrust_ui::shell::{title_strip, window};
use fthrust_ui::state::AppState;
use fthrust_ui::tokens::Color;

fn state(settings: serde_json::Value) -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), settings);
    s.tokens.reduce_motion = true;
    (tmp, s)
}

#[test]
fn resize_edges_8px() {
    use egui::ResizeDirection::*;
    let size = Vec2::new(1100.0, 720.0);
    let edge = |x: f32, y: f32| window::resize_edge_at(Pos2::new(x, y), size, 8.0);
    assert_eq!(edge(3.0, 300.0), Some(West));
    assert_eq!(edge(1095.0, 300.0), Some(East));
    assert_eq!(edge(500.0, 2.0), Some(North));
    assert_eq!(edge(500.0, 715.0), Some(South));
    assert_eq!(edge(2.0, 2.0), Some(NorthWest));
    assert_eq!(edge(1099.0, 719.0), Some(SouthEast));
    assert_eq!(edge(1099.0, 1.0), Some(NorthEast));
    assert_eq!(edge(1.0, 719.0), Some(SouthWest));
    assert_eq!(edge(8.0, 300.0), None, "margin is exclusive");
    assert_eq!(edge(500.0, 300.0), None);
}

#[test]
fn geometry_restore_defaults_and_clamps_to_monitor() {
    let screens = [Screen {
        name: "DP-1".into(),
        available: GRect {
            x: 0,
            y: 0,
            width: 2560,
            height: 1400,
        },
        primary: true,
    }];
    let (_tmp, s) = state(serde_json::json!({}));
    let p = window::restore_geometry(&s.settings, &screens);
    assert!(
        p.maximized,
        "no saved geometry → maximised (reference default)"
    );
    assert_eq!((p.rect.width, p.rect.height), (1100, 720));
    // Saved on a monitor that no longer exists, off to the right: clamped onto the visible screen.
    let (_tmp, s) = state(serde_json::json!({"main_window_geometry": {
        "normal_geometry": {"x": 4000, "y": 100, "width": 1300, "height": 800},
        "screen_name": "HDMI-2", "maximized": false}}));
    let p = window::restore_geometry(&s.settings, &screens);
    assert!(!p.maximized);
    assert_eq!(
        p.rect.x + p.rect.width as i32,
        2560,
        "right edge pulled onto the screen"
    );
    assert_eq!((p.rect.width, p.rect.height), (1300, 800));
    // Persist round-trips through the settings key the reference uses.
    let (_tmp, mut s) = state(serde_json::json!({}));
    window::persist_geometry(
        &mut s.settings,
        GRect {
            x: 10,
            y: 20,
            width: 1200,
            height: 800,
        },
        true,
        "DP-1",
    );
    let saved = s.settings.get("main_window_geometry").cloned().unwrap();
    assert_eq!(saved["normal_geometry"]["width"], 1200);
    assert_eq!(saved["maximized"], true);
    assert_eq!(saved["screen_name"], "DP-1");
}

#[test]
fn frame_input_maps_to_viewport_commands() {
    let strip = egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 36.0));
    let size = Vec2::new(1100.0, 720.0);
    // Press on the strip → StartDrag; press on an edge → BeginResize; double-click → maximise toggle.
    assert!(matches!(
        window::frame_action(Pos2::new(300.0, 18.0), size, strip, false, false),
        Some(ViewportCommand::StartDrag)
    ));
    assert!(matches!(
        window::frame_action(Pos2::new(2.0, 300.0), size, strip, false, false),
        Some(ViewportCommand::BeginResize(egui::ResizeDirection::West))
    ));
    assert!(matches!(
        window::frame_action(Pos2::new(300.0, 18.0), size, strip, true, false),
        Some(ViewportCommand::Maximized(true))
    ));
    assert!(matches!(
        window::frame_action(Pos2::new(300.0, 18.0), size, strip, true, true),
        Some(ViewportCommand::Maximized(false))
    ));
    assert!(
        window::frame_action(Pos2::new(2.0, 300.0), size, strip, false, true).is_none(),
        "no resize while maximised"
    );
    assert!(window::frame_action(Pos2::new(300.0, 300.0), size, strip, false, false).is_none());
}

#[test]
fn title_strip_markers_match_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(1440.0, 36.0),
            )),
            |ui| {
                title_strip::ui(ui, &mut s);
            },
        );
    });
    let strip = r.marker("topBar").unwrap();
    assert_eq!(strip.rect.size(), Vec2::new(1440.0, 36.0));
    assert_eq!(
        r.pixel(Pos2::new(300.0, 18.0)),
        Some(tokens.color(Color::ShellBg))
    );
    let brand = r.marker("QLabel[FTHR Clips]").unwrap();
    assert_eq!(brand.rect.min.x, 56.0, "16 margin + 28 logo + 12 spacing");
    assert_eq!(brand.text.as_ref().unwrap().weight, Weight::W700);
    let logo = r.marker("logoLabel").unwrap();
    assert_eq!(logo.rect.size(), Vec2::new(28.0, 28.0));
    assert_eq!(
        r.marker("closeBtn").unwrap().rect.min,
        Pos2::new(1394.0, 0.0)
    );
    assert_eq!(
        r.marker("winBtn#2").unwrap().rect.min,
        Pos2::new(1336.0, 0.0)
    );
    assert_eq!(r.marker("winBtn").unwrap().rect.min, Pos2::new(1278.0, 0.0));
    let kofi = r
        .marker("QPushButton[Help us keep the app free and open for e]")
        .unwrap();
    assert!((kofi.rect.min.x - 932.0).abs() <= 1.5, "{:?}", kofi.rect);
    assert_eq!(kofi.rect.max.x, 1266.0);
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, &tokens);
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    assert_fidelity!(r, "settings", &tokens, &ctx);
}

#[test]
fn kofi_hidden_by_setting_and_buttons_report_clicks() {
    let (_tmp, mut s) = state(serde_json::json!({"show_kofi_widget": false}));
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 36.0), 1.0, &tokens, |ui| {
        title_strip::ui(ui, &mut s);
    });
    assert!(
        r.marker("QPushButton[Help us keep the app free and open for e]")
            .is_none()
    );
    let mut clicked = (false, false, false);
    let mut h = common::harness(Vec2::new(1440.0, 36.0), 1.0, &tokens, |ui| {
        let resp = title_strip::ui(ui, &mut s);
        clicked.0 |= resp.minimise;
        clicked.1 |= resp.maximise;
        clicked.2 |= resp.close;
    });
    for x in [1301.0, 1359.0, 1417.0] {
        h.drag_at(Pos2::new(x, 18.0));
        h.step();
        h.drop_at(Pos2::new(x, 18.0));
        h.run_steps(2);
    }
    drop(h);
    assert_eq!(clicked, (true, true, true));
}
