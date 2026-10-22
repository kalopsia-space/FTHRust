mod common;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::settings::{self, general};
use fthrust_ui::shell;
use fthrust_ui::state::{AppState, Page, StatusStyle, StatusText, Tab};
use fthrust_ui::tokens::Color;

fn state(settings_json: serde_json::Value) -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), settings_json);
    s.tokens.reduce_motion = true;
    s.page = Page::Settings;
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
fn settings_frame_and_general_tab_match_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    assert_eq!(
        r.marker("settingsTabBar").unwrap().rect,
        Rect::from_min_size(Pos2::new(94.0, 101.0), Vec2::new(1346.0, 90.0))
    );
    assert_eq!(
        r.marker("settingsDivider").unwrap().rect,
        Rect::from_min_size(Pos2::new(94.0, 191.0), Vec2::new(1346.0, 1.0))
    );
    assert_eq!(
        r.marker("settingsContent").unwrap().rect,
        Rect::from_min_size(Pos2::new(94.0, 192.0), Vec2::new(1346.0, 708.0))
    );
    let general = r.marker("settingsTabBtn[General]").unwrap();
    assert_eq!(
        general.rect,
        Rect::from_min_size(Pos2::new(364.0, 101.0), Vec2::new(115.0, 90.0))
    );
    assert_eq!(
        r.marker("settingsTabBtn[Shortcuts]").unwrap().rect.min.x,
        1054.0
    );
    assert_eq!(
        r.pixel(Pos2::new(421.0, 189.0)),
        Some(tokens.color(Color::Accent)),
        "2 px ACCENT underline on the checked tab"
    );
    assert_eq!(
        r.pixel(Pos2::new(536.0, 189.0)),
        Some(tokens.color(Color::ShellBg2)),
        "no underline elsewhere"
    );
    assert_eq!(
        r.pixel(Pos2::new(700.0, 191.0)),
        Some(tokens.color(Color::ShellDivider))
    );
    assert_eq!(
        r.marker("QLabel[System]").unwrap().rect,
        Rect::from_min_size(Pos2::new(118.0, 216.0), Vec2::new(58.0, 30.0))
    );
    assert_eq!(
        r.marker("QCheckBox[Show Ko-fi donation widget]")
            .unwrap()
            .rect,
        Rect::from_min_size(Pos2::new(118.0, 272.0), Vec2::new(1278.0, 26.0))
    );
    assert_eq!(
        r.marker("QComboBox[All notifications]").unwrap().rect,
        Rect::from_min_size(Pos2::new(118.0, 428.0), Vec2::new(1278.0, 38.0))
    );
    assert_eq!(
        r.marker("QPushButton[Support Catbox hosting]")
            .unwrap()
            .rect,
        Rect::from_min_size(Pos2::new(118.0, 574.0), Vec2::new(1266.0, 44.0))
    );
    assert_eq!(
        r.marker("QLabel[Export Presets]").unwrap().rect.min.y,
        674.0
    );
    let new_btn = r.marker("QPushButton[New]").unwrap().rect;
    assert!(
        (new_btn.min.x - 1194.0).abs() <= 1.0 && new_btn.min.y == 724.0,
        "{new_btn:?}"
    );
    assert_eq!(
        r.marker("QLabel[Troubleshooting]").unwrap().rect.min.y,
        820.0
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "settings", &tokens, &ctx);
}

#[test]
fn clip_tab_matches_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    s.settings_tab = Tab::Clip;
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    assert_eq!(
        r.marker("QLabel[Clip Settings]").unwrap().rect.min,
        Pos2::new(118.0, 216.0)
    );
    assert_eq!(
        r.marker("QLabel[CLIPS STORAGE FOLDER]").unwrap().rect,
        Rect::from_min_size(Pos2::new(118.0, 300.0), Vec2::new(180.0, 41.0))
    );
    assert_eq!(
        r.marker("QPushButton[USE DEFAULT]").unwrap().rect.min,
        Pos2::new(1290.0, 300.0)
    );
    assert_eq!(
        r.marker("QPushButton[BROWSE]#2").unwrap().rect.min,
        Pos2::new(1314.0, 349.0)
    );
    assert_eq!(
        r.marker("QPushButton[Scan for clips]").unwrap().rect,
        Rect::from_min_size(Pos2::new(218.0, 454.0), Vec2::new(115.0, 43.0))
    );
    assert_eq!(
        r.marker("QLabel[No import folders added yet.]")
            .unwrap()
            .rect
            .min
            .y,
        509.0
    );
    assert_eq!(
        r.marker("QLabel[Video Encoding]").unwrap().rect.min.y,
        579.0
    );
    assert_eq!(
        r.marker("_DropdownCombo[Detecting available encoders…]")
            .unwrap()
            .rect,
        Rect::from_min_size(Pos2::new(270.0, 621.0), Vec2::new(1126.0, 38.0))
    );
    assert_eq!(
        r.marker("QPushButton[Redetect]").unwrap().rect.min,
        Pos2::new(118.0, 754.0)
    );
    assert_eq!(
        r.marker("QCheckBox[Add animated Capture Card to exports")
            .unwrap()
            .rect
            .min
            .y,
        861.0
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "clip-settings", &tokens, &ctx);
}

#[test]
fn tab_clicks_and_shift_arrows_cycle() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    let clip_tab = Pos2::new(536.0, 146.0);
    h.drag_at(clip_tab);
    h.step();
    h.drop_at(clip_tab);
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowRight);
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowLeft);
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowLeft);
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowLeft);
    h.run_steps(2);
    drop(h);
    assert_eq!(
        s.settings_tab,
        Tab::Shortcuts,
        "Clip → Audio → Clip → General → Shortcuts (wraps)"
    );
    assert_eq!(Tab::General.cycle(-1), Tab::Shortcuts);
    assert_eq!(Tab::Shortcuts.cycle(1), Tab::General);
}

#[test]
fn export_presets_list_builtins_first_and_describe() {
    let (_tmp, s) = state(serde_json::json!({}));
    let store = fthrust_core::export_presets::ExportPresetStore::new(&s.paths);
    let mut custom = fthrust_core::export_presets::ExportPreset::builtin_discord();
    custom.preset_id = String::new();
    custom.name = "My 720p".into();
    custom.built_in = false;
    custom.target_width = Some(1280);
    custom.target_height = Some(720);
    custom.target_fps = Some(30.0);
    let saved = store.save(&custom).unwrap();
    let items = general::export_preset_items(&store);
    assert!(items[0].0.ends_with(" · Built-in"), "{items:?}");
    assert!(
        items
            .iter()
            .any(|(label, id)| label == "My 720p" && *id == saved.preset_id)
    );
    assert!(
        items
            .iter()
            .position(|(l, _)| l.ends_with("Built-in"))
            .unwrap()
            < items.iter().position(|(l, _)| l == "My 720p").unwrap()
    );
    assert_eq!(
        general::export_preset_detail(&saved),
        "10 MB target · 1280×720 max · 30 FPS max · AAC 96 kbps"
            .replace("10 MB", &format!("{} MB", saved.target_size_mb))
    );
    let discord = fthrust_core::export_presets::ExportPreset::builtin_discord();
    assert!(
        general::export_preset_detail(&discord)
            .starts_with(&format!("{} MB target", discord.target_size_mb))
    );
}

#[test]
fn settings_presets_save_load_delete_roundtrip() {
    let (_tmp, mut s) = state(serde_json::json!({"clip_length": 45, "framerate": 120}));
    assert_eq!(
        general::settings_preset_names(&s),
        vec!["— no presets —".to_owned()]
    );
    general::save_settings_preset(&mut s, "Fast");
    assert_eq!(general::settings_preset_names(&s), vec!["Fast".to_owned()]);
    s.set_setting("clip_length", serde_json::json!(15));
    assert!(general::load_settings_preset(&mut s, "Fast"));
    assert_eq!(s.settings.get_i64("clip_length"), Some(45));
    assert!(s.restart_pending);
    assert!(general::delete_settings_preset(&mut s, "Fast"));
    assert_eq!(
        general::settings_preset_names(&s),
        vec!["— no presets —".to_owned()]
    );
}

#[test]
fn clip_tab_writes_settings() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    s.settings_tab = Tab::Clip;
    s.settings.set(
        "imported_clip_folders",
        serde_json::json!(["/tmp/a", "/tmp/b"]),
    );
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 1100.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    h.step();
    let markers = fthrust_ui::fidelity::take(&h.ctx);
    let wm = markers
        .iter()
        .find(|m| m.name.starts_with("QCheckBox[Add animated"))
        .unwrap()
        .rect;
    let p = Pos2::new(wm.min.x + 7.0, wm.center().y);
    h.hover_at(p);
    h.step();
    h.hover_at(p);
    h.step();
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.run_steps(2);
    let remove = markers
        .iter()
        .find(|m| m.name == "importFolderRemove#1")
        .unwrap()
        .rect
        .center();
    h.hover_at(remove);
    h.step();
    h.hover_at(remove);
    h.step();
    h.drag_at(remove);
    h.step();
    h.drop_at(remove);
    h.run_steps(2);
    drop(h);
    assert_eq!(s.settings.get_bool("watermark_enabled"), Some(true));
    assert_eq!(
        s.settings.get("imported_clip_folders").unwrap(),
        &serde_json::json!(["/tmp/b"]),
        "row #1 is the first folder"
    );
    let _ = settings::CONTENT_INSET;
}
