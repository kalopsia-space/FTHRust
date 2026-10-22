//! Oswald (400/600/700) from `assets/fonts`, theme font files, `FontId`
//! helpers and text metrics. `fthrust_core::theme` names roles
//! (`display`, `body`) by family; a role naming an imported file maps every
//! weight to that file, as the reference's `QFontDatabase` registration does.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use egui::{Color32, Context, FontData, FontDefinitions, FontFamily, FontId, RichText};

use crate::tokens::Tokens;

/// Named family for Oswald Regular.
pub const FAMILY_400: &str = "oswald-400";
/// Named family for Oswald SemiBold.
pub const FAMILY_600: &str = "oswald-600";
/// Named family for Oswald Bold.
pub const FAMILY_700: &str = "oswald-700";
/// Named family for the system monospace face (Qt's `font-family: monospace`),
/// falling back to egui's bundled monospace.
pub const MONO_FAMILY: &str = "qt-monospace";

/// Candidate system monospace files, in Qt's fontconfig preference order
/// on this platform.
/// System CJK fallbacks (fontconfig's usual choices), tried in order.
const CJK_CANDIDATES: [&str; 5] = [
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Light.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJKjp-Regular.otf",
    "/usr/share/fonts/droid/DroidSansFallbackFull.ttf",
];
/// Named family for the CJK fallback face.
pub const CJK_FAMILY: &str = "system-cjk";

/// The first CJK fallback font present on this system.
pub fn cjk_fallback_path() -> Option<std::path::PathBuf> {
    CJK_CANDIDATES
        .iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.is_file())
}

/// True when `bytes` start like a TrueType/OpenType font or collection and
/// carry the tables egui's parser needs; a corrupt theme font is skipped
/// instead of panicking inside epaint.
pub fn is_font_file(bytes: &[u8]) -> bool {
    let Some(magic) = bytes.get(0..4) else {
        return false;
    };
    match magic {
        b"ttcf" => bytes.len() > 12,
        [0, 1, 0, 0] | b"OTTO" | b"true" => face_metrics(bytes).is_some(),
        _ => false,
    }
}

const MONO_CANDIDATES: [&str; 4] = [
    "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
];

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
        // Qt resolves a requested 500 (Medium) to Oswald Regular: weights at or
        // below Medium prefer the nearest lighter face.
        if w >= 650 {
            Weight::W700
        } else if w > 500 {
            Weight::W600
        } else {
            Weight::W400
        }
    }
}

/// Font-unit ratios Qt derives its integer metrics from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceMetrics {
    /// `xAvgCharWidth / unitsPerEm`.
    pub avg_width: f32,
    /// `hhea.ascender / unitsPerEm`.
    pub ascent: f32,
    /// `-hhea.descender / unitsPerEm`.
    pub descent: f32,
}

static METRICS: OnceLock<Mutex<HashMap<String, FaceMetrics>>> = OnceLock::new();

/// Reads the OS/2 average width and hhea ascent/descent ratios from a
/// TrueType/OpenType file (Qt's `QFontMetrics` rounds these per size).
pub fn face_metrics(bytes: &[u8]) -> Option<FaceMetrics> {
    let num = u16::from_be_bytes([*bytes.get(4)?, *bytes.get(5)?]) as usize;
    let (mut os2, mut head, mut hhea) = (None, None, None);
    for i in 0..num {
        let o = 12 + 16 * i;
        let tag = bytes.get(o..o + 4)?;
        let off = u32::from_be_bytes(bytes.get(o + 8..o + 12)?.try_into().ok()?) as usize;
        match tag {
            b"OS/2" => os2 = Some(off),
            b"head" => head = Some(off),
            b"hhea" => hhea = Some(off),
            _ => {}
        }
    }
    let (os2, head, hhea) = (os2?, head?, hhea?);
    let i16_at = |o: usize| -> Option<i16> {
        Some(i16::from_be_bytes([*bytes.get(o)?, *bytes.get(o + 1)?]))
    };
    let xavg = i16_at(os2 + 2)? as f32;
    let upem = u16::from_be_bytes([*bytes.get(head + 18)?, *bytes.get(head + 19)?]) as f32;
    let asc = i16_at(hhea + 4)? as f32;
    let desc = i16_at(hhea + 6)? as f32;
    (upem > 0.0).then_some(FaceMetrics {
        avg_width: xavg / upem,
        ascent: asc / upem,
        descent: -desc / upem,
    })
}

fn remember_metrics(key: &str, bytes: &[u8]) {
    if let Some(m) = face_metrics(bytes) {
        METRICS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(key.to_owned(), m);
    }
}

fn metrics_for(family_key: &str) -> FaceMetrics {
    METRICS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .get(family_key)
        .copied()
        .unwrap_or(FaceMetrics {
            avg_width: 0.465,
            ascent: 1.193,
            descent: 0.289,
        })
}

/// Qt's integer `averageCharWidth()` for a role at `px`.
pub fn avg_char_width(px: f32, weight: Weight) -> f32 {
    (metrics_for(weight.family()).avg_width * px).round()
}

/// Qt's `QFontMetrics::height()`: `round(ascent) + round(descent)`.
pub fn qt_height(px: f32, weight: Weight) -> f32 {
    let m = metrics_for(weight.family());
    (m.ascent * px).round() + (m.descent * px).round()
}

/// [`qt_wrapped_line_height`] for a named family (the monospace face).
pub fn qt_wrapped_line_height_named(family_key: &str, px: f32) -> f32 {
    let m = metrics_for(family_key);
    ((m.ascent + m.descent) * px).ceil()
}

/// The height of one line in a word-wrapped `QLabel` (`QTextLine`):
/// `ceil(ascent + descent)`.
pub fn qt_wrapped_line_height(px: f32, weight: Weight) -> f32 {
    let m = metrics_for(weight.family());
    ((m.ascent + m.descent) * px).ceil()
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
            if !is_font_file(&bytes) {
                tracing::warn!(
                    "theme font {} is not a usable font file; skipped",
                    path.display()
                );
                continue;
            }
            let key = theme_family(family);
            remember_metrics(&key, &bytes);
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
    // Qt falls back through fontconfig for scripts Oswald lacks; load the
    // system CJK face once so titles, paths and device names never show tofu.
    let cjk_loaded = cjk_fallback_path()
        .and_then(|p| std::fs::read(p).ok())
        .filter(|b| is_font_file(b))
        .map(|bytes| {
            defs.font_data
                .insert(CJK_FAMILY.to_owned(), Arc::new(FontData::from_owned(bytes)));
        })
        .is_some();
    for (weight, file) in [
        (Weight::W400, "Oswald-Regular.ttf"),
        (Weight::W600, "Oswald-SemiBold.ttf"),
        (Weight::W700, "Oswald-Bold.ttf"),
    ] {
        let key = weight.family().to_owned();
        match std::fs::read(dir.join(file)) {
            Ok(bytes) => {
                remember_metrics(&key, &bytes);
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
        if cjk_loaded {
            chain.push(CJK_FAMILY.to_owned());
        }
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
    // System monospace (what Qt resolves `monospace` to), else egui's.
    let mut mono_chain: Vec<String> = Vec::new();
    for path in MONO_CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            remember_metrics(MONO_FAMILY, &bytes);
            defs.font_data.insert(
                MONO_FAMILY.to_owned(),
                Arc::new(FontData::from_owned(bytes)),
            );
            mono_chain.push(MONO_FAMILY.to_owned());
            break;
        }
    }
    mono_chain.extend(
        defs.families
            .get(&FontFamily::Monospace)
            .cloned()
            .unwrap_or_default(),
    );
    defs.families
        .insert(FontFamily::Name(MONO_FAMILY.into()), mono_chain);
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
