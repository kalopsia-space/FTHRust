//! Custom game and blacklist rules (`core/game_detector.py`): normalised,
//! de-duplicated, matched on Wayland by title substring and app id. The
//! reference's `exe_*` fields are kept in the file but never evaluated.

use serde_json::{Map, Value, json};

/// A normalised crop rectangle stored with a rule (fractions of the frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropProfile {
    /// Whether the crop applies.
    pub enabled: bool,
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// One rule.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CustomGameRule {
    /// Case-insensitive title substring.
    pub title_contains: String,
    /// Case-insensitive app id equality (Wayland).
    pub app_id: String,
    /// Kept for the file format; not evaluated on Wayland.
    pub exe_name: String,
    /// Kept for the file format; not evaluated on Wayland.
    pub exe_path: String,
    /// Kept for the file format; not evaluated on Wayland.
    pub folder_path: String,
    /// Optional crop.
    pub crop: Option<CropProfile>,
}

fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// `normalise_crop_profile`: clamps into the unit square, min size 0.01.
pub fn normalise_crop(v: &Value) -> Option<CropProfile> {
    let m = v.as_object()?;
    let f = |k: &str, d: f64| {
        m.get(k)
            .map(|x| {
                x.as_f64()
                    .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(Some(d))
    };
    let (x, y, w, h) = (f("x", 0.0)?, f("y", 0.0)?, f("w", 1.0)?, f("h", 1.0)?);
    if ![x, y, w, h].iter().all(|v| v.is_finite()) {
        return None;
    }
    let x = x.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let w = w.max(0.01).min(1.0 - x);
    let h = h.max(0.01).min(1.0 - y);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(CropProfile {
        enabled: m.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        x: round6(x),
        y: round6(y),
        w: round6(w),
        h: round6(h),
    })
}

fn text(m: &Map<String, Value>, k: &str) -> String {
    m.get(k)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned()
}

/// `normalise_custom_game_rules`: trims, derives `exe_name` from
/// `exe_path`, drops empty rules and duplicates (case-insensitive).
pub fn normalise_rules(raw: &Value) -> Vec<CustomGameRule> {
    let Some(list) = raw.as_array() else {
        return Vec::new();
    };
    let mut out: Vec<CustomGameRule> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in list {
        let Some(m) = item.as_object() else { continue };
        let title = text(m, "title_contains");
        let app_id = text(m, "app_id");
        let exe_path = text(m, "exe_path");
        let folder_path = text(m, "folder_path");
        let mut exe_name = text(m, "exe_name");
        if exe_name.is_empty() && !exe_path.is_empty() {
            exe_name = std::path::Path::new(&exe_path)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        if title.is_empty()
            && app_id.is_empty()
            && exe_path.is_empty()
            && folder_path.is_empty()
            && exe_name.is_empty()
        {
            continue;
        }
        let key = (
            title.to_lowercase(),
            app_id.to_lowercase(),
            exe_path.to_lowercase(),
            folder_path.to_lowercase(),
            exe_name.to_lowercase(),
        );
        if !seen.insert(key) {
            continue;
        }
        out.push(CustomGameRule {
            title_contains: title,
            app_id,
            exe_name,
            exe_path,
            folder_path,
            crop: m.get("crop_profile").and_then(normalise_crop),
        });
    }
    out
}

/// The first rule matching `title`/`app_id` (exe-only rules never match
/// on Wayland).
pub fn matching_rule<'a>(
    rules: &'a [CustomGameRule],
    title: &str,
    app_id: &str,
) -> Option<&'a CustomGameRule> {
    let title_l = title.to_lowercase();
    let app_l = app_id.to_lowercase();
    rules.iter().find(|r| {
        let by_title =
            !r.title_contains.is_empty() && title_l.contains(&r.title_contains.to_lowercase());
        let by_app = !r.app_id.is_empty() && app_l == r.app_id.to_lowercase();
        if r.title_contains.is_empty() && r.app_id.is_empty() {
            return false;
        }
        (r.title_contains.is_empty() || by_title) && (r.app_id.is_empty() || by_app)
    })
}

/// Whether a blacklist rule matches.
pub fn is_blacklisted(rules: &[CustomGameRule], title: &str, app_id: &str) -> bool {
    matching_rule(rules, title, app_id).is_some()
}

/// The enabled crop of the matching rule.
pub fn crop_for(rules: &[CustomGameRule], title: &str, app_id: &str) -> Option<CropProfile> {
    matching_rule(rules, title, app_id)
        .and_then(|r| r.crop)
        .filter(|c| c.enabled)
}

/// Serialises rules in the reference's shape (`app_id` only when set).
pub fn to_value(rules: &[CustomGameRule]) -> Value {
    Value::Array(
        rules
            .iter()
            .map(|r| {
                let mut m = json!({
                    "title_contains": r.title_contains,
                    "exe_path": r.exe_path,
                    "folder_path": r.folder_path,
                    "exe_name": r.exe_name,
                });
                if !r.app_id.is_empty() {
                    m["app_id"] = json!(r.app_id);
                }
                if let Some(c) = r.crop {
                    m["crop_profile"] =
                        json!({"enabled": c.enabled, "x": c.x, "y": c.y, "w": c.w, "h": c.h});
                }
                m
            })
            .collect(),
    )
}
