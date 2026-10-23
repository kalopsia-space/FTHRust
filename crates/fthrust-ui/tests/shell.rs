mod common;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::shell::error_bar::{Action, ErrorBarState, Level};
use fthrust_ui::shell::{self, nav_rail, page_stack, toolbar};
use fthrust_ui::state::{AppState, Page, StatusStyle, StatusText};
use fthrust_ui::tokens::Color;

fn state(settings: serde_json::Value) -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), settings);
    s.tokens.reduce_motion = true;
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
fn capture_summary_text_rules() {
    let (_tmp, s) = state(serde_json::json!({}));
    assert_eq!(
        toolbar::capture_summary(&s.settings),
        "30s // 60 fps // Source // 25,000 kbps"
    );
    let (_tmp, s) = state(
        serde_json::json!({"clip_length": 90, "framerate": 30, "resolution": "1080p", "bitrate_level": "high"}),
    );
    assert_eq!(
        toolbar::capture_summary(&s.settings),
        "1.5m // 30 fps // 1080p // 50,000 kbps"
    );
    let (_tmp, s) = state(
        serde_json::json!({"clip_length": 120, "bitrate_level": "custom", "custom_bitrate_kbps": 123500}),
    );
    assert_eq!(
        toolbar::capture_summary(&s.settings),
        "2m // 60 fps // Source // 123,500 kbps"
    );
    let (_tmp, s) = state(serde_json::json!({"clip_length": 100}));
    assert!(toolbar::capture_summary(&s.settings).starts_with("1.67m // "));
    assert_eq!(
        toolbar::game_summary(&s.settings, true),
        "Game detection: Off"
    );
    let (_tmp, s) = state(serde_json::json!({"game_detection_enabled": true}));
    assert_eq!(
        toolbar::game_summary(&s.settings, true),
        "Game detection: On"
    );
}

#[test]
fn stretch_widths_follow_qt_rounding() {
    // 1440 wide: 1440 − 28 margins − 5×8 spacing − 110 status = 1262 over stretch 1:3:2:2:1.
    let widths = toolbar::stretch_widths(1262.0, &[1, 3, 2, 2, 1], &[60.0; 5]);
    assert_eq!(widths, vec![140.0, 421.0, 280.0, 281.0, 140.0]);
    // A hint larger than its share takes the hint; the rest share what is left.
    let widths = toolbar::stretch_widths(1000.0, &[1, 3], &[600.0, 100.0]);
    assert_eq!(widths, vec![600.0, 400.0]);
    // Hints that overflow are kept (the toolbar scrolls).
    let widths = toolbar::stretch_widths(300.0, &[1, 1], &[200.0, 200.0]);
    assert_eq!(widths, vec![200.0, 200.0]);
}

#[test]
fn status_label_capitalizes_and_styles() {
    assert_eq!(
        toolbar::status_display("STARTING CAPTURE"),
        "Starting capture"
    );
    assert_eq!(toolbar::status_display("connecting"), "Connecting");
    assert_eq!(toolbar::status_display(""), "");
    assert_eq!(toolbar::status_color(StatusStyle::Active), Color::Accent);
    assert_eq!(toolbar::status_color(StatusStyle::Idle), Color::TextMuted);
    assert_eq!(toolbar::status_color(StatusStyle::Warning), Color::Error);
}

#[test]
fn toolbar_rail_and_stack_markers_match_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    s.page = Page::Settings;
    s.status = StatusText {
        text: "STARTING CAPTURE".into(),
        style: StatusStyle::Idle,
    };
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    let names = [
        "captureToolbarScroll",
        "topClusterMain",
        "QPushButton[Record]",
        "TopBarButton[30s // 60 fps // Source // 25,000 kbps]",
        "TopBarButton[Source]",
        "TopBarButton[Game detection: Off]",
        "TopBarButton[Hotkeys]",
        "statusLabel",
        "navigationPanel",
        "homeBtn",
        "settingsGearBtn",
        "mainStack",
    ];
    for n in names {
        assert!(r.marker(n).is_some(), "missing marker {n}");
    }
    assert_eq!(
        r.marker("QPushButton[Record]").unwrap().rect,
        Rect::from_min_size(Pos2::new(14.0, 46.0), Vec2::new(140.0, 44.0))
    );
    assert_eq!(
        r.marker("statusLabel").unwrap().rect,
        Rect::from_min_size(Pos2::new(1316.0, 42.0), Vec2::new(110.0, 52.0))
    );
    assert_eq!(
        r.marker("settingsGearBtn").unwrap().rect.min,
        Pos2::new(8.0, 206.0)
    );
    assert_eq!(
        r.pixel(Pos2::new(700.0, 60.0)),
        Some(tokens.color(Color::Surface3)),
        "toolbar background"
    );
    assert_eq!(
        r.pixel(Pos2::new(50.0, 500.0)),
        Some(tokens.color(Color::ShellBg2)),
        "rail background"
    );
    assert_eq!(
        r.pixel(Pos2::new(93.0, 500.0)),
        Some(tokens.color(Color::ShellDivider)),
        "rail right border"
    );
    assert_eq!(
        r.pixel(Pos2::new(700.0, 100.0)),
        Some(tokens.color(Color::ShellDivider)),
        "stack top border"
    );
    assert_eq!(
        r.pixel(Pos2::new(20.0, 240.0)),
        Some(tokens.color(Color::AccentSoft)),
        "checked Settings button"
    );
    assert_eq!(
        r.pixel(Pos2::new(9.0, 240.0)),
        Some(tokens.color(Color::Accent)),
        "3 px left accent on the checked button"
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "settings", &tokens, &ctx);
    // The clips page in the library screen: home button checked instead.
    s.page = Page::Clips;
    s.status = StatusText {
        text: "CONNECTING".into(),
        style: StatusStyle::Idle,
    };
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    assert_eq!(
        r.pixel(Pos2::new(20.0, 150.0)),
        Some(tokens.color(Color::AccentSoft)),
        "checked Clips button"
    );
    assert_fidelity!(r, "library", &tokens, &ctx);
}

#[test]
fn rail_clicks_switch_pages_and_toolbar_opens_popups() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    let gear = Pos2::new(47.0, 245.0);
    h.drag_at(gear);
    h.step();
    h.drop_at(gear);
    h.run_steps(3);
    let hotkeys = Pos2::new(1238.0, 68.0);
    h.drag_at(hotkeys);
    h.step();
    h.drop_at(hotkeys);
    h.run_steps(3);
    drop(h);
    assert_eq!(s.page, Page::Settings);
    assert!(s.popups.hotkeys.open);
}

#[test]
fn page_slide_takes_220ms_and_reduce_motion_jumps() {
    let mut slide = page_stack::PageSlide::default();
    slide.begin(Page::Clips, Page::Settings, 10.0, false);
    assert!(slide.running(10.1, false));
    let (out_off, in_off) = slide.offsets(10.11, 800.0, false);
    assert!(
        (out_off + 400.0).abs() < 2.0,
        "outgoing half way up: {out_off}"
    );
    assert!(
        (in_off - 400.0).abs() < 2.0,
        "incoming half way in: {in_off}"
    );
    assert!(!slide.running(10.3, false));
    let (o, i) = slide.offsets(10.3, 800.0, false);
    assert_eq!((o, i), (-800.0, 0.0));
    let mut slide = page_stack::PageSlide::default();
    slide.begin(Page::Settings, Page::Clips, 10.0, true);
    assert!(!slide.running(10.0, true));
    assert_eq!(
        slide.offsets(10.0, 800.0, true),
        (800.0, 0.0),
        "downward direction, finished at once"
    );
}

#[test]
fn error_bar_queue_and_dismiss_semantics() {
    let mut bar = ErrorBarState::default();
    assert!(!bar.visible());
    assert!(bar.push_error("capture failed", "x", Level::Error, vec![], None, true));
    assert_eq!(
        bar.current().unwrap().title,
        "[Error 003] CAPTURE FAILED".to_owned().replace(
            "[Error 003] ",
            &format!(
                "[{}] ",
                fthrust_core::error_codes::code_for_title("CAPTURE FAILED")
                    .map(|c| c.code)
                    .unwrap_or("?")
            )
        )
    );
    assert!(
        bar.push_error("unknown thing", "y", Level::Warning, vec![], None, true),
        "unknown titles show normalised"
    );
    assert_eq!(bar.queued_count(), 1);
    assert_eq!(bar.more_label(), Some("+1 MORE".to_owned()));
    bar.push_error("keyed", "z", Level::Warning, vec![], Some("k".into()), true);
    bar.push_error(
        "keyed again",
        "z2",
        Level::Warning,
        vec![],
        Some("k".into()),
        true,
    );
    assert_eq!(
        bar.queued_count(),
        2,
        "a keyed push replaces its queued entry"
    );
    bar.dismiss_current();
    assert_eq!(bar.current().unwrap().title, "UNKNOWN THING");
    assert!(bar.remove_by_key("k"));
    assert_eq!(bar.queued_count(), 0);
    bar.dismiss_current();
    assert!(!bar.visible());
    bar.enabled = false;
    assert!(
        !bar.push_error("muted", "m", Level::Error, vec![], None, true),
        "error_notifications_enabled=false suppresses"
    );
    assert!(!bar.visible());
}

#[test]
fn error_bar_markers_match_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    s.error_bar.push(
        "CAPTURE FAILED",
        "The compositor stopped answering",
        Level::Error,
        vec![Action {
            label: "RESTART ENGINE".into(),
            id: "restart".into(),
        }],
        None,
        true,
    );
    let bar_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 36.0));
    let r = common::render(Vec2::new(1440.0, 36.0), 1.0, &tokens, |ui| {
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
            shell::error_bar::ui(ui, &mut s)
        });
    });
    assert_eq!(
        r.pixel(Pos2::new(600.0, 0.0)),
        Some(tokens.color(Color::Error)),
        "1 px top border in the level colour"
    );
    assert_eq!(
        r.pixel(Pos2::new(600.0, 18.0)),
        Some(tokens.color(Color::Surface1))
    );
    assert_eq!(
        r.pixel(Pos2::new(30.0, 1.0)),
        Some(tokens.color(Color::Error)),
        "Qt's QFrame rule gives every label a top border too"
    );
    let notice = r.marker("QLabel[Notice]").unwrap();
    assert_eq!(
        notice.rect,
        Rect::from_min_size(Pos2::new(20.0, 1.0), Vec2::new(41.0, 35.0))
    );
    let action = r.marker("QPushButton[RESTART ENGINE]").unwrap();
    assert_eq!(action.rect.min.y, 4.0);
    assert_eq!(action.rect.height(), 29.0);
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "error-bar-error", &tokens, &ctx);
    s.error_bar.push(
        "LIMITED FEATURE SUPPORT",
        "Game detection is unavailable on this compositor",
        Level::Warning,
        vec![Action {
            label: "LEARN MORE".into(),
            id: "learn".into(),
        }],
        Some("compositor".into()),
        true,
    );
    s.error_bar.push(
        "MIC NOT FOUND",
        "No microphone is connected",
        Level::Warning,
        vec![],
        None,
        true,
    );
    let r = common::render(Vec2::new(1440.0, 36.0), 1.0, &tokens, |ui| {
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
            shell::error_bar::ui(ui, &mut s)
        });
    });
    assert_eq!(
        r.marker("QLabel[+2 MORE]").unwrap().rect.min,
        Pos2::new(1207.0, 1.0)
    );
    assert_fidelity!(r, "error-bar-warning", &tokens, &ctx);
}

#[test]
fn error_bar_actions_and_dismiss_click() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    s.error_bar.push(
        "CAPTURE FAILED",
        "x",
        Level::Error,
        vec![Action {
            label: "RESTART ENGINE".into(),
            id: "restart".into(),
        }],
        None,
        true,
    );
    let mut fired = None;
    let bar_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 36.0));
    let mut h = common::harness(Vec2::new(1440.0, 36.0), 1.0, &tokens, |ui| {
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
            if let Some(id) = shell::error_bar::ui(ui, &mut s) {
                fired = Some(id);
            }
        });
    });
    h.drag_at(Pos2::new(1325.0, 18.0));
    h.step();
    h.drop_at(Pos2::new(1325.0, 18.0));
    h.run_steps(2);
    drop(h);
    assert_eq!(fired.as_deref(), Some("restart"));
    assert!(!s.error_bar.visible(), "dismissed on action");
    let _ = nav_rail::WIDTH;
}
