mod common;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::settings::{performance, shortcuts};
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
fn customize_tab_matches_qt_at_four_widths() {
    for (w, h, screen) in [
        (1440.0, 900.0, "customize"),
        (1920.0, 1080.0, "customize-1920"),
        (2560.0, 1440.0, "customize-2560"),
        (3840.0, 2160.0, "customize-3840"),
    ] {
        let (_tmp, mut s) = state(serde_json::json!({}), Tab::Customize);
        let tokens = s.tokens.clone();
        let r = common::render(Vec2::new(w, h), 1.0, &tokens, |ui| {
            shell::frame(ui, &mut s);
        });
        assert_eq!(
            r.marker("QPushButton[Export theme]")
                .unwrap_or_else(|| panic!(
                    "no export button at {screen}: {:?}",
                    r.markers.iter().map(|m| m.name.clone()).collect::<Vec<_>>()
                ))
                .rect
                .min,
            Pos2::new(118.0, 216.0),
            "{screen}"
        );
        assert_eq!(
            r.marker("QPushButton[Apply theme]").unwrap().rect.max.x,
            w - 36.0,
            "{screen}"
        );
        assert_eq!(
            r.marker("accordionSection").unwrap().rect.min,
            Pos2::new(118.0, 330.0),
            "{screen}"
        );
        assert_eq!(
            r.marker("QLabel[Typography]").unwrap().rect.min,
            Pos2::new(163.0, 331.0),
            "{screen}"
        );
        assert_eq!(
            r.marker("accordionSection#6").unwrap().rect.min.y,
            620.0,
            "{screen}"
        );
        let ctx = ctx_for(&tokens);
        assert_fidelity!(r, screen, &tokens, &ctx);
    }
}

#[test]
fn colors_and_typography_sections_match_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}), Tab::Customize);
    s.customize_open[1] = true;
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    assert_eq!(
        r.marker("colorPreview").unwrap().rect,
        Rect::from_min_size(Pos2::new(135.0, 449.0), Vec2::new(1244.0, 220.0))
    );
    assert_eq!(
        r.marker("QLabel[Surfaces]").unwrap().rect.min,
        Pos2::new(135.0, 689.0)
    );
    assert_eq!(r.marker("QLabel[Accent]").unwrap().rect.min.y, 813.0);
    assert_eq!(
        r.marker("colorSwatch[BG]").unwrap().rect,
        Rect::from_min_size(Pos2::new(241.0, 689.0), Vec2::new(96.0, 52.0))
    );
    assert_eq!(
        r.pixel(Pos2::new(289.0, 704.0)),
        Some(tokens.color(Color::Bg)),
        "BG swatch shows the token colour"
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "colors", &tokens, &ctx);
    s.customize_open[1] = false;
    s.customize_open[0] = true;
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    assert_eq!(
        r.marker("WheelSafeComboBox[Oswald]").unwrap().rect,
        Rect::from_min_size(Pos2::new(287.0, 427.0), Vec2::new(982.0, 38.0))
    );
    assert_eq!(
        r.marker("QPushButton[Import font]#2").unwrap().rect.min,
        Pos2::new(1281.0, 479.0)
    );
    assert_eq!(
        r.marker("QLabel[Your privacy isn't currency.]")
            .unwrap()
            .rect,
        Rect::from_min_size(Pos2::new(135.0, 532.0), Vec2::new(1244.0, 52.0))
    );
    assert_eq!(r.marker("accordionSection#2").unwrap().rect.min.y, 666.0);
    assert_fidelity!(r, "typography", &tokens, &ctx);
}

#[test]
fn color_edit_saves_draft_but_applies_on_apply() {
    let (_tmp, mut s) = state(serde_json::json!({}), Tab::Customize);
    let accent_before = s.tokens.color(Color::Accent);
    fthrust_ui::settings::customize::set_draft_color(&mut s, "ACCENT", "#ff0000");
    assert_eq!(
        s.theme.color("ACCENT"),
        "#ff0000",
        "draft saved to theme.json"
    );
    let reloaded = fthrust_core::theme::ThemeStore::load(s.paths.clone()).unwrap();
    assert_eq!(reloaded.color("ACCENT"), "#ff0000");
    assert_eq!(
        s.tokens.color(Color::Accent),
        accent_before,
        "visuals unchanged until Apply"
    );
    let ctx = ctx_for(&s.tokens.clone());
    fthrust_ui::settings::customize::apply_theme(&ctx, &mut s);
    assert_eq!(
        fthrust_ui::tokens::to_hex(s.tokens.color(Color::Accent)),
        "#ff0000"
    );
    fthrust_ui::settings::customize::reset_all(&ctx, &mut s);
    assert_eq!(s.theme.color("ACCENT"), "#20e3a0");
    assert_eq!(
        fthrust_ui::tokens::to_hex(s.tokens.color(Color::Accent)),
        "#20e3a0"
    );
}

#[test]
fn performance_tab_matches_qt_and_presets_resolve() {
    let (_tmp, mut s) = state(serde_json::json!({}), Tab::Performance);
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    assert_eq!(
        r.marker("_DropdownCombo[2 exports at once]").unwrap().rect,
        Rect::from_min_size(Pos2::new(118.0, 284.0), Vec2::new(1278.0, 38.0))
    );
    assert_eq!(
        r.marker("QPushButton[View export jobs]").unwrap().rect,
        Rect::from_min_size(Pos2::new(118.0, 368.0), Vec2::new(1278.0, 41.0))
    );
    assert_eq!(
        r.marker("_DropdownCombo[Full experience]")
            .unwrap()
            .rect
            .min
            .y,
        471.0
    );
    assert_eq!(
        r.marker("clip_editor_enabled_performance_check")
            .unwrap()
            .rect
            .min
            .y,
        544.0
    );
    assert_eq!(
        r.marker("settings_visual_previews_enabled_performance_check")
            .unwrap()
            .rect
            .min
            .y,
        849.0
    );
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "performance", &tokens, &ctx);
    assert_eq!(performance::matching_preset(&s), "Full experience");
    s.set_setting("reduce_ui_animations", serde_json::json!(true));
    s.set_setting("settings_visual_previews_enabled", serde_json::json!(false));
    s.set_setting("timeline_preview_strip_enabled", serde_json::json!(false));
    assert_eq!(performance::matching_preset(&s), "Balanced");
    s.set_setting("library_thumbnails_enabled", serde_json::json!(false));
    assert_eq!(performance::matching_preset(&s), "Custom");
    performance::apply_preset(&mut s, "Minimal / viewer only");
    assert_eq!(s.settings.get_bool("clip_editor_enabled"), Some(false));
    assert!(
        s.tokens.reduce_motion,
        "presets drive the reduce-motion flag live"
    );
}

#[test]
fn shortcuts_tab_matches_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}), Tab::Shortcuts);
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    assert_eq!(
        r.marker("QLabel[Beta]").unwrap().rect.min,
        Pos2::new(127.0, 225.0)
    );
    assert_eq!(r.marker("QLabel[Application]").unwrap().rect.min.y, 383.0);
    let beta = r.marker("QLabel[Beta]").unwrap().rect;
    assert_eq!(beta.width(), 1272.0, "page content width");
    assert_eq!(
        r.marker("BindingButton[Tab+S]").unwrap().rect,
        Rect::from_min_size(Pos2::new(1179.0, 447.0), Vec2::new(150.0, 41.0))
    );
    assert_eq!(
        r.marker("QPushButton[Clear]").unwrap().rect,
        Rect::from_min_size(Pos2::new(1335.0, 447.0), Vec2::new(64.0, 41.0))
    );
    assert_eq!(r.marker("QLabel[Clips]").unwrap().rect.min.y, 696.0);
    let ctx = ctx_for(&tokens);
    assert_fidelity!(r, "shortcuts", &tokens, &ctx);
}

#[test]
fn binding_conflict_keeps_old_binding_and_restore_resets() {
    let (_tmp, mut s) = state(serde_json::json!({}), Tab::Shortcuts);
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    h.step();
    let markers = fthrust_ui::fidelity::take(&h.ctx);
    let btn = markers
        .iter()
        .find(|m| m.name == "BindingButton[Shift+Down]")
        .unwrap()
        .rect
        .center();
    h.hover_at(btn);
    h.step();
    h.drag_at(btn);
    h.step();
    h.drop_at(btn);
    h.run_steps(2);
    // Tab+S is already used by "Switch Clips / Settings" in the same scope.
    h.key_down(egui::Key::Tab);
    h.step();
    h.key_down(egui::Key::S);
    h.step();
    h.key_up(egui::Key::S);
    h.step();
    h.key_up(egui::Key::Tab);
    h.run_steps(2);
    drop(h);
    assert_eq!(
        s.nav.get("sidebar_next"),
        Some("Shift+Down"),
        "conflict keeps the old binding"
    );
    assert!(
        s.shortcut_error.contains("already used"),
        "{}",
        s.shortcut_error
    );
    assert!(s.nav.set("sidebar_next", "Shift+J").is_ok());
    s.nav.write_to(&mut s.settings);
    shortcuts::restore_defaults(&mut s);
    assert_eq!(s.nav.get("sidebar_next"), Some("Shift+Down"));
    assert_eq!(
        s.settings.get_str("sidebar_next").or(s
            .settings
            .get("navigation_bindings")
            .and_then(|v| v.get("sidebar_next"))
            .and_then(|v| v.as_str())),
        Some("Shift+Down")
    );
}
