//! Post-save overlays (`_apply_image_overlay`, `_apply_camera_overlay`):
//! the image layers and the webcam become one filter graph over the saved
//! clip, so the clip is re-encoded once (the reference re-encoded once per
//! overlay). Sources are `movie=` filters, so the runner reads one input.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::export::graph::{AudioOut, GraphPlan, MapRef, VideoOut};
use crate::settings::SettingsStore;

/// A normalized rectangle of the clip frame (top-left origin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    /// Left.
    pub x: f64,
    /// Top.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// `DEFAULT_OVERLAY_RECT` (camera).
pub const CAMERA_DEFAULT: Rect = Rect {
    x: 0.72,
    y: 0.64,
    w: 0.25,
    h: 0.34,
};

/// `MIN_OVERLAY_SIZE`.
const MIN_SIZE: f64 = 0.08;

/// New-layer start corners, by index modulo 5 (`_IMAGE_LAYER_POSITIONS`).
const SLOTS: [(f64, f64); 5] = [
    (0.76, 0.04),
    (0.04, 0.04),
    (0.04, 0.72),
    (0.76, 0.72),
    (0.40, 0.38),
];

fn round4(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

/// `clamp_overlay_rect`: each field parsed (the default's on failure),
/// sizes clamped to 0.08–0.92, then positions inside the frame, rounded to
/// four decimals.
pub fn clamp_rect(value: &Value, default: Rect) -> Rect {
    let field = |k: &str, d: f64| {
        value
            .get(k)
            .and_then(|v| match v {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => s.trim().parse().ok(),
                Value::Bool(b) => Some(f64::from(u8::from(*b))),
                _ => None,
            })
            .filter(|f| f.is_finite())
            .unwrap_or(d)
    };
    let (x, y, w, h) = if value.is_object() {
        (
            field("x", default.x),
            field("y", default.y),
            field("w", default.w),
            field("h", default.h),
        )
    } else {
        (default.x, default.y, default.w, default.h)
    };
    let w = w.clamp(MIN_SIZE, 0.92);
    let h = h.clamp(MIN_SIZE, 0.92);
    Rect {
        x: round4(x.clamp(0.0, 1.0 - w)),
        y: round4(y.clamp(0.0, 1.0 - h)),
        w: round4(w),
        h: round4(h),
    }
}

/// One image layer to burn.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageLayer {
    /// The image file.
    pub path: PathBuf,
    /// Opacity percent, 10–100.
    pub opacity: u8,
    /// `fill` (centre crop) instead of `fit` (transparent letterbox).
    pub fill: bool,
    /// Placement.
    pub rect: Rect,
}

/// Python's `bool(value)` for JSON.
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

fn slot_rect(index: usize) -> Rect {
    let (x, y) = SLOTS[index % SLOTS.len()];
    Rect {
        x,
        y,
        w: 0.20,
        h: 0.24,
    }
}

/// `image_overlay_layers`, enabled layers only: the global switch and each
/// layer's own flag; empty paths and non-objects skipped; opacity 10–100;
/// a broken rect falls back to that index's start slot.
pub fn image_layers(settings: &SettingsStore) -> Vec<ImageLayer> {
    let raw = settings.get("image_overlays").and_then(Value::as_array);
    let global = settings
        .get_bool("image_overlay_enabled")
        .unwrap_or_else(|| raw.is_some_and(|r| !r.is_empty()));
    if !global {
        return Vec::new();
    }
    let Some(raw) = raw else {
        return Vec::new();
    };
    raw.iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let obj = item.as_object()?;
            let path = obj
                .get("path")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())?;
            if obj.get("enabled").is_some_and(|v| !truthy(v)) {
                return None;
            }
            let opacity = obj
                .get("opacity")
                .and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                })
                .map_or(100.0, f64::trunc)
                .clamp(10.0, 100.0) as u8;
            Some(ImageLayer {
                path: PathBuf::from(path),
                opacity,
                fill: obj.get("fit").and_then(Value::as_str) == Some("fill"),
                rect: clamp_rect(obj.get("rect").unwrap_or(&Value::Null), slot_rect(i)),
            })
        })
        .collect()
}

/// The saved camera rectangle (the legacy corner/size keys are migrated at
/// settings load).
pub fn camera_rect(settings: &SettingsStore) -> Rect {
    clamp_rect(
        settings.get("camera_overlay_rect").unwrap_or(&Value::Null),
        CAMERA_DEFAULT,
    )
}

/// `(x, y, w, h)` in pixels: `int()` truncation, sizes at least 64×48 and
/// even.
pub fn pixels(r: Rect, (cw, ch): (u32, u32)) -> (u32, u32, u32, u32) {
    let (cw, ch) = (f64::from(cw), f64::from(ch));
    let w = ((cw * r.w) as u32).max(64) & !1;
    let h = ((ch * r.h) as u32).max(48) & !1;
    ((cw * r.x).max(0.0) as u32, (ch * r.y).max(0.0) as u32, w, h)
}

/// A path as a `movie=` option value: escaped for the option parser
/// (`\ ' :`), then for the graph parser (`\ ' [ ] , ;`), unquoted (an
/// apostrophe inside quotes cannot be escaped).
pub fn movie_path(path: &Path) -> String {
    let escape = |s: &str, special: &str| {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            if special.contains(c) {
                out.push('\\');
            }
            out.push(c);
        }
        out
    };
    escape(&escape(&path.to_string_lossy(), "\\':"), "\\'[],;")
}

fn movie(path: &Path) -> String {
    format!("movie=filename={}", movie_path(path))
}

/// The burn graph: image layers in order, then the camera (a recorded
/// segment) on top. `None` when there is nothing to burn.
pub fn plan(
    layers: &[ImageLayer],
    camera: Option<&Path>,
    camera_rect: Rect,
    clip: (u32, u32),
) -> Option<GraphPlan> {
    let total = layers.len() + usize::from(camera.is_some());
    if total == 0 {
        return None;
    }
    let mut parts = Vec::new();
    let mut prev = "0:v".to_owned();
    let label = |n: usize, name: String| {
        if n + 1 == total {
            "vout".to_owned()
        } else {
            name
        }
    };
    for (i, l) in layers.iter().enumerate() {
        let (x, y, w, h) = pixels(l.rect, clip);
        let shape = if l.fill {
            format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}")
        } else {
            format!(
                "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=0x00000000"
            )
        };
        let alpha = (f64::from(l.opacity) / 100.0).clamp(0.10, 1.0);
        parts.push(format!(
            "{},format=rgba,{shape},colorchannelmixer=aa={alpha:.3}[image{i}]",
            movie(&l.path)
        ));
        let out = label(i, format!("layer{i}"));
        parts.push(format!(
            "[{prev}][image{i}]overlay={x}:{y}:eof_action=repeat[{out}]"
        ));
        prev = out;
    }
    if let Some(cam) = camera {
        let (x, y, w, h) = pixels(camera_rect, clip);
        parts.push(format!(
            "{},scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2[cam]",
            movie(cam)
        ));
        parts.push(format!(
            "[{prev}][cam]overlay={x}:{y}:eof_action=repeat[vout]"
        ));
    }
    Some(GraphPlan {
        seek_s: None,
        duration_s: None,
        filter_graph: Some(parts.join(";")),
        video_map: MapRef::Label("vout".into()),
        audio_map: Some(MapRef::AllAudioOptional),
        video: VideoOut::Encode,
        audio: AudioOut::Copy,
        audio_filter: None,
        shortest: false,
    })
}
