use egui::FontFamily;
use fthrust_ui::fonts::{self, Weight};
use fthrust_ui::tokens::Tokens;

fn run_frame(ctx: &egui::Context) {
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
}

fn ctx_with_fonts() -> egui::Context {
    let ctx = egui::Context::default();
    fonts::install(&ctx, &Tokens::default_palette());
    // Fonts are realised on the first frame.
    run_frame(&ctx);
    ctx
}

#[test]
fn oswald_families_are_registered() {
    let ctx = ctx_with_fonts();
    let widths: Vec<f32> = [Weight::W400, Weight::W600, Weight::W700]
        .iter()
        .map(|w| fonts::measure(&ctx, "FTHR Clips", 17.0, *w).width)
        .collect();
    assert!(widths.iter().all(|w| *w > 40.0), "{widths:?}");
    assert!(
        widths[0] < widths[1] && widths[1] < widths[2],
        "bolder is wider: {widths:?}"
    );
    let families = ctx.fonts(|f| f.families());
    for name in [fonts::FAMILY_400, fonts::FAMILY_600, fonts::FAMILY_700] {
        assert!(
            families.contains(&FontFamily::Name(name.into())),
            "{name} missing from {families:?}"
        );
    }
}

#[test]
fn proportional_is_body_400() {
    let ctx = ctx_with_fonts();
    let a = fonts::measure(&ctx, "Settings", 15.0, Weight::W400).width;
    let b = ctx.fonts_mut(|f| {
        f.layout_no_wrap(
            "Settings".into(),
            egui::FontId::proportional(15.0),
            egui::Color32::WHITE,
        )
        .size()
        .x
    });
    assert!((a - b).abs() < 0.01, "{a} vs {b}");
}

#[test]
fn pixel_size_metrics_match_qt() {
    // Qt reports for Oswald at 17 px / weight 700 ("FTHR Clips" brand label): see settings.json.
    let json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/reference/qt-screens/settings.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let brand = json["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["text"]["content"] == "FTHR Clips")
        .unwrap();
    let qt_height = brand["text"]["height"].as_f64().unwrap() as f32;
    let qt_advance = brand["text"]["advance"].as_f64().unwrap() as f32;
    let ctx = ctx_with_fonts();
    let m = fonts::measure(&ctx, "FTHR Clips", 17.0, Weight::W700);
    assert!(
        (m.height - qt_height).abs() <= 1.0,
        "height {} vs Qt {qt_height}",
        m.height
    );
    assert!(
        (m.width - qt_advance).abs() <= 2.0,
        "advance {} vs Qt {qt_advance}",
        m.width
    );
}

#[test]
fn theme_font_file_registers_named_family() {
    let ctx = egui::Context::default();
    let mut tokens = Tokens::default_palette();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("custom.ttf");
    std::fs::copy(
        fthrust_ui::assets_dir().join("fonts/Oswald-Bold.ttf"),
        &file,
    )
    .unwrap();
    tokens.font_files = vec![("My Face".into(), file)];
    tokens.body_family = "My Face".into();
    fonts::install(&ctx, &tokens);
    run_frame(&ctx);
    let families = ctx.fonts(|f| f.families());
    assert!(
        families.contains(&FontFamily::Name(fonts::theme_family("My Face").into())),
        "{families:?}"
    );
    // Body role now resolves to the custom face at every weight.
    let body = fonts::measure(&ctx, "FTHR Clips", 17.0, Weight::W400).width;
    let bold = ctx.fonts_mut(|f| {
        f.layout_no_wrap(
            "FTHR Clips".into(),
            egui::FontId::new(17.0, FontFamily::Name(fonts::FAMILY_700.into())),
            egui::Color32::WHITE,
        )
        .size()
        .x
    });
    assert!(
        (body - bold).abs() < 0.01,
        "body role uses the custom (bold) face: {body} vs {bold}"
    );
}
