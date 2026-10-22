mod common;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::settings::{audio, visuals};
use fthrust_ui::shell;
use fthrust_ui::state::{AppState, Page, StatusStyle, StatusText, Tab};
use fthrust_ui::tokens::Color;

fn state(settings_json: serde_json::Value, tab: Tab) -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), settings_json);
    s.tokens.reduce_motion = true;
    s.page = Page::Settings;
    s.settings_tab = tab;
    s.status = StatusText {
        text: "STARTING CAPTURE".into(),
        style: StatusStyle::Idle,
    };
    (tmp, s)
}

fn ctx_for(tokens: &fthrust_ui::tokens::Tokens) -> egui::Context {
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, tokens);
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    ctx
}

#[test]
fn audio_tab_matches_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}), Tab::Audio);
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    assert_eq!(
        r.marker("QLabel[Microphone]").unwrap().rect.min,
        Pos2::new(118.0, 236.0),
        "shorter left column is centred against the right one"
    );
    assert_eq!(
        r.marker("QLabel[Notifications]").unwrap().rect.min,
        Pos2::new(792.0, 216.0)
    );
    assert_eq!(
        r.marker("_DropdownCombo[System Default]").unwrap().rect,
        Rect::from_min_size(Pos2::new(226.0, 278.0), Vec2::new(483.0, 36.0))
    );
    assert_eq!(
        r.marker("micRefreshBtn").unwrap().rect,
        Rect::from_min_size(Pos2::new(717.0, 283.0), Vec2::new(26.0, 26.0))
    );
    assert_eq!(
        r.marker("garyPanel").unwrap().rect,
        Rect::from_min_size(Pos2::new(118.0, 522.0), Vec2::new(625.0, 66.0))
    );
    assert_eq!(
        r.marker("garyImageButton[Default]").unwrap().rect.min,
        Pos2::new(682.0, 540.0)
    );
    assert_eq!(
        r.marker("QLabel[Upload failed]").unwrap().rect.min,
        Pos2::new(792.0, 576.0)
    );
    assert_eq!(r.marker("QLabel[Audio Capture]").unwrap().rect.min.y, 654.0);
    assert_eq!(
        r.marker("QComboBox[System default]").unwrap().rect,
        Rect::from_min_size(Pos2::new(295.0, 735.0), Vec2::new(1031.0, 38.0))
    );
    assert_eq!(
        r.marker("playbackDeviceRefresh").unwrap().rect.min.x,
        1338.0
    );
    assert_eq!(
        r.pixel(Pos2::new(767.0, 400.0)),
        Some(tokens.color(Color::ShellDivider)),
        "column separator"
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "audio", &tokens, &ctx);
}

#[test]
fn mic_meter_paints_level_peak_and_markers() {
    let (_tmp, mut s) = state(serde_json::json!({}), Tab::Audio);
    s.mic_level = Some((0.5, 0.75));
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(400.0, 36.0), 1.0, &tokens, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 36.0))),
            |ui| {
                audio::mic_meter(ui, &mut s, 400.0, "meter");
            },
        );
    });
    let bar_y = 13.0;
    assert_eq!(
        r.pixel(Pos2::new(0.0, bar_y)),
        Some(tokens.color(Color::Hairline)),
        "1 px HAIRLINE frame"
    );
    assert_eq!(
        r.pixel(Pos2::new(100.0, 18.0)),
        Some(tokens.color(Color::Accent)),
        "fill up to 50% of 398 px"
    );
    assert_eq!(
        r.pixel(Pos2::new(250.0, 18.0)),
        Some(tokens.color(Color::Bg)),
        "track beyond the level"
    );
    let peak_x = 1.0 + (398.0f32 * 0.75).floor();
    assert_eq!(
        r.pixel(Pos2::new(peak_x, 18.0)),
        Some(tokens.color(Color::Text)),
        "2 px peak bar"
    );
    let min_x = 1.0 + (398.0f32 * 55.0 / 100.0).floor();
    assert_eq!(
        r.pixel(Pos2::new(min_x, bar_y - 3.0)),
        Some(tokens.color(Color::Warning)),
        "MIN flag"
    );
    let max_x = 1.0 + (398.0f32 * 85.0 / 100.0).floor();
    assert_eq!(
        r.pixel(Pos2::new(max_x, bar_y - 3.0)),
        Some(tokens.color(Color::Text)),
        "MAX flag"
    );
    let m = r.marker("meter").unwrap();
    assert_eq!(m.rect.height(), 36.0);
}

#[test]
fn visuals_tab_matches_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}), Tab::Visuals);
    s.camera_devices = vec!["Camera 2".into()];
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    assert_eq!(
        r.marker("overlayOptionsPanel").unwrap().rect.min,
        Pos2::new(118.0, 258.0)
    );
    assert_eq!(r.marker("overlayOptionsPanel").unwrap().rect.width(), 380.0);
    assert_eq!(
        r.marker("_DropdownCombo[Camera 2]").unwrap().rect,
        Rect::from_min_size(Pos2::new(184.0, 318.0), Vec2::new(268.0, 36.0))
    );
    assert_eq!(
        r.marker("QLabel[0 IMAGE LAYERS]").unwrap().rect,
        Rect::from_min_size(Pos2::new(394.0, 432.0), Vec2::new(92.0, 38.0))
    );
    assert_eq!(
        r.marker("QPushButton[Clear all]").unwrap().rect,
        Rect::from_min_size(Pos2::new(401.0, 478.0), Vec2::new(85.0, 43.0))
    );
    assert_eq!(
        r.marker("QLabel[100%]").unwrap().rect.min,
        Pos2::new(452.0, 533.0)
    );
    assert_eq!(r.marker("QLabel[Key color]").unwrap().rect.min.y, 835.0);
    let pick = r.marker("QPushButton[Pick from preview]").unwrap().rect;
    assert!((pick.min.x - 350.0).abs() <= 1.0, "{pick:?}");
    assert_eq!(
        r.marker("overlayPreviewPanel").unwrap().rect.min,
        Pos2::new(514.0, 258.0)
    );
    assert_eq!(
        r.marker("QLabel[DEFAULT · DESKTOP]").unwrap().rect.min.x,
        1304.0
    );
    let stage = r.marker("unifiedOverlayPreview").unwrap().rect;
    assert_eq!(stage.min, Pos2::new(526.0, 287.0));
    assert_eq!(stage.width(), 870.0);
    assert!(stage.height() >= 613.0, "{stage:?}");
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "visuals", &tokens, &ctx);
}

#[test]
fn overlay_box_drag_moves_within_stage() {
    let (_tmp, mut s) = state(serde_json::json!({"camera_enabled": true}), Tab::Visuals);
    let tokens = s.tokens.clone();
    let before: serde_json::Value = s.settings.get("camera_overlay_rect").cloned().unwrap();
    let mut h = common::harness(Vec2::new(1440.0, 1200.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    h.step();
    let markers = fthrust_ui::fidelity::take(&h.ctx);
    let cam = markers
        .iter()
        .find(|m| m.name == "overlayBox[CAMERA]")
        .unwrap()
        .rect;
    let start = Pos2::new(cam.min.x + 30.0, cam.min.y + 10.0);
    h.hover_at(start);
    h.step();
    h.drag_at(start);
    h.step();
    for i in 1..=10 {
        h.hover_at(Pos2::new(
            start.x - 20.0 * i as f32,
            start.y - 10.0 * i as f32,
        ));
        h.step();
    }
    h.drop_at(Pos2::new(start.x - 200.0, start.y - 100.0));
    h.run_steps(2);
    drop(h);
    let after: serde_json::Value = s.settings.get("camera_overlay_rect").cloned().unwrap();
    assert_ne!(before, after, "drag persisted the camera rect");
    let x = after["x"].as_f64().unwrap();
    let y = after["y"].as_f64().unwrap();
    assert!(
        x < before["x"].as_f64().unwrap() && y < before["y"].as_f64().unwrap(),
        "{before} -> {after}"
    );
    assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
    let _ = visuals::DEFAULT_PREVIEW_SOURCE;
}
