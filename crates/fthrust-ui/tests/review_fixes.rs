//! Tests for the findings of the plan-4 final review (one per fix).

mod common;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::fonts::Weight;
use fthrust_ui::shell::error_bar::{Action, Level};
use fthrust_ui::state::{AppState, DialogKind, Page, Tab};
use fthrust_ui::tokens::Color;
use fthrust_ui::widgets::combo::Combo;
use fthrust_ui::widgets::popup::{Panel, PanelState};
use fthrust_ui::widgets::section;

fn state(settings_json: serde_json::Value) -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), settings_json);
    s.tokens.reduce_motion = true;
    (tmp, s)
}

fn markers_after(h: &mut egui_kittest::Harness<'_>) -> Vec<fthrust_ui::fidelity::Marker> {
    h.run_steps(2);
    let m = fthrust_ui::fidelity::take(&h.ctx);
    if m.is_empty() {
        h.step();
        fthrust_ui::fidelity::take(&h.ctx)
    } else {
        m
    }
}

fn click(h: &mut egui_kittest::Harness<'_>, at: Pos2) {
    h.hover_at(at);
    h.step();
    h.drag_at(at);
    h.step();
    h.drop_at(at);
    h.run_steps(2);
}

// 1. Marker names slice on char boundaries.
#[test]
fn marker_names_truncate_on_char_boundary() {
    let t = fthrust_ui::tokens::Tokens::default_palette();
    let text = "é".repeat(45);
    let r = common::render(Vec2::new(400.0, 100.0), 1.0, &t, |ui| {
        section::label_wide(ui, &t, &text, 15.0, Weight::W400, Color::Text, None);
        section::label_wrapped(ui, &t, &text, 15.0, Weight::W400, Color::Text, 380.0, 0.0);
        fthrust_ui::widgets::dialog::message_body(ui, &t, &text, 380.0);
    });
    let expected = format!("QLabel[{}]", "é".repeat(40));
    assert!(
        r.markers.iter().any(|m| m.name == expected),
        "{:?}",
        r.markers.iter().map(|m| m.name.clone()).collect::<Vec<_>>()
    );
    assert_eq!(
        fthrust_ui::fidelity::marker_label(&"日本語テキスト".repeat(10))
            .chars()
            .count(),
        40
    );
}

// 2. A corrupt theme font is skipped instead of panicking.
#[test]
fn corrupt_theme_font_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let bad = tmp.path().join("Bad.ttf");
    std::fs::write(&bad, b"this is not a font file at all, just bytes").unwrap();
    let mut t = fthrust_ui::tokens::Tokens::default_palette();
    t.font_files.push(("Bad".into(), bad));
    t.body_family = "Bad".into();
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, &t);
    let mut out = ctx.run_pass(egui::RawInput::default(), |ctx| {
        let _ = ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                "hello".into(),
                fthrust_ui::fonts::id(15.0, Weight::W400),
                egui::Color32::WHITE,
            )
        });
    });
    out.textures_delta.clear();
    assert!(fthrust_ui::fonts::is_font_file(
        &std::fs::read(fthrust_ui::assets_dir().join("fonts/Oswald-Regular.ttf")).unwrap()
    ));
    assert!(!fthrust_ui::fonts::is_font_file(b"nope"));
}

// 3. A combo list hanging below a popup panel keeps the panel open.
#[test]
fn combo_list_below_popup_keeps_panel_open() {
    let t = {
        let mut t = fthrust_ui::tokens::Tokens::default_palette();
        t.reduce_motion = true;
        t
    };
    let mut icons = fthrust_ui::icons::IconCache::default();
    let mut panel = PanelState::default();
    panel.show();
    let mut selected = 0usize;
    let anchor = Rect::from_min_size(Pos2::new(20.0, 0.0), Vec2::new(100.0, 30.0));
    let mut h = common::harness(Vec2::new(500.0, 600.0), 1.0, &t, |ui| {
        let tt = t.clone();
        Panel::new("p", anchor, 300.0).show(ui, &tt, &mut panel, |ui| {
            let mut combo = Combo::new((0..8).map(|i| format!("Item {i}")).collect(), selected)
                .with_width(260.0);
            if combo.ui(ui, &tt, &mut icons, "combo").changed() {
                selected = combo.selected;
            }
        });
    });
    let m = markers_after(&mut h);
    let combo_rect = m.iter().find(|m| m.name == "combo").unwrap().rect;
    click(&mut h, combo_rect.center());
    let m = markers_after(&mut h);
    let panel_rect = m.iter().find(|m| m.name == "p").unwrap().rect;
    let item = m
        .iter()
        .find(|m| m.name == "combo/item[Item 6]")
        .unwrap()
        .rect;
    assert!(
        item.min.y > panel_rect.max.y,
        "item 6 hangs below the panel: {item:?} vs {panel_rect:?}"
    );
    click(&mut h, item.center());
    h.run_steps(2);
    drop(h);
    assert_eq!(selected, 6);
    assert!(panel.open, "panel stays open after picking from the list");
}

// 4. Playback device uses the reference keys and shows an unavailable entry.
#[test]
fn playback_device_uses_reference_keys() {
    let items = fthrust_ui::settings::audio::playback_items(
        "sink-x",
        "Speakers",
        &[("Headset".into(), "sink-y".into())],
    );
    assert_eq!(items[0].0, "System default");
    assert_eq!(items[1], ("Headset".into(), "sink-y".into()));
    assert_eq!(
        items[2],
        (
            "Speakers (unavailable; using the default output)".into(),
            "sink-x".into()
        )
    );
    let (_tmp, mut s) = state(
        serde_json::json!({"playback_device_id": "sink-x", "playback_device_name": "Speakers"}),
    );
    s.page = Page::Settings;
    s.settings_tab = Tab::Audio;
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        fthrust_ui::shell::frame(ui, &mut s);
    });
    assert!(
        r.marker("QComboBox[Speakers (unavailable; using the default output)]")
            .is_some()
    );
    assert_eq!(
        fthrust_ui::settings::audio::PLAYBACK_ID_KEY,
        "playback_device_id"
    );
    assert_eq!(
        fthrust_ui::settings::audio::PLAYBACK_NAME_KEY,
        "playback_device_name"
    );
}

// 5. Shift+arrows only cycle tabs when nothing else owns the keyboard.
#[test]
fn shift_arrows_respect_recording_focus_and_modals() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    s.page = Page::Settings;
    s.settings_tab = Tab::Shortcuts;
    s.shortcut_recording = Some("switch_panel".into());
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        fthrust_ui::shell::frame(ui, &mut s);
    });
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowRight);
    h.run_steps(2);
    drop(h);
    assert_eq!(
        s.settings_tab,
        Tab::Shortcuts,
        "recording a binding owns the keys"
    );
    s.shortcut_recording = None;
    s.dialogs.push_modal(DialogKind::Message {
        title: "x".into(),
        message: "y".into(),
    });
    s.dialogs.modal.push_back(DialogKind::Question {
        title: "q".into(),
        message: "m".into(),
        tag: "t".into(),
    });
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        fthrust_ui::shell::frame(ui, &mut s);
    });
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowRight);
    h.run_steps(2);
    drop(h);
    assert_eq!(s.settings_tab, Tab::Shortcuts, "a modal owns the keys");
    s.dialogs.modal.clear();
    s.shortcut_recording = Some("switch_panel".into());
    s.settings_tab = Tab::General;
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        fthrust_ui::shell::frame(ui, &mut s);
    });
    h.run_steps(2);
    drop(h);
    assert_eq!(
        s.shortcut_recording, None,
        "leaving the Shortcuts tab stops recording"
    );
}

// 6. Audio changes restart immediately; the Source popup has its own Apply.
#[test]
fn audio_change_requests_restart_and_source_popup_has_apply() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    s.page = Page::Settings;
    s.settings_tab = Tab::Audio;
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 1100.0), 1.0, &tokens, |ui| {
        fthrust_ui::shell::frame(ui, &mut s);
    });
    let m = markers_after(&mut h);
    let cb = m
        .iter()
        .find(|m| m.name == "QCheckBox[Enable audio capture]")
        .unwrap()
        .rect;
    click(&mut h, Pos2::new(cb.min.x + 10.0, cb.center().y));
    drop(h);
    assert!(
        s.engine_restart_requested,
        "audio changes restart the engine like the reference"
    );
    assert!(
        !s.restart_pending,
        "the capture popup's Apply is not involved"
    );
    // Source popup: change the mode → its own Apply appears; clicking it restarts.
    s.engine_restart_requested = false;
    s.popups.source.show();
    let mut h = common::harness(Vec2::new(700.0, 400.0), 1.0, &tokens, |ui| {
        let t = s.tokens.clone();
        fthrust_ui::widgets::popup::draw_frame(ui, &t, "source", Pos2::ZERO, 300.0, |ui| {
            fthrust_ui::popups::source::content(ui, &mut s)
        });
    });
    let m = markers_after(&mut h);
    assert!(
        m.iter().all(|m| m.name != "QPushButton[Apply]"),
        "no Apply before a change"
    );
    let mode = m
        .iter()
        .find(|m| m.name == "_DropdownCombo[Desktop]")
        .unwrap()
        .rect;
    click(&mut h, mode.center());
    let m = markers_after(&mut h);
    let item = m
        .iter()
        .find(|m| m.name == "_DropdownCombo[Desktop]/item[Window / Game]")
        .unwrap()
        .rect;
    click(&mut h, item.center());
    let m = markers_after(&mut h);
    let apply = m
        .iter()
        .find(|m| m.name == "QPushButton[Apply]")
        .expect("Apply row after a source change")
        .rect;
    click(&mut h, apply.center());
    drop(h);
    assert!(!s.restart_pending, "the capture popup's flag stays clear");
    assert!(s.engine_restart_requested);
    assert!(!s.source_restart_pending);
}

// 7. The Source popup persists the window choice.
#[test]
fn source_window_choice_persisted_and_cleared_on_desktop() {
    let (_tmp, mut s) = state(serde_json::json!({"capture_mode": "window"}));
    s.running_windows = vec!["Game A".into(), "Game B".into()];
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(700.0, 500.0), 1.0, &tokens, |ui| {
        let t = s.tokens.clone();
        fthrust_ui::widgets::popup::draw_frame(ui, &t, "source", Pos2::ZERO, 300.0, |ui| {
            fthrust_ui::popups::source::content(ui, &mut s)
        });
    });
    let m = markers_after(&mut h);
    let wc = m
        .iter()
        .find(|m| m.name == "_DropdownCombo[Game A]")
        .unwrap()
        .rect;
    click(&mut h, wc.center());
    let m = markers_after(&mut h);
    let item = m
        .iter()
        .find(|m| m.name == "_DropdownCombo[Game A]/item[Game B]")
        .unwrap()
        .rect;
    click(&mut h, item.center());
    drop(h);
    assert_eq!(s.settings.get_str("target_window_name"), Some("Game B"));
    let mut h = common::harness(Vec2::new(700.0, 500.0), 1.0, &tokens, |ui| {
        let t = s.tokens.clone();
        fthrust_ui::widgets::popup::draw_frame(ui, &t, "source", Pos2::ZERO, 300.0, |ui| {
            fthrust_ui::popups::source::content(ui, &mut s)
        });
    });
    let m = markers_after(&mut h);
    let mode = m
        .iter()
        .find(|m| m.name == "_DropdownCombo[Window / Game]")
        .unwrap()
        .rect;
    click(&mut h, mode.center());
    let m = markers_after(&mut h);
    let item = m
        .iter()
        .find(|m| m.name == "_DropdownCombo[Window / Game]/item[Desktop]")
        .unwrap()
        .rect;
    click(&mut h, item.center());
    drop(h);
    assert_eq!(s.settings.get_str("capture_mode"), Some("desktop"));
    assert!(s.source_restart_pending);
    assert_eq!(s.settings.get_str("target_window_name"), Some(""));
    assert_eq!(s.settings.get_i64("target_hwnd"), Some(0));
}

// 8. Game rules are normalised and de-duplicated.
#[test]
fn game_rule_add_deduplicates() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    assert!(fthrust_ui::popups::game_detection::add_rule(
        &mut s,
        "Overwatch",
        false
    ));
    assert!(!fthrust_ui::popups::game_detection::add_rule(
        &mut s,
        "Overwatch",
        false
    ));
    assert!(fthrust_ui::popups::game_detection::add_rule(
        &mut s,
        "Overwatch",
        true
    ));
    assert!(!fthrust_ui::popups::game_detection::add_rule(
        &mut s, "  ", true
    ));
    let rules = s
        .settings
        .get("game_detection_custom_games")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["title_contains"], "Overwatch");
    assert_eq!(
        s.settings
            .get("game_detection_blacklist")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

// 9. A system CJK font is a fallback for every family.
#[test]
fn cjk_fallback_font_registered() {
    let Some(path) = fthrust_ui::fonts::cjk_fallback_path() else {
        eprintln!("no CJK font on this machine; skipped");
        return;
    };
    assert!(path.exists());
    let t = fthrust_ui::tokens::Tokens::default_palette();
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, &t);
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let ok = ctx.fonts_mut(|f| f.has_glyphs(&fthrust_ui::fonts::id(15.0, Weight::W600), "日本語"));
    assert!(ok, "CJK glyphs resolve through the fallback");
}

// 10. Restored size is clamped to the monitor; persisting keeps position and screen.
#[test]
fn restored_size_clamped_and_position_kept() {
    use fthrust_core::window_geometry::Rect as GRect;
    let clamped = fthrust_ui::shell::window::clamp_to_monitor(
        GRect {
            x: 10,
            y: 20,
            width: 3840,
            height: 2160,
        },
        Vec2::new(1920.0, 1080.0),
    );
    assert_eq!(
        clamped,
        GRect {
            x: 10,
            y: 20,
            width: 1920,
            height: 1080
        }
    );
    let same = fthrust_ui::shell::window::clamp_to_monitor(
        GRect {
            x: 10,
            y: 20,
            width: 1200,
            height: 800,
        },
        Vec2::new(1920.0, 1080.0),
    );
    assert_eq!(same.width, 1200);
    let (_tmp, mut s) = state(
        serde_json::json!({"main_window_geometry": {"normal_geometry": {"x": 100, "y": 50, "width": 1300, "height": 800}, "screen_name": "DP-1", "maximized": false}}),
    );
    fthrust_ui::shell::window::persist_geometry(&mut s.settings, (1400, 900), true);
    let saved = fthrust_ui::shell::window::saved_geometry(&s.settings);
    let g = saved.normal_geometry.unwrap();
    assert_eq!((g.x, g.y, g.width, g.height), (100, 50, 1400, 900));
    assert_eq!(saved.screen_name.as_deref(), Some("DP-1"));
    assert_eq!(saved.maximized, Some(Some(true)));
}

// 11. Error-bar actions reach the state.
#[test]
fn error_bar_action_reaches_state() {
    let (_tmp, mut s) = state(serde_json::json!({}));
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
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        fthrust_ui::shell::ui(ui, &mut s);
    });
    let m = markers_after(&mut h);
    let btn = m
        .iter()
        .find(|m| m.name == "QPushButton[RESTART ENGINE]")
        .unwrap()
        .rect;
    click(&mut h, btn.center());
    drop(h);
    assert_eq!(s.error_action_requested.as_deref(), Some("restart"));
}

// 12. Only the topmost dialog consumes Enter and Escape.
#[test]
fn dialog_keys_go_to_the_topmost_dialog_only() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    s.dialogs
        .push_modal(DialogKind::ExportPreset { preset: None });
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(900.0, 700.0), 1.0, &tokens, |ui| {
        fthrust_ui::dialogs::draw_all(ui, &mut s);
    });
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    drop(h);
    assert_eq!(s.dialogs.modal.len(), 1, "the invalid preset stays open");
    assert_eq!(
        s.dialogs.messages.len(),
        1,
        "the validation message is still shown"
    );
    // Escape closes the message first, then the modal.
    let mut h = common::harness(Vec2::new(900.0, 700.0), 1.0, &tokens, |ui| {
        fthrust_ui::dialogs::draw_all(ui, &mut s);
    });
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    drop(h);
    assert_eq!(s.dialogs.messages.len(), 0);
    assert_eq!(s.dialogs.modal.len(), 1, "one Escape closes one dialog");
}
