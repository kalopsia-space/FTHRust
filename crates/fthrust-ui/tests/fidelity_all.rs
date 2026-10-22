//! The fidelity gate: every Qt reference screen this plan owns is rendered
//! from the same state the Qt renderer used and compared marker for marker
//! (`docs/ui-fidelity.md`). Screens owned by later plans are ignored with
//! the owning plan named.

mod common;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::dialogs;
use fthrust_ui::popups::{capture_settings, game_detection, hotkeys, source};
use fthrust_ui::shell;
use fthrust_ui::shell::error_bar::{Action, Level};
use fthrust_ui::state::{AppState, DialogKind, Page, StatusStyle, StatusText, Tab};
use fthrust_ui::widgets::popup;

fn screen_size(screen: &str) -> Vec2 {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/reference/qt-screens/"
    );
    let json = std::fs::read_to_string(format!("{path}{screen}.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    Vec2::new(
        v["size"][0].as_f64().unwrap() as f32,
        v["size"][1].as_f64().unwrap() as f32,
    )
}

fn state() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.tokens.reduce_motion = true;
    (tmp, s)
}

fn check(screen: &str, mut s: AppState, draw: impl FnMut(&mut egui::Ui, &mut AppState)) {
    let mut draw = draw;
    let tokens = s.tokens.clone();
    let size = screen_size(screen);
    let r = common::render(size, 1.0, &tokens, |ui| draw(ui, &mut s));
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, &tokens);
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    assert_fidelity!(r, screen, &tokens, &ctx);
}

fn settings_tab(screen: &str, tab: Tab, open: &[usize]) {
    let (_tmp, mut s) = state();
    s.page = Page::Settings;
    s.settings_tab = tab;
    s.status = StatusText {
        text: "STARTING CAPTURE".into(),
        style: StatusStyle::Idle,
    };
    for i in open {
        s.customize_open[*i] = true;
    }
    check(screen, s, |ui, s| {
        shell::frame(ui, s);
    });
}

fn popup_screen(
    screen: &str,
    name: &str,
    s: AppState,
    mut content: impl FnMut(&mut egui::Ui, &mut AppState),
) {
    let width = screen_size(screen).x;
    check(screen, s, |ui, s| {
        let t = s.tokens.clone();
        popup::draw_frame(ui, &t, name, Pos2::ZERO, width, |ui| content(ui, s));
    });
}

fn dialog_screen(screen: &str, kind: DialogKind) {
    let (_tmp, s) = state();
    check(screen, s, |ui, s| {
        dialogs::draw_at(ui, s, &kind, Pos2::ZERO);
    });
}

#[test]
fn settings() {
    settings_tab("settings", Tab::General, &[]);
}

#[test]
fn clip_settings() {
    settings_tab("clip-settings", Tab::Clip, &[]);
}

#[test]
fn audio() {
    settings_tab("audio", Tab::Audio, &[]);
}

#[test]
fn visuals() {
    settings_tab("visuals", Tab::Visuals, &[]);
}

#[test]
fn customize() {
    for screen in [
        "customize",
        "customize-1920",
        "customize-2560",
        "customize-3840",
    ] {
        settings_tab(screen, Tab::Customize, &[]);
    }
}

#[test]
fn typography() {
    settings_tab("typography", Tab::Customize, &[0]);
}

#[test]
fn colors() {
    settings_tab("colors", Tab::Customize, &[1]);
}

#[test]
fn performance() {
    settings_tab("performance", Tab::Performance, &[]);
}

#[test]
fn shortcuts() {
    settings_tab("shortcuts", Tab::Shortcuts, &[]);
}

#[test]
fn library() {
    let (_tmp, mut s) = state();
    s.page = Page::Clips;
    s.status = StatusText {
        text: "CONNECTING".into(),
        style: StatusStyle::Idle,
    };
    check("library", s, |ui, s| {
        shell::frame(ui, s);
    });
}

#[test]
fn capture_popup() {
    let (_tmp, s) = state();
    popup_screen("capture-popup", "capture", s, capture_settings::content);
}

#[test]
fn source_popup() {
    let (_tmp, mut s) = state();
    s.monitors = vec![fthrust_ui::state::MonitorInfo {
        name: "DP-1".into(),
        width: 2560,
        height: 1400,
        refresh_hz: 165,
    }];
    popup_screen("source-popup", "source", s, source::content);
}

#[test]
fn game_detection_popup() {
    let (_tmp, mut s) = state();
    s.running_windows = vec!["Steam · Steam".into()];
    popup_screen("game-detection-popup", "game", s, game_detection::content);
}

#[test]
fn hotkey_popup() {
    let (_tmp, s) = state();
    popup_screen("hotkey-popup", "hotkeys", s, |ui, s| {
        hotkeys::content(ui, s)
    });
}

#[test]
fn error_bars() {
    let (_tmp, mut s) = state();
    let bar = Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 36.0));
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
    let tokens = s.tokens.clone();
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, &tokens);
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let r = common::render(screen_size("error-bar-error"), 1.0, &tokens, |ui| {
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
            shell::error_bar::ui(ui, &mut s)
        });
    });
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
    let r = common::render(screen_size("error-bar-warning"), 1.0, &tokens, |ui| {
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
            shell::error_bar::ui(ui, &mut s)
        });
    });
    assert_fidelity!(r, "error-bar-warning", &tokens, &ctx);
}

#[test]
fn dialog_message() {
    dialog_screen(
        "dialog-message",
        DialogKind::Message {
            title: "Clip not found".into(),
            message: "The clip is no longer on disk.".into(),
        },
    );
}

#[test]
fn dialog_question() {
    dialog_screen(
        "dialog-question",
        DialogKind::Question {
            title: "Delete clip".into(),
            message: "Delete Overwatch_clip.mp4?\nThis cannot be undone.".into(),
            tag: "t".into(),
        },
    );
}

#[test]
fn dialog_input() {
    dialog_screen(
        "dialog-input",
        DialogKind::Input {
            title: "Rename clip".into(),
            label: "New name".into(),
            text: "Overwatch_clip".into(),
            tag: "rename".into(),
        },
    );
}

#[test]
fn custom_clip_length() {
    dialog_screen(
        "custom-clip-length",
        DialogKind::CustomClipLength { seconds: 30 },
    );
}

#[test]
fn export_preset() {
    dialog_screen("export-preset", DialogKind::ExportPreset { preset: None });
}

#[test]
#[ignore = "plan 5 (library): clip cards and the small library layout"]
fn library_small() {}

#[test]
#[ignore = "plan 5 (library): clip-card, clip-card-hover, clip-card-feedback"]
fn clip_cards() {}

#[test]
#[ignore = "plan 5 (library): record-hover toolbar state"]
fn record_hover() {}

#[test]
#[ignore = "plan 6 (editor): clip-editor"]
fn clip_editor() {}
