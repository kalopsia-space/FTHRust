//! Oswald (400/600/700) from `assets/fonts`, theme font files, `FontId`
//! helpers and text metrics. `fthrust_core::theme` names roles
//! (`display`, `body`) by family; a role naming an imported file maps every
//! weight to that file, as the reference's `QFontDatabase` registration does.

use std::sync::Arc;

use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use egui::{Color32, Context, FontData, FontDefinitions, FontFamily, FontId, RichText};

use crate::tokens::Tokens;

/// Named family for Oswald Regular.
pub const FAMILY_400: &str = "oswald-400";
/// Named family for Oswald SemiBold.
pub const FAMILY_600: &str = "oswald-600";
/// Named family for Oswald Bold.
pub const FAMILY_700: &str = "oswald-700";

/// Font weights the reference uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Weight {
    W400,
    W600,
    W700,
}

impl Weight {
    /// The named family for this weight.
    pub fn family(self) -> &'static str {
        match self {
            Weight::W400 => FAMILY_400,
            Weight::W600 => FAMILY_600,
            Weight::W700 => FAMILY_700,
        }
    }

    /// Qt numeric weight.
    pub fn qt(self) -> u32 {
        match self {
            Weight::W400 => 400,
            Weight::W600 => 600,
            Weight::W700 => 700,
        }
    }

    /// The weight matching a Qt numeric weight (nearest).
    pub fn from_qt(w: u32) -> Weight {
        if w >= 650 {
            Weight::W700
        } else if w >= 500 {
            Weight::W600
        } else {
            Weight::W400
        }
    }
}

/// The named family under which a theme font file is registered.
pub fn theme_family(family: &str) -> String {
    format!("theme-{}", family.to_ascii_lowercase().replace(' ', "-"))
}

/// Registers Oswald and the theme's font files, then maps the three weight
/// families (and `Proportional`) to the body role.
pub fn install(ctx: &Context, tokens: &Tokens) {
    let mut defs = FontDefinitions::default();
    let dir = crate::assets_dir().join("fonts");
    let mut custom: Vec<(String, String)> = Vec::new();
    for (family, path) in &tokens.font_files {
        if let Ok(bytes) = std::fs::read(path) {
            let key = theme_family(family);
            defs.font_data
                .insert(key.clone(), Arc::new(FontData::from_owned(bytes)));
            defs.families
                .entry(FontFamily::Name(key.clone().into()))
                .or_default()
                .insert(0, key.clone());
            custom.push((family.clone(), key));
        }
    }
    let body_custom = custom
        .iter()
        .find(|(f, _)| *f == tokens.body_family)
        .map(|(_, k)| k.clone());
    for (weight, file) in [
        (Weight::W400, "Oswald-Regular.ttf"),
        (Weight::W600, "Oswald-SemiBold.ttf"),
        (Weight::W700, "Oswald-Bold.ttf"),
    ] {
        let key = weight.family().to_owned();
        match std::fs::read(dir.join(file)) {
            Ok(bytes) => {
                defs.font_data
                    .insert(key.clone(), Arc::new(FontData::from_owned(bytes)));
            }
            Err(e) => tracing::error!("font {file} unreadable: {e}"),
        }
        let mut chain = Vec::new();
        if let Some(c) = &body_custom {
            chain.push(c.clone());
        }
        chain.push(key);
        // Keep egui's defaults as glyph fallback (symbols, emoji).
        chain.extend(
            defs.families
                .get(&FontFamily::Proportional)
                .cloned()
                .unwrap_or_default(),
        );
        defs.families
            .insert(FontFamily::Name(weight.family().into()), chain);
    }
    let body = defs
        .families
        .get(&FontFamily::Name(FAMILY_400.into()))
        .cloned()
        .unwrap_or_default();
    defs.families.insert(FontFamily::Proportional, body);
    ctx.set_fonts(defs);
}

/// Adds one more font file at runtime (Customize → Import font) without
/// rebuilding the definitions.
pub fn add_file(ctx: &Context, family: &str, bytes: Vec<u8>) {
    let key = theme_family(family);
    ctx.add_font(FontInsert::new(
        &key,
        FontData::from_owned(bytes),
        vec![InsertFontFamily {
            family: FontFamily::Name(key.clone().into()),
            priority: FontPriority::Highest,
        }],
    ));
}

/// `FontId` for a pixel size and weight.
pub fn id(px: f32, weight: Weight) -> FontId {
    FontId::new(px, FontFamily::Name(weight.family().into()))
}

/// `RichText` in the given size, weight and colour.
pub fn text(s: impl Into<String>, px: f32, weight: Weight, color: Color32) -> RichText {
    RichText::new(s).font(id(px, weight)).color(color)
}

/// Measured text, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextMetrics {
    /// Advance width of the laid-out text.
    pub width: f32,
    /// Row height (ascent + descent, as Qt's `QFontMetrics::height`).
    pub height: f32,
    /// Baseline from the row top.
    pub ascent: f32,
    /// Descent below the baseline.
    pub descent: f32,
}

/// Lays `s` out without wrapping and reports its metrics.
pub fn measure(ctx: &Context, s: &str, px: f32, weight: Weight) -> TextMetrics {
    let galley = ctx.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), id(px, weight), Color32::WHITE));
    let size = galley.size();
    let (ascent, height) = galley
        .rows
        .first()
        .and_then(|r| r.row.glyphs.first())
        .map(|g| (g.font_ascent, g.font_height))
        .unwrap_or((size.y * 0.8, size.y));
    TextMetrics {
        width: size.x,
        height,
        ascent,
        descent: height - ascent,
    }
}
