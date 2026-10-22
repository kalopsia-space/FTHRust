//! Load-time merge and palette/typography migrations.

use super::defaults::*;
use super::{ThemeData, is_hex_color};
use serde_json::Value;
use std::collections::BTreeMap;

/// Font families that are always available without an imported file.
pub const BUILTIN_FONT_FAMILIES: [&str; 2] = ["Oswald", "Segoe UI"];

fn string_map(v: Option<&Value>) -> Option<BTreeMap<String, String>> {
    let obj = v?.as_object()?;
    Some(
        obj.iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect(),
    )
}

/// Replaces tokens whose value equals the previous shipped default.
fn upgrade(
    target: &mut BTreeMap<String, String>,
    previous: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) {
    for (token, old) in previous {
        if target.get(token) == Some(old)
            && let Some(new) = current.get(token)
        {
            target.insert(token.clone(), new.clone());
        }
    }
}

/// Merges a parsed `theme.json` onto the defaults and applies migrations.
///
/// Mirrors the Python `ThemeManager._load`. Versions of the result are always
/// the current ones (4 and 2); nothing is written to disk.
pub(super) fn merge_and_migrate(loaded: &Value) -> ThemeData {
    let mut data = ThemeData::default();
    let palette = loaded
        .get("palette_version")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let typography = loaded
        .get("typography_version")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let def_colors = default_colors();
    let def_cc = default_capture_card_colors();

    if let Some(stored) = string_map(loaded.get("colors")) {
        data.colors.extend(stored);
        for (step, version) in [(1u8, 2u64), (2, 3), (3, 4)] {
            if palette < version {
                upgrade(&mut data.colors, &legacy_palette(step), &def_colors);
            }
        }
    }
    if let Some(m) = string_map(loaded.get("icons")) {
        data.icons = m;
    }
    if let Some(m) = string_map(loaded.get("sounds")) {
        data.sounds = m;
    }
    if let Some(mut tints) = string_map(loaded.get("icon_tints")) {
        if palette < 3 && tints.get("_global").map(String::as_str) == Some("#00ffaa") {
            tints.insert("_global".into(), DEFAULT_ICON_TINT.into());
        }
        if palette < 4 && tints.get("_global") == legacy_palette(3).get("TEXT_DIM") {
            tints.insert("_global".into(), DEFAULT_ICON_TINT.into());
        }
        data.icon_tints = tints;
    }
    if let Some(stored) = string_map(loaded.get("capture_card")) {
        data.capture_card.extend(stored);
        for (step, version) in [(1u8, 2u64), (2, 3), (3, 4)] {
            if palette < version {
                upgrade(&mut data.capture_card, &legacy_capture_card(step), &def_cc);
            }
        }
    }
    if let Some(stored) = string_map(loaded.get("fonts")) {
        data.fonts.extend(stored);
    }
    if let Some(m) = string_map(loaded.get("font_files")) {
        data.font_files = m;
    }

    sanitize_colors(&mut data.colors, &mut data.capture_card);

    // Restore Oswald after the briefly shipped Segoe default; keep imports.
    for (role, default) in default_fonts() {
        let family = data.fonts.get(&role).cloned().unwrap_or_default();
        if data.font_files.contains_key(&family) {
            continue;
        }
        if !BUILTIN_FONT_FAMILIES.contains(&family.as_str())
            || (typography < 2 && family == "Segoe UI")
        {
            data.fonts.insert(role, default);
        }
    }
    data
}

/// Drops unknown colour tokens and replaces malformed values with the
/// shipped default for that token, in both the palette and the capture-card
/// map. Shared by the loader and the zip importer.
pub(crate) fn sanitize_colors(
    colors: &mut std::collections::BTreeMap<String, String>,
    capture_card: &mut std::collections::BTreeMap<String, String>,
) {
    let def_colors = super::default_colors();
    let def_cc = super::default_capture_card_colors();
    colors.retain(|k, _| def_colors.contains_key(k));
    for (token, default) in &def_colors {
        if !colors.get(token).is_some_and(|v| is_hex_color(v)) {
            colors.insert(token.clone(), default.clone());
        }
    }
    for (token, default) in &def_cc {
        if !capture_card.get(token).is_some_and(|v| is_hex_color(v)) {
            capture_card.insert(token.clone(), default.clone());
        }
    }
}
