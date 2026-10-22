use egui::Color32;
use fthrust_core::FthrPaths;
use fthrust_ui::theme;
use fthrust_ui::tokens::{Color, Tokens, geom, motion, size};

fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

#[test]
fn default_palette_matches_inventory() {
    let t = Tokens::default_palette();
    let expect = [
        (Color::Bg, "#070a09"),
        (Color::Surface1, "#0a0f0d"),
        (Color::Surface2, "#101512"),
        (Color::Surface3, "#141b17"),
        (Color::ShellBg, "#0a0f0d"),
        (Color::ShellBg2, "#101512"),
        (Color::ShellDivider, "#1b231e"),
        (Color::CardBg, "#101512"),
        (Color::CardBgHi, "#141b17"),
        (Color::CardBorder, "#1b231e"),
        (Color::Hairline, "#181f1b"),
        (Color::Border, "#242c27"),
        (Color::BorderHi, "#49584f"),
        (Color::Text, "#f3f6f4"),
        (Color::TextDim, "#a0aba5"),
        (Color::TextMuted, "#7e8a83"),
        (Color::TextGhost, "#7e8a83"),
        (Color::Accent, "#20e3a0"),
        (Color::AccentDim, "#16bf87"),
        (Color::AccentSoft, "#0d2b20"),
        (Color::Error, "#ff795d"),
        (Color::ErrorSoft, "#270b0f"),
        (Color::Warning, "#e7b56e"),
        (Color::Success, "#20e3a0"),
        (Color::Delete, "#b84f45"),
    ];
    assert_eq!(Color::ALL.len(), 25);
    for (c, h) in expect {
        assert_eq!(hex(t.color(c)), h, "{c:?}");
    }
    assert_eq!(Color::Surface1.key(), "SURFACE_1");
    assert_eq!(Color::Bg.key(), "BG");
    assert_eq!(hex(t.capture_card.bg), "#0a0f0d");
    assert_eq!(hex(t.capture_card.accent), "#20e3a0");
    assert_eq!(hex(t.capture_card.progress_fill), "#20e3a0");
    assert_eq!(hex(t.bar_bg()), "#0a0f0d");
    assert_eq!(hex(t.bar_fg_dim()), "#a0aba5");
    assert_eq!(hex(t.icon_tint_global), "#a0aba5");
    assert_eq!(t.display_family, "Oswald");
    assert_eq!(t.body_family, "Oswald");
    assert_eq!(
        (
            size::MICRO,
            size::LABEL,
            size::BODY,
            size::BODY_L,
            size::H3,
            size::H2,
            size::H1
        ),
        (12.0, 14.0, 15.0, 17.0, 20.0, 26.0, 36.0)
    );
    assert_eq!(
        (
            geom::TITLE_BAR_H,
            geom::CAPTURE_BAR_H,
            geom::CAPTURE_CONTROL_H,
            geom::FILTER_H,
            geom::HW_BAR_H,
            geom::NAV_W,
            geom::NAV_BTN
        ),
        (36.0, 64.0, 44.0, 60.0, 36.0, 94.0, 78.0)
    );
    assert_eq!(
        (
            geom::RADIUS,
            geom::RADIUS_SM,
            geom::RADIUS_CARD,
            geom::PAGE_INSET,
            geom::RESIZE_MARGIN
        ),
        (6.0, 4.0, 8.0, 24.0, 8.0)
    );
    assert_eq!(geom::SPACE, [4.0, 8.0, 12.0, 16.0, 20.0, 24.0, 32.0, 48.0]);
    assert_eq!(
        (
            motion::HOVER_MS,
            motion::SLIDE_MS,
            motion::POPUP_FADE_MS,
            motion::SCROLL_TAU_MS
        ),
        (120, 220, 180, 35)
    );
}

fn write_theme(dir: &std::path::Path, json: &str) -> FthrPaths {
    let paths = FthrPaths::new(dir);
    std::fs::create_dir_all(paths.theme_dir()).unwrap();
    std::fs::write(paths.theme_file(), json).unwrap();
    paths
}

#[test]
fn theme_overrides_only_valid_values() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = write_theme(
        tmp.path(),
        r##"{"palette_version": 4, "typography_version": 2,
             "colors": {"ACCENT": "#ff0000", "TEXT": "nope"},
             "icon_tints": {"_global": "#123456", "play.png": "#abcdef"},
             "fonts": {"display": "Oswald", "body": "Nonexistent Family"}}"##,
    );
    let t = theme::load(&paths);
    assert_eq!(hex(t.color(Color::Accent)), "#ff0000");
    assert_eq!(
        hex(t.color(Color::Text)),
        "#f3f6f4",
        "invalid value keeps the default"
    );
    assert_eq!(
        hex(t.color(Color::Bg)),
        "#070a09",
        "missing value keeps the default"
    );
    assert_eq!(hex(t.icon_tint_global), "#123456");
    assert_eq!(hex(t.icon_tint("play.png")), "#abcdef");
    assert_eq!(
        hex(t.icon_tint("pause.png")),
        "#123456",
        "falls back to the global tint"
    );
    assert_eq!(
        t.body_family, "Oswald",
        "unknown families fall back to Oswald"
    );
}

#[test]
fn corrupt_theme_file_gives_defaults() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = write_theme(tmp.path(), "{not json");
    let t = theme::load(&paths);
    assert_eq!(hex(t.color(Color::Accent)), "#20e3a0");
    let tmp2 = tempfile::tempdir().unwrap();
    let t = theme::load(&FthrPaths::new(tmp2.path()));
    assert_eq!(hex(t.color(Color::Surface2)), "#101512", "no file at all");
}

#[test]
fn custom_icon_override_resolves_under_theme_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = write_theme(
        tmp.path(),
        r##"{"palette_version": 4, "typography_version": 2, "colors": {}, "icons": {"play.png": "play.svg"}}"##,
    );
    std::fs::create_dir_all(paths.theme_icons_dir()).unwrap();
    std::fs::write(paths.theme_icons_dir().join("play.svg"), "<svg/>").unwrap();
    let t = theme::load(&paths);
    assert_eq!(
        t.icon_overrides.get("play.png"),
        Some(&paths.theme_icons_dir().join("play.svg"))
    );
    assert!(!t.icon_overrides.contains_key("pause.png"));
}
