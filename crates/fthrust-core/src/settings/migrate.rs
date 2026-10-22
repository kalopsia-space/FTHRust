//! Settings migrations, ported from the Python `SettingsManager.load`.

use crate::FthrPaths;
use serde_json::{Map, Value, json};

/// Settings keys removed outright (besides the overlay prefixes).
const RETIRED_KEYS: [&str; 5] = [
    "input_overlay_preview_clip",
    "multiband_audio_enabled",
    "audio_categories",
    "auto_crop_enabled",
    "extended_clip_length",
];

/// Latest navigation bindings schema version.
const NAV_VERSION: i64 = 5;

/// Applies settings migrations in place; returns true when the result should
/// be written back to disk.
///
/// `data` is the one-level merge of the defaults and the file; `loaded` is the
/// file's own top-level content, needed because the Python decides on key
/// absence in the file, not in the merged map. Like the reference, only the
/// retired-key removal, image-layer creation and navigation migrations ask
/// for a re-save; the other derived values are recomputed on every load. A
/// missing file is never migrated.
pub(crate) fn migrate(
    data: &mut Map<String, Value>,
    loaded: &Map<String, Value>,
    paths: &FthrPaths,
    file_existed: bool,
) -> bool {
    if !file_existed {
        return false;
    }
    let mut rewrite = false;

    // (1) clips and recording directories.
    if !loaded.contains_key("clips_directory") {
        data.insert(
            "clips_directory".into(),
            json!(paths.legacy_clips_directory().to_string_lossy()),
        );
    }
    if !loaded.contains_key("recording_directory")
        && let Some(clips) = data
            .get("clips_directory")
            .and_then(Value::as_str)
            .and_then(|raw| super::resolve_storage_path(raw, paths))
    {
        let rec = clips.join("Recordings");
        data.insert("recording_directory".into(), json!(rec.to_string_lossy()));
    }

    // (6) navigation bindings (the reference runs this before the rest).
    rewrite |= migrate_navigation(data, loaded, paths);

    // (3) seed recording values from the clip values.
    for (rec, clip) in [
        ("recording_framerate", "framerate"),
        ("recording_resolution", "resolution"),
        ("recording_bitrate_level", "bitrate_level"),
        ("recording_custom_bitrate_kbps", "custom_bitrate_kbps"),
    ] {
        if !loaded.contains_key(rec)
            && let Some(v) = data.get(clip).cloned()
        {
            data.insert(rec.into(), v);
        }
    }

    // (2) retired keys.
    let retired: Vec<String> = data
        .keys()
        .filter(|k| {
            k.starts_with("keyboard_overlay_")
                || k.starts_with("mouse_overlay_")
                || RETIRED_KEYS.contains(&k.as_str())
        })
        .cloned()
        .collect();
    rewrite |= !retired.is_empty();
    for k in retired {
        data.remove(&k);
    }
    if let Some(Value::Object(hot)) = data.get_mut("hotkeys") {
        rewrite |= hot.remove("save_extended_clip").is_some();
    }

    // (4) singular image overlay becomes the first layer.
    let legacy_path = match loaded.get("image_overlay_path") {
        Some(Value::String(s)) => s.as_str(),
        _ => "",
    };
    if !loaded.contains_key("image_overlays") && !legacy_path.is_empty() {
        let layer = json!({
            "path": legacy_path,
            "enabled": loaded.get("image_overlay_enabled").is_some_and(truthy),
            "opacity": loaded.get("image_overlay_opacity").cloned().unwrap_or(json!(100)),
            "fit": loaded.get("image_overlay_fit").cloned().unwrap_or(json!("fit")),
            "rect": loaded.get("image_overlay_rect").cloned()
                .unwrap_or_else(|| rect_json(0.76, 0.04, 0.20, 0.24)),
        });
        data.insert("image_overlays".into(), json!([layer]));
        rewrite = true;
    }

    // (5) camera rectangle from the former corner/size settings.
    if !loaded.contains_key("camera_overlay_rect") {
        let position = loaded.get("camera_position").and_then(Value::as_str);
        let size = loaded.get("camera_size").and_then(Value::as_str);
        data.insert(
            "camera_overlay_rect".into(),
            legacy_overlay_rect(position.unwrap_or("bottom-right"), size.unwrap_or("medium")),
        );
    }
    rewrite
}

/// Python truthiness for the JSON values `bool(...)` could see.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Builds a normalized `{x, y, w, h}` rectangle value.
fn rect_json(x: f64, y: f64, w: f64, h: f64) -> Value {
    json!({"x": x, "y": y, "w": w, "h": h})
}

/// Rounds to four decimals like Python's `round(v, 4)`.
fn round4(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

/// Translates the legacy corner/size settings into a freeform rectangle
/// (`legacy_overlay_rect` plus `clamp_overlay_rect` in `camera_overlay.py`).
fn legacy_overlay_rect(position: &str, size: &str) -> Value {
    const MIN: f64 = 0.08;
    let width: f64 = match size {
        "small" => 0.20,
        "large" => 0.33,
        _ => 0.25,
    };
    // A 4:3 webcam frame inside a 16:9 canvas.
    let height = width * 1.3333;
    let margin = 0.02;
    let x = if position.contains("left") {
        margin
    } else {
        1.0 - width - margin
    };
    let y = if position.starts_with("top") {
        margin
    } else {
        1.0 - height - margin
    };
    let w = width.clamp(MIN, 0.92);
    let h = height.clamp(MIN, 0.92);
    let x = x.clamp(0.0, 1.0 - w);
    let y = y.clamp(0.0, 1.0 - h);
    rect_json(round4(x), round4(y), round4(w), round4(h))
}

/// Migrates `navigation_bindings` and its version; true when the version moved.
fn migrate_navigation(
    data: &mut Map<String, Value>,
    loaded: &Map<String, Value>,
    paths: &FthrPaths,
) -> bool {
    let version = loaded
        .get("navigation_bindings_version")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    if version >= NAV_VERSION as f64 {
        return false;
    }
    let known: Vec<String> = match super::defaults(paths).get("navigation_bindings") {
        Some(Value::Object(m)) => m.keys().cloned().collect(),
        _ => Vec::new(),
    };
    if let Some(Value::Object(b)) = data.get_mut("navigation_bindings") {
        if version < 4.0 {
            if version < 2.0 {
                for (action, old, new) in [
                    ("editor_next", "Right", "Tab+Right"),
                    ("editor_previous", "Left", "Tab+Left"),
                ] {
                    if b.get(action).and_then(Value::as_str) == Some(old) {
                        b.insert(action.into(), json!(new));
                    }
                }
            }
            if b.get("editor_minimize").and_then(Value::as_str) == Some("Escape")
                && b.get("editor_close").and_then(Value::as_str) == Some("Shift+Escape")
            {
                b.insert("editor_minimize".into(), json!("Shift+Escape"));
                b.insert("editor_close".into(), json!("Escape"));
            }
            b.retain(|k, _| known.contains(k));
        }
        if b.get("editor_split").and_then(Value::as_str) == Some("Ctrl+S") {
            b.insert("editor_split".into(), json!("Ctrl+C"));
        }
    }
    data.insert("navigation_bindings_version".into(), json!(NAV_VERSION));
    true
}
