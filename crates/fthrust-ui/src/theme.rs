//! Applying `~/.fthr/theme/theme.json` (through `fthrust_core::theme`) to
//! the tokens: colours, capture-card colours, fonts, icon tints and files.

use std::collections::BTreeMap;

use fthrust_core::FthrPaths;
use fthrust_core::theme::ThemeStore;

use crate::tokens::{CaptureCardColors, Color, Tokens, parse_hex};

/// Built-in families a role may name without an imported file.
pub use fthrust_core::theme::BUILTIN_FONT_FAMILIES as BUILTIN_FAMILIES;

/// Loads the theme for `paths` and applies it to the default palette. A
/// missing or corrupt file yields the defaults (the core loader quarantines
/// and reports; the UI never shows an unstyled screen).
pub fn load(paths: &FthrPaths) -> Tokens {
    let mut tokens = Tokens::default_palette();
    match ThemeStore::load(paths.clone()) {
        Ok(store) => apply(&mut tokens, &store),
        Err(e) => tracing::warn!("theme not loaded, using defaults: {e}"),
    }
    tokens
}

/// Overrides `tokens` with every valid value in `store`; invalid or missing
/// values keep what `tokens` already holds.
pub fn apply(tokens: &mut Tokens, store: &ThemeStore) {
    for c in Color::ALL {
        if let Some(value) = parse_hex(store.color(c.key())) {
            tokens.set_color(c, value);
        }
    }
    let cc = |key: &str, current| parse_hex(store.capture_card_color(key)).unwrap_or(current);
    tokens.capture_card = CaptureCardColors {
        bg: cc("CAPTURE_CARD_BG", tokens.capture_card.bg),
        accent: cc("CAPTURE_CARD_ACCENT", tokens.capture_card.accent),
        text: cc("CAPTURE_CARD_TEXT", tokens.capture_card.text),
        divider: cc("CAPTURE_CARD_DIVIDER", tokens.capture_card.divider),
        stats_dim: cc("CAPTURE_CARD_STATS_DIM", tokens.capture_card.stats_dim),
        progress_track: cc(
            "CAPTURE_CARD_PROGRESS_TRACK",
            tokens.capture_card.progress_track,
        ),
        progress_fill: cc(
            "CAPTURE_CARD_PROGRESS_FILL",
            tokens.capture_card.progress_fill,
        ),
    };
    if let Some(global) = parse_hex(store.icon_tint("_global")) {
        tokens.icon_tint_global = global;
    }
    let mut tints = BTreeMap::new();
    for (logical, value) in &store.data.icon_tints {
        if logical != "_global"
            && let Some(c) = parse_hex(value)
        {
            tints.insert(logical.clone(), c);
        }
    }
    tokens.icon_tints = tints;
    tokens.icon_overrides = store
        .data
        .icons
        .keys()
        .filter_map(|logical| {
            store
                .custom_icon_path(logical)
                .map(|p| (logical.clone(), p))
        })
        .collect();
    tokens.font_files = store.custom_font_paths();
    let known = |family: &str| {
        BUILTIN_FAMILIES.contains(&family) || tokens.font_files.iter().any(|(f, _)| f == family)
    };
    for (role, target) in [
        ("display", &mut tokens.display_family),
        ("body", &mut tokens.body_family),
    ] {
        if let Some(family) = store.data.fonts.get(role)
            && known(family)
        {
            *target = family.clone();
        } else {
            *target = "Oswald".into();
        }
    }
}
