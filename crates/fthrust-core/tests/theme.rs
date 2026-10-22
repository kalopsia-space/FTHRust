use fthrust_core::FthrPaths;
use fthrust_core::theme::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

fn fixture(rel: &str) -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn map(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect()
}

fn store_with(json: Option<&str>) -> (tempfile::TempDir, ThemeStore) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(tmp.path());
    if let Some(j) = json {
        std::fs::create_dir_all(paths.theme_dir()).unwrap();
        std::fs::write(paths.theme_file(), j).unwrap();
    }
    let s = ThemeStore::load(paths).unwrap();
    (tmp, s)
}

#[test]
fn defaults_match_fixture() {
    let f = fixture("theme-defaults.json");
    assert_eq!(default_colors(), map(&f["colors"]));
    assert_eq!(default_capture_card_colors(), map(&f["capture_card"]));
    assert_eq!(default_fonts(), map(&f["fonts"]));
    assert_eq!(DEFAULT_ICON_TINT, f["icon_tint"].as_str().unwrap());
    let toks: Vec<&str> = f["colors"]
        .as_object()
        .unwrap()
        .keys()
        .map(|s| s.as_str())
        .collect();
    assert_eq!(COLOR_TOKENS.to_vec(), toks);
    let pairs = |t: &[(&str, &str)]| -> BTreeMap<String, String> {
        t.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    };
    assert_eq!(pairs(CUSTOMIZABLE_ICONS), map(&f["customizable_icons"]));
    assert_eq!(pairs(ICON_ASSETS), map(&f["icon_assets"]));
    let sounds: Vec<&str> = f["customizable_sounds"]
        .as_object()
        .unwrap()
        .keys()
        .map(|s| s.as_str())
        .collect();
    assert_eq!(CUSTOMIZABLE_SOUNDS.to_vec(), sounds);
    let groups = f["color_groups"].as_array().unwrap();
    assert_eq!(COLOR_GROUPS.len(), groups.len());
    for (g, fg) in COLOR_GROUPS.iter().zip(groups) {
        assert_eq!(g.0, fg[0].as_str().unwrap());
        let ft: Vec<&str> = fg[1]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t[0].as_str().unwrap())
            .collect();
        assert_eq!(g.1.to_vec(), ft);
    }
    for v in 1..=3u8 {
        assert_eq!(legacy_palette(v), map(&f["legacy"][format!("v{v}")]));
        assert_eq!(
            legacy_capture_card(v),
            map(&f["legacy"][format!("cc_v{v}")])
        );
    }
    let d = ThemeData::default();
    assert_eq!((d.palette_version, d.typography_version), (4, 2));
    assert_eq!(d.icon_tints["_global"], DEFAULT_ICON_TINT);
}

fn check_case(case: &str) {
    let dir = format!("theme-migrations/{case}");
    let input = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(&dir)
            .join("input.json"),
    )
    .unwrap();
    let (_t, s) = store_with(Some(&input));
    let got = serde_json::to_value(&s.data).unwrap();
    assert_eq!(got, fixture(&format!("{dir}/expected.json")), "case {case}");
}

#[test]
fn migration_missing_keys() {
    check_case("missing-keys");
}
#[test]
fn migration_segoe_font() {
    check_case("segoe-font");
}
#[test]
fn migration_v1_untouched_defaults() {
    check_case("v1-untouched-defaults");
}
#[test]
fn migration_v1_user_override() {
    check_case("v1-user-override");
}
#[test]
fn migration_v3_global_tint() {
    check_case("v3-global-tint");
}

#[test]
fn invalid_color_replaced_by_default() {
    let (_t, s) = store_with(Some(
        r##"{"colors":{"ACCENT":"green","BG":"#aabbccdd","TEXT":"#ABCDEF","NOPE":"#000000"}}"##,
    ));
    assert_eq!(s.color("ACCENT"), "#20e3a0");
    assert_eq!(s.color("BG"), "#070a09");
    assert_eq!(s.color("TEXT"), "#ABCDEF");
    assert!(!s.data.colors.contains_key("NOPE"));
    assert!(is_hex_color("#aBc012") && !is_hex_color("#abc") && !is_hex_color("abcdef0"));
}

#[test]
fn parse_failure_gives_defaults() {
    let (_t, s) = store_with(Some("{not json"));
    assert_eq!(s.data, ThemeData::default());
}

#[test]
fn favicon_never_tinted() {
    let (_t, mut s) = store_with(None);
    assert_eq!(s.icon_tint("favicon.ico"), "");
    assert_eq!(s.icon_tint("play.png"), "#a0aba5");
    s.data
        .icon_tints
        .insert("play.png".into(), "#112233".into());
    assert_eq!(s.icon_tint("play.png"), "#112233");
    assert_eq!(s.icon_tint_for_custom("play.png"), Some("#112233"));
    assert_eq!(s.icon_tint_for_custom("other.png"), None);
}

#[test]
fn unknown_capture_card_key_is_white() {
    let (_t, s) = store_with(None);
    assert_eq!(s.capture_card_color("NOPE"), "#ffffff");
    assert_eq!(s.capture_card_color("CAPTURE_CARD_BG"), "#0a0f0d");
}

#[test]
fn set_custom_icon_names_dest_by_stem_and_source_ext() {
    let (t, mut s) = store_with(None);
    let src = t.path().join("foo.SVG");
    std::fs::write(&src, "<svg/>").unwrap();
    s.set_custom_icon("play.png", &src).unwrap();
    assert!(t.path().join(".fthr/theme/icons/play.svg").is_file());
    assert_eq!(s.data.icons["play.png"], "play.svg");
    assert!(s.custom_icon_path("play.png").is_some());
    let bad = t.path().join("x.txt");
    std::fs::write(&bad, "x").unwrap();
    assert!(s.set_custom_icon("play.png", &bad).is_err());
}

#[test]
fn custom_sound_and_save_roundtrip() {
    let (t, mut s) = store_with(None);
    let src = t.path().join("a.MP3");
    std::fs::write(&src, "x").unwrap();
    s.set_custom_sound("startup", &src).unwrap();
    assert_eq!(s.data.sounds["startup"], "startup.mp3");
    s.save().unwrap();
    let s2 = ThemeStore::load(FthrPaths::new(t.path())).unwrap();
    assert_eq!(s2.data, s.data);
    assert!(s2.custom_sound_path("startup").is_some());
}

#[test]
fn custom_font_paths_drop_missing_files() {
    let (t, mut s) = store_with(None);
    let src = t.path().join("My.ttf");
    std::fs::write(&src, "x").unwrap();
    s.set_custom_font(&["My Font".to_string()], &src).unwrap();
    assert_eq!(s.custom_font_paths().len(), 1);
    std::fs::remove_file(t.path().join(".fthr/theme/fonts/My.ttf")).unwrap();
    assert!(s.custom_font_paths().is_empty());
    assert!(
        s.set_custom_font(&["X".into()], &t.path().join("a.woff"))
            .is_err()
    );
}

#[test]
fn reset_all_keeps_files() {
    let (t, mut s) = store_with(None);
    assert!(!s.has_any_customization());
    let src = t.path().join("a.wav");
    std::fs::write(&src, "x").unwrap();
    s.set_custom_sound("error", &src).unwrap();
    assert!(s.has_any_customization());
    s.reset_all();
    assert_eq!(s.data, ThemeData::default());
    assert!(!s.has_any_customization());
    assert!(t.path().join(".fthr/theme/sounds/error.wav").is_file());
}
