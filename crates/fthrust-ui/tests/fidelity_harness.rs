mod common;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::fidelity::{self, QtScreen, QtWidget};
use fthrust_ui::fonts::Weight;
use fthrust_ui::tokens::{Color, Tokens};

fn screen_with(widgets: Vec<QtWidget>) -> QtScreen {
    QtScreen {
        screen: "synthetic".into(),
        size: [400, 300],
        widgets,
    }
}

fn qt_widget(name: &str, rect: [i32; 4]) -> QtWidget {
    serde_json::from_value(serde_json::json!({"name": name, "rect": rect, "samples": []})).unwrap()
}

fn painted_box(ui: &mut egui::Ui, tokens: &Tokens) {
    let rect = Rect::from_min_size(Pos2::new(20.0, 30.0), Vec2::new(100.0, 40.0));
    ui.painter()
        .rect_filled(rect, 0.0, tokens.color(Color::Surface3));
    ui.painter().rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, tokens.color(Color::Border)),
        egui::StrokeKind::Inside,
    );
    fidelity::mark(ui, "box", rect)
        .fill(Color::Surface3)
        .edge(Color::Border)
        .done();
}

#[test]
fn marker_registry_collects_per_frame() {
    let tokens = Tokens::default_palette();
    let r = common::render(Vec2::new(400.0, 300.0), 1.0, &tokens, |ui| {
        painted_box(ui, &tokens)
    });
    assert_eq!(
        r.markers.len(),
        1,
        "one marker per frame, not accumulated: {:?}",
        r.markers
    );
    assert_eq!(r.marker("box").unwrap().rect.min, Pos2::new(20.0, 30.0));
}

#[test]
fn renders_on_this_gpu_at_requested_size() {
    let tokens = Tokens::default_palette();
    let r = common::render(Vec2::new(400.0, 300.0), 1.0, &tokens, |ui| {
        painted_box(ui, &tokens)
    });
    assert_eq!(r.image.dimensions(), (400, 300));
    assert_eq!(
        r.pixel(Pos2::new(70.0, 50.0)),
        Some(tokens.color(Color::Surface3))
    );
    assert_eq!(
        r.pixel(Pos2::new(20.0, 50.0)),
        Some(tokens.color(Color::Border))
    );
}

#[test]
fn compare_accepts_within_one_px_and_flags_two() {
    let tokens = Tokens::default_palette();
    let r = common::render(Vec2::new(400.0, 300.0), 1.0, &tokens, |ui| {
        painted_box(ui, &tokens)
    });
    let ok = screen_with(vec![qt_widget("box", [21, 29, 100, 41])]);
    let report = fidelity::compare(&r.markers, &ok, &tokens, |p| r.pixel(p), |_, _| 0.0);
    assert_eq!(report.compared, 1);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    let off = screen_with(vec![qt_widget("box", [22, 30, 100, 40])]);
    let report = fidelity::compare(&r.markers, &off, &tokens, |p| r.pixel(p), |_, _| 0.0);
    assert_eq!(report.failures.len(), 1);
    assert!(
        report.failures[0].contains("x 20 vs Qt 22"),
        "{:?}",
        report.failures
    );
}

#[test]
fn compare_flags_wrong_token_pixel() {
    let tokens = Tokens::default_palette();
    let r = common::render(Vec2::new(400.0, 300.0), 1.0, &tokens, |ui| {
        let rect = Rect::from_min_size(Pos2::new(20.0, 30.0), Vec2::new(100.0, 40.0));
        ui.painter()
            .rect_filled(rect, 0.0, tokens.color(Color::Surface2));
        fidelity::mark(ui, "box", rect).fill(Color::Surface3).done();
    });
    let qt = screen_with(vec![qt_widget("box", [20, 30, 100, 40])]);
    let report = fidelity::compare(&r.markers, &qt, &tokens, |p| r.pixel(p), |_, _| 0.0);
    assert_eq!(report.failures.len(), 1);
    assert!(
        report.failures[0].contains("not SURFACE_3"),
        "{:?}",
        report.failures
    );
}

#[test]
fn compare_checks_text_size_weight_and_row_height() {
    let tokens = Tokens::default_palette();
    let r = common::render(Vec2::new(400.0, 300.0), 1.0, &tokens, |ui| {
        let rect = Rect::from_min_size(Pos2::new(20.0, 30.0), Vec2::new(100.0, 40.0));
        fidelity::mark(ui, "label", rect)
            .text("Hello", 17.0, Weight::W700)
            .done();
    });
    let mut w = qt_widget("label", [20, 30, 100, 40]);
    w.text = Some(serde_json::from_value(serde_json::json!({"content": "Hello", "px": 17, "weight": 700, "ascent": 20, "descent": 5, "height": 25, "advance": 40})).unwrap());
    let qt = screen_with(vec![w]);
    let report = fidelity::compare(&r.markers, &qt, &tokens, |p| r.pixel(p), |_, _| 25.5);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    let report = fidelity::compare(&r.markers, &qt, &tokens, |p| r.pixel(p), |_, _| 27.0);
    assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
    let mut w2 = qt.widgets[0].clone();
    w2.text.as_mut().unwrap().weight = 400;
    let qt2 = screen_with(vec![w2]);
    let report = fidelity::compare(&r.markers, &qt2, &tokens, |p| r.pixel(p), |_, _| 25.0);
    assert!(
        report.failures.iter().any(|f| f.contains("weight")),
        "{:?}",
        report.failures
    );
}

#[test]
fn layout_equal_across_ppp() {
    let tokens = Tokens::default_palette();
    let draw = |ui: &mut egui::Ui| painted_box(ui, &tokens);
    let one = common::render(Vec2::new(400.0, 300.0), 1.0, &tokens, draw);
    let two = common::render(Vec2::new(400.0, 300.0), 2.0, &tokens, draw);
    assert_eq!(
        one.marker("box").unwrap().rect,
        two.marker("box").unwrap().rect
    );
    assert_eq!(two.image.dimensions(), (800, 600));
    assert_eq!(
        two.pixel(Pos2::new(70.0, 50.0)),
        Some(tokens.color(Color::Surface3))
    );
}

#[test]
fn qt_screen_json_loads() {
    let qt = QtScreen::load("settings").unwrap();
    assert_eq!(qt.size, [1440, 900]);
    let bar = qt.widget("settingsTabBar").unwrap();
    assert_eq!(bar.rect[2], 1346);
    assert!(
        qt.widget("settingsTabBtn[General]")
            .unwrap()
            .checked
            .unwrap()
    );
}

#[test]
fn ssim_is_one_for_identical_images() {
    let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([10, 200, 30, 255]));
    assert!((fidelity::ssim(&img, &img).unwrap() - 1.0).abs() < 1e-9);
    let other = image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 10, 30, 255]));
    assert!(fidelity::ssim(&img, &other).unwrap() < 0.9);
}
