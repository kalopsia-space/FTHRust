use egui::Color32;
use fthrust_ui::icons::{Icon, IconCache, rasterise};
use fthrust_ui::tokens::{Tokens, parse_hex};

#[test]
fn asset_map_matches_inventory() {
    let assets = fthrust_ui::assets_dir();
    for icon in Icon::ALL {
        let path = icon.bundled_path(&assets);
        assert!(path.is_file(), "{:?} -> {}", icon, path.display());
    }
    assert_eq!(Icon::Clip.logical_name(), "clip.png");
    assert_eq!(Icon::Clip.asset(), "icons/clips.svg");
    assert_eq!(Icon::General.logical_name(), "settings(general).png");
    assert_eq!(Icon::Decrease.asset(), "icons/minimise.svg");
    assert_eq!(Icon::Increase.asset(), "icons/plus.svg");
    assert_eq!(Icon::Logo.asset(), "fthr_logo.png");
    assert_eq!(Icon::Favicon.logical_name(), "favicon.ico");
    assert_eq!(Icon::ALL.len(), 28);
    assert_eq!(fthrust_ui::icons::sizes(), [14.0, 16.0, 18.0, 28.0, 36.0]);
}

#[test]
fn tint_is_source_in() {
    let tint = parse_hex("#20e3a0").unwrap();
    let img = rasterise(
        &fthrust_ui::assets_dir().join("icons/close.svg"),
        14,
        Some(tint),
    )
    .unwrap();
    assert_eq!(img.size, [28, 28], "rendered at 2x like the reference");
    let mut opaque = 0;
    for p in &img.pixels {
        if p.a() == 255 {
            opaque += 1;
            let u = p.to_srgba_unmultiplied();
            assert_eq!((u[0], u[1], u[2]), (0x20, 0xe3, 0xa0), "{p:?}");
        } else if p.a() > 0 {
            // Storage is premultiplied: every channel is tint * alpha, within one rounding step.
            let a = p.a() as f32 / 255.0;
            for (got, want) in [p.r(), p.g(), p.b()].into_iter().zip([0x20u8, 0xe3, 0xa0]) {
                let want = (want as f32 * a).round() as u8;
                assert!(got.abs_diff(want) <= 1, "{p:?}");
            }
        }
    }
    assert!(opaque > 20, "{opaque} opaque pixels");
}

#[test]
fn untinted_keeps_source_colour() {
    let img = rasterise(&fthrust_ui::assets_dir().join("icons/close.svg"), 14, None).unwrap();
    let any = img
        .pixels
        .iter()
        .find(|p| p.a() == 255)
        .unwrap()
        .to_srgba_unmultiplied();
    assert_eq!(
        (any[0], any[1], any[2]),
        (0xb4, 0xbd, 0xc7),
        "stroke #B4BDC7 untouched"
    );
}

#[test]
fn png_assets_rasterise_too() {
    let img = rasterise(&fthrust_ui::assets_dir().join("fthr_logo.png"), 28, None).unwrap();
    assert!(img.size[0] <= 56 && img.size[1] <= 56 && img.size[0] > 0);
}

#[test]
fn cache_uses_theme_tint_and_custom_override() {
    let ctx = egui::Context::default();
    let mut tokens = Tokens::default_palette();
    let mut cache = IconCache::default();
    let mut out = ctx.run_pass(egui::RawInput::default(), |ctx| {
        let a = cache.texture(ctx, Icon::Play, 18.0, &tokens);
        assert_eq!(a.size, egui::Vec2::splat(36.0));
        let before = cache.texture(ctx, Icon::Play, 18.0, &tokens).id;
        assert_eq!(a.id, before, "cached");
        tokens.icon_tints.insert("play.png".into(), Color32::RED);
        let b = cache.texture(ctx, Icon::Play, 18.0, &tokens);
        assert_ne!(a.id, b.id, "per-icon tint changes the texture");
        assert_eq!(cache.last_tint(Icon::Play), Some(Color32::RED));
        // Custom icon file: untinted unless a per-icon tint exists.
        let tmp = tempfile::tempdir().unwrap();
        let custom = tmp.path().join("pause.svg");
        std::fs::copy(fthrust_ui::assets_dir().join("icons/close.svg"), &custom).unwrap();
        tokens.icon_overrides.insert("pause.png".into(), custom);
        cache.texture(ctx, Icon::Pause, 14.0, &tokens);
        assert_eq!(cache.last_tint(Icon::Pause), None);
        tokens.icon_tints.insert("pause.png".into(), Color32::BLUE);
        cache.texture(ctx, Icon::Pause, 14.0, &tokens);
        assert_eq!(cache.last_tint(Icon::Pause), Some(Color32::BLUE));
        // Favicon and logo are never tinted by the global tint.
        cache.texture(ctx, Icon::Logo, 28.0, &tokens);
        assert_eq!(cache.last_tint(Icon::Logo), None);
    });
    out.textures_delta.clear();
}
