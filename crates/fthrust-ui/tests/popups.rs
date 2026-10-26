mod common;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::popups::{capture_settings, game_detection, hotkeys, source};
use fthrust_ui::state::AppState;
use fthrust_ui::tokens::Color;
use fthrust_ui::widgets::popup;

fn state(settings: serde_json::Value) -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), settings);
    s.tokens.reduce_motion = true;
    s.hotkey_socket_path = "/run/user/1000/fthr/hotkey.sock".into();
    s.client_exe = "/opt/fthrust/bin/xxxxxxxxxxxxxxxxxxxxx/fthrust".into();
    s.compositor = "unknown".into();
    (tmp, s)
}

fn ctx_for(tokens: &fthrust_ui::tokens::Tokens) -> egui::Context {
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, tokens);
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    ctx
}

fn render_popup(
    s: &mut AppState,
    size: Vec2,
    name: &str,
    width: f32,
    mut content: impl FnMut(&mut egui::Ui, &mut AppState),
) -> common::Rendered {
    let tokens = s.tokens.clone();
    common::render(size, 1.0, &tokens, |ui| {
        let t = s.tokens.clone();
        popup::draw_frame(ui, &t, name, Pos2::ZERO, width, |ui| content(ui, s));
    })
}

#[test]
fn capture_popup_markers_match_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let r = render_popup(
        &mut s,
        Vec2::new(460.0, 363.0),
        "capture",
        460.0,
        |ui, s| {
            capture_settings::content(ui, s);
        },
    );
    let frame = r.marker("capture").unwrap();
    // Qt's popup is 363 px tall; the "Replay buffer" row and its note (an
    // FTHRust addition below every Qt widget) add 116 px.
    assert_eq!(
        frame.rect,
        Rect::from_min_size(Pos2::ZERO, Vec2::new(460.0, 479.0)),
        "popup grows to its content"
    );
    assert_eq!(
        r.marker("QToolButton[Clips]").unwrap().rect,
        Rect::from_min_size(Pos2::new(21.0, 17.0), Vec2::new(205.0, 30.0))
    );
    assert_eq!(
        r.marker("QLabel[Clip length]").unwrap().rect,
        Rect::from_min_size(Pos2::new(21.0, 59.0), Vec2::new(92.0, 38.0))
    );
    assert_eq!(
        r.marker("_DropdownCombo[30s]").unwrap().rect,
        Rect::from_min_size(Pos2::new(129.0, 59.0), Vec2::new(310.0, 38.0))
    );
    assert_eq!(
        r.marker("fthrNumberSpin").unwrap().rect,
        Rect::from_min_size(Pos2::new(129.0, 312.0), Vec2::new(310.0, 34.0))
    );
    assert_eq!(
        r.pixel(Pos2::new(120.0, 76.0)),
        Some(tokens.color(Color::Surface2)),
        "popup background"
    );
    assert_eq!(
        r.pixel(Pos2::new(0.0, 180.0)),
        Some(tokens.color(Color::BorderHi))
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "capture-popup", &tokens, &ctx);
}

#[test]
fn clip_length_outside_list_is_shown_and_kept() {
    let (_tmp, mut s) = state(serde_json::json!({"framerate": 75, "clip_length": 42}));
    let r = render_popup(
        &mut s,
        Vec2::new(460.0, 400.0),
        "capture",
        460.0,
        |ui, s| {
            capture_settings::content(ui, s);
        },
    );
    assert!(
        r.marker("_DropdownCombo[75]").is_some(),
        "stored fps shown as its own entry"
    );
    assert!(r.marker("_DropdownCombo[42s]").is_some());
    assert_eq!(
        s.settings.get_i64("framerate"),
        Some(75),
        "settings untouched until the user changes it"
    );
    assert_eq!(s.settings.get_i64("clip_length"), Some(42));
    assert!(!s.restart_pending);
}

#[test]
fn apply_appears_only_for_restart_keys() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let r = render_popup(
        &mut s,
        Vec2::new(460.0, 420.0),
        "capture",
        460.0,
        |ui, s| {
            capture_settings::content(ui, s);
        },
    );
    assert!(r.marker("QPushButton[Apply]").is_none());
    // Change the quality through the combo: open the list and pick "Custom".
    let mut h = common::harness(Vec2::new(460.0, 500.0), 1.0, &tokens, |ui| {
        let t = s.tokens.clone();
        popup::draw_frame(ui, &t, "capture", Pos2::ZERO, 460.0, |ui| {
            capture_settings::content(ui, &mut s)
        });
    });
    let combo = Pos2::new(284.0, 281.0);
    h.drag_at(combo);
    h.step();
    h.drop_at(combo);
    h.run_steps(3);
    let item = Pos2::new(284.0, 300.0 + 26.0 * 3.0 + 13.0);
    h.drag_at(item);
    h.step();
    h.drop_at(item);
    h.run_steps(3);
    drop(h);
    assert_eq!(s.settings.get_str("bitrate_level"), Some("custom"));
    assert!(s.restart_pending, "quality changes need an engine restart");
    let r = render_popup(
        &mut s,
        Vec2::new(460.0, 460.0),
        "capture",
        460.0,
        |ui, s| {
            capture_settings::content(ui, s);
        },
    );
    assert!(
        r.marker("QPushButton[Apply]").is_some(),
        "Apply shows once a restart is pending"
    );
    assert!(r.marker("fthrNumberSpin").is_some());
}

#[test]
fn source_popup_markers_match_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    s.monitors = vec![fthrust_ui::state::MonitorInfo {
        name: "DP-1".into(),
        width: 2560,
        height: 1400,
        refresh_hz: 165,
    }];
    let tokens = s.tokens.clone();
    let r = render_popup(&mut s, Vec2::new(300.0, 148.0), "source", 300.0, |ui, s| {
        source::content(ui, s);
    });
    assert_eq!(r.marker("source").unwrap().rect.height(), 148.0);
    assert_eq!(
        r.marker("_DropdownCombo[Desktop]").unwrap().rect,
        Rect::from_min_size(Pos2::new(17.0, 45.0), Vec2::new(266.0, 38.0))
    );
    assert_eq!(
        r.marker("_DropdownCombo[First Screen (Default)]")
            .unwrap()
            .rect
            .min,
        Pos2::new(17.0, 95.0)
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "source-popup", &tokens, &ctx);
    assert_eq!(
        source::monitor_label("DP-1", 2560, 1400, 165),
        "DP-1  (2560×1400 @ 165Hz)"
    );
}

#[test]
fn game_detection_popup_markers_match_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    s.running_windows = vec!["Steam · Steam".into()];
    let tokens = s.tokens.clone();
    let r = render_popup(&mut s, Vec2::new(533.0, 403.0), "game", 533.0, |ui, s| {
        game_detection::content(ui, s);
    });
    assert_eq!(r.marker("game").unwrap().rect.height(), 403.0);
    assert_eq!(
        r.marker("QToolButton[AUTO-SWITCH]").unwrap().rect,
        Rect::from_min_size(Pos2::new(19.0, 92.0), Vec2::new(244.0, 30.0))
    );
    assert_eq!(
        r.marker("QPushButton[Block detection]").unwrap().rect,
        Rect::from_min_size(Pos2::new(357.0, 261.0), Vec2::new(157.0, 41.0))
    );
    assert_eq!(
        r.marker("QLabel[No blocked applications.]")
            .unwrap()
            .rect
            .min,
        Pos2::new(19.0, 363.0)
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "game-detection-popup", &tokens, &ctx);
}

#[test]
fn hotkey_popup_markers_match_qt_and_rows_follow_custom_lengths() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let r = render_popup(
        &mut s,
        Vec2::new(550.0, 526.0),
        "hotkeys",
        550.0,
        |ui, s| {
            hotkeys::content(ui, s);
        },
    );
    assert_eq!(r.marker("hotkeys").unwrap().rect.height(), 526.0);
    assert_eq!(
        r.marker("hotkeyActionRow").unwrap().rect,
        Rect::from_min_size(Pos2::new(19.0, 62.0), Vec2::new(512.0, 79.0))
    );
    assert_eq!(
        r.marker("hotkeyActionRow#2").unwrap().rect.height(),
        78.0,
        "rows share 314 px with cumulative rounding"
    );
    assert_eq!(
        r.marker("_KeyboardBindingButton[F9]").unwrap().rect,
        Rect::from_min_size(Pos2::new(148.0, 100.0), Vec2::new(127.0, 30.0))
    );
    assert_eq!(
        r.marker("QPushButton[Clear]").unwrap().rect.size(),
        Vec2::new(46.0, 30.0)
    );
    assert_eq!(
        r.marker("QLabel[HOTKEYS]").unwrap().rect.height(),
        35.0,
        "Qt hands the wrapped-label slack to the title"
    );
    let instr = r
        .marker("QLabel[Bind your preferred keys to these comman]")
        .unwrap();
    assert_eq!(
        instr.rect,
        Rect::from_min_size(Pos2::new(19.0, 418.0), Vec2::new(512.0, 91.0))
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "hotkey-popup", &tokens, &ctx);
    // Custom lengths add CLIP rows after CAPTURE CLIP.
    s.set_setting("custom_clip_lengths", serde_json::json!([90, 15]));
    s.hotkeys.sync_custom_clips(&[90, 15]);
    let r = render_popup(
        &mut s,
        Vec2::new(550.0, 700.0),
        "hotkeys",
        550.0,
        |ui, s| {
            hotkeys::content(ui, s);
        },
    );
    assert!(r.marker("QLabel[CLIP 15s]").is_some());
    assert!(r.marker("QLabel[CLIP 1.5m]").is_some());
    let labels: Vec<f32> = [
        "QLabel[CAPTURE CLIP]",
        "QLabel[CLIP 15s]",
        "QLabel[CLIP 1.5m]",
        "QLabel[SCREENSHOT]",
    ]
    .iter()
    .map(|n| r.marker(n).unwrap().rect.min.y)
    .collect();
    assert!(labels.windows(2).all(|w| w[0] < w[1]), "{labels:?}");
}

#[test]
fn hotkey_recording_commits_a_chord_and_clear_unsets() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(550.0, 600.0), 1.0, &tokens, |ui| {
        let t = s.tokens.clone();
        popup::draw_frame(ui, &t, "hotkeys", Pos2::ZERO, 550.0, |ui| {
            hotkeys::content(ui, &mut s)
        });
    });
    let f9 = Pos2::new(211.0, 115.0);
    h.drag_at(f9);
    h.step();
    h.drop_at(f9);
    h.run_steps(2);
    h.key_press_modifiers(
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        egui::Key::F10,
    );
    h.run_steps(2);
    let clear = Pos2::new(306.0, 115.0);
    h.drag_at(Pos2::new(211.0, 203.0));
    h.step();
    h.drop_at(Pos2::new(211.0, 203.0));
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    let _ = clear;
    drop(h);
    assert_eq!(s.hotkeys.get("save_clip"), Some("Ctrl+Shift+F10"));
    assert_eq!(
        s.hotkeys.get("save_screenshot"),
        Some("F12"),
        "Escape cancels without changing the binding"
    );
    assert!(s.hotkeys_changed);
    assert_eq!(hotkeys::display_combo(""), "Unset");
    assert_eq!(hotkeys::display_combo("ctrl+f9"), "Ctrl+F9");
    assert!(hotkeys::setup_instructions("unknown", "/usr/bin/fthrust", "/run/user/1000/fthrust/hotkey.sock").starts_with("Bind your preferred keys to these commands:\n\n\"/usr/bin/fthrust\" --hotkey save_clip\n"));
    assert!(
        hotkeys::setup_instructions("kwin", "fthrust", "/s").starts_with("KDE: System Settings")
    );
    assert!(
        hotkeys::setup_instructions("gnome", "fthrust", "/s")
            .contains("bash -c '\"fthrust\" --hotkey save_clip'")
    );
}
