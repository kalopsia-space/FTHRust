mod common;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::dialogs;
use fthrust_ui::state::{AppState, DialogKind, DialogResult};
use fthrust_ui::tokens::Color;

fn state(settings_json: serde_json::Value) -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), settings_json);
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

fn render_dialog(s: &mut AppState, size: Vec2, kind: &DialogKind) -> common::Rendered {
    let tokens = s.tokens.clone();
    common::render(size, 1.0, &tokens, |ui| {
        dialogs::draw_at(ui, s, kind, Pos2::ZERO);
    })
}

#[test]
fn message_and_question_dialogs_match_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let kind = DialogKind::Message {
        title: "Clip not found".into(),
        message: "The clip is no longer on disk.".into(),
    };
    let r = render_dialog(&mut s, Vec2::new(340.0, 177.0), &kind);
    assert_eq!(
        r.marker("fthrDialog").unwrap().rect,
        Rect::from_min_size(Pos2::ZERO, Vec2::new(340.0, 177.0))
    );
    assert_eq!(r.marker("QPushButton[OK]").unwrap().rect.min.y, 122.0);
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "dialog-message", &tokens, &ctx);
    let kind = DialogKind::Question {
        title: "Delete clip".into(),
        message: "Delete Overwatch_clip.mp4?\nThis cannot be undone.".into(),
        tag: "t".into(),
    };
    let r = render_dialog(&mut s, Vec2::new(360.0, 177.0), &kind);
    assert_eq!(
        r.marker("QPushButton[No]").unwrap().rect.min,
        Pos2::new(232.0, 122.0)
    );
    assert_eq!(
        r.marker("QPushButton[Yes]").unwrap().rect.min,
        Pos2::new(289.0, 122.0)
    );
    assert_fidelity!(r, "dialog-question", &tokens, &ctx);
}

#[test]
fn input_dialog_matches_qt_and_enter_accepts() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let kind = DialogKind::Input {
        title: "Rename clip".into(),
        label: "New name".into(),
        text: "Overwatch_clip".into(),
        tag: "rename".into(),
    };
    let r = render_dialog(&mut s, Vec2::new(360.0, 196.0), &kind);
    assert_eq!(
        r.marker("fthrDialogInput").unwrap().rect,
        Rect::from_min_size(Pos2::new(16.0, 85.0), Vec2::new(328.0, 40.0))
    );
    assert_eq!(
        r.marker("QPushButton[Cancel]").unwrap().rect.min,
        Pos2::new(212.0, 141.0)
    );
    assert_eq!(
        r.pixel(Pos2::new(20.0, 105.0)),
        Some(tokens.color(Color::Surface1))
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "dialog-input", &tokens, &ctx);
    // Through the queue: the field has focus; Enter accepts with the edited text.
    s.dialogs.push_modal(kind.clone());
    let mut h = common::harness(Vec2::new(800.0, 600.0), 1.0, &tokens, |ui| {
        dialogs::draw_all(ui, &mut s);
    });
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    drop(h);
    let results = s.dialogs.take_results();
    assert_eq!(
        results,
        vec![DialogResult::Input {
            tag: "rename".into(),
            text: Some("Overwatch_clip".into())
        }]
    );
    assert!(s.dialogs.modal.is_empty());
}

#[test]
fn question_escape_answers_no_and_yes_click_answers_yes() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    s.dialogs.push_modal(DialogKind::Question {
        title: "Delete clip".into(),
        message: "Sure?".into(),
        tag: "del".into(),
    });
    let mut h = common::harness(Vec2::new(800.0, 600.0), 1.0, &tokens, |ui| {
        dialogs::draw_all(ui, &mut s);
    });
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    drop(h);
    assert_eq!(
        s.dialogs.take_results(),
        vec![DialogResult::Answer {
            tag: "del".into(),
            yes: false
        }]
    );
    s.dialogs.push_modal(DialogKind::Question {
        title: "Delete clip".into(),
        message: "Sure?".into(),
        tag: "del2".into(),
    });
    let mut h = common::harness(Vec2::new(800.0, 600.0), 1.0, &tokens, |ui| {
        dialogs::draw_all(ui, &mut s);
    });
    h.run_steps(2);
    let markers = fthrust_ui::fidelity::take(&h.ctx);
    h.step();
    let markers = if markers.is_empty() {
        fthrust_ui::fidelity::take(&h.ctx)
    } else {
        markers
    };
    let yes = markers
        .iter()
        .find(|m| m.name == "QPushButton[Yes]")
        .unwrap()
        .rect
        .center();
    h.hover_at(yes);
    h.step();
    h.drag_at(yes);
    h.step();
    h.drop_at(yes);
    h.run_steps(2);
    drop(h);
    assert_eq!(
        s.dialogs.take_results(),
        vec![DialogResult::Answer {
            tag: "del2".into(),
            yes: true
        }]
    );
}

#[test]
fn custom_clip_length_matches_qt_and_validates() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let kind = DialogKind::CustomClipLength { seconds: 30 };
    let r = render_dialog(&mut s, Vec2::new(340.0, 187.0), &kind);
    assert_eq!(
        r.marker("fthrNumberSpin").unwrap().rect,
        Rect::from_min_size(Pos2::new(63.0, 54.0), Vec2::new(261.0, 34.0))
    );
    assert_eq!(
        r.marker("QLabel[Choose 1–15 minutes.]").unwrap().rect.min.y,
        98.0
    );
    assert_eq!(
        r.marker("QPushButton[Add length]").unwrap().rect.min,
        Pos2::new(227.0, 132.0)
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "custom-clip-length", &tokens, &ctx);
    assert!(dialogs::custom_clip_length::validate(0).is_err());
    assert!(dialogs::custom_clip_length::validate(901).is_err());
    assert_eq!(dialogs::custom_clip_length::validate(900).unwrap(), 900);
    assert!(
        dialogs::custom_clip_length::validate(0)
            .unwrap_err()
            .contains("normal clip length")
    );
    // Accepting writes the custom length and a hotkey row.
    s.dialogs
        .push_modal(DialogKind::CustomClipLength { seconds: 120 });
    let mut h = common::harness(Vec2::new(800.0, 600.0), 1.0, &tokens, |ui| {
        dialogs::draw_all(ui, &mut s);
    });
    h.run_steps(2);
    let markers = fthrust_ui::fidelity::take(&h.ctx);
    h.step();
    let markers = if markers.is_empty() {
        fthrust_ui::fidelity::take(&h.ctx)
    } else {
        markers
    };
    let add = markers
        .iter()
        .find(|m| m.name == "QPushButton[Add length]")
        .unwrap()
        .rect
        .center();
    h.hover_at(add);
    h.step();
    h.drag_at(add);
    h.step();
    h.drop_at(add);
    h.run_steps(2);
    drop(h);
    dialogs::dispatch_results(&mut s);
    assert_eq!(
        s.settings.get("custom_clip_lengths").unwrap(),
        &serde_json::json!([120])
    );
    assert!(s.hotkeys.custom_clip_lengths().contains(&120));
    assert!(s.restart_pending);
}

#[test]
fn export_preset_dialog_matches_qt_and_saves() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let kind = DialogKind::ExportPreset { preset: None };
    let r = render_dialog(&mut s, Vec2::new(420.0, 447.0), &kind);
    assert_eq!(r.marker("fthrDialog").unwrap().rect.height(), 447.0);
    assert_eq!(
        r.marker("presetStepper").unwrap().rect,
        Rect::from_min_size(Pos2::new(108.0, 102.0), Vec2::new(296.0, 34.0))
    );
    assert_eq!(
        r.marker("presetStepper#2").unwrap().rect,
        Rect::from_min_size(Pos2::new(108.0, 178.0), Vec2::new(118.0, 34.0))
    );
    assert_eq!(r.marker("presetStepper#3").unwrap().rect.min.x, 286.0);
    assert_eq!(
        r.marker("WheelSafeComboBox[H.264 (reviewed runtime)]")
            .unwrap()
            .rect
            .min
            .y,
        296.0
    );
    assert_eq!(
        r.marker("QPushButton[Save preset]").unwrap().rect.min,
        Pos2::new(303.0, 392.0)
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "export-preset", &tokens, &ctx);
    // Name it and save: a validated preset comes back.
    s.dialogs
        .push_modal(DialogKind::ExportPreset { preset: None });
    s.export_preset_draft = None;
    let mut h = common::harness(Vec2::new(900.0, 700.0), 1.0, &tokens, |ui| {
        dialogs::draw_all(ui, &mut s);
    });
    h.run_steps(2);
    let markers = fthrust_ui::fidelity::take(&h.ctx);
    h.step();
    let markers = if markers.is_empty() {
        fthrust_ui::fidelity::take(&h.ctx)
    } else {
        markers
    };
    let name_field = markers
        .iter()
        .find(|m| m.name == "presetName")
        .unwrap()
        .rect
        .center();
    h.hover_at(name_field);
    h.step();
    h.drag_at(name_field);
    h.step();
    h.drop_at(name_field);
    h.run_steps(2);
    for c in "Tiny".chars() {
        h.event(egui::Event::Text(c.to_string()));
        h.step();
    }
    let save = markers
        .iter()
        .find(|m| m.name == "QPushButton[Save preset]")
        .unwrap()
        .rect
        .center();
    h.hover_at(save);
    h.step();
    h.drag_at(save);
    h.step();
    h.drop_at(save);
    h.run_steps(2);
    drop(h);
    let results = s.dialogs.take_results();
    match &results[..] {
        [DialogResult::ExportPreset(Some(p))] => {
            assert_eq!(p.name, "Tiny");
            assert_eq!(p.target_size_mb, 25.0);
            assert_eq!(p.audio_bitrate_kbps, 128);
            assert!(p.target_width.is_none());
        }
        other => panic!("{other:?}"),
    }
}
