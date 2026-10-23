//! Speed presets and crop helpers (`SPEED_PRESETS`, `_normalise_speed`,
//! `_apply_quick_crop`, `_save_quick_crop`).

use serde_json::{Value, json};

/// Speed combo entries: label (two spaces after the `×`) and rate.
pub const SPEED_PRESETS: [(&str, f64); 7] = [
    ("0.25×  SLOW", 0.25),
    ("0.5×  SLOW", 0.5),
    ("0.75×  SLOW", 0.75),
    ("1×  NORMAL", 1.0),
    ("1.25×  FAST", 1.25),
    ("1.5×  FAST", 1.5),
    ("2×  FAST", 2.0),
];

/// Index of the 1× preset.
pub const DEFAULT_SPEED_INDEX: usize = 3;

/// Clamps to 0.25..2; non-finite or non-positive becomes 1.
pub fn normalise_speed(rate: f64) -> f64 {
    if !rate.is_finite() || rate <= 0.0 {
        return 1.0;
    }
    rate.clamp(0.25, 2.0)
}

/// The `quick_crop` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuickCrop {
    /// Left.
    pub x: u32,
    /// Top.
    pub y: u32,
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
    /// Source width it was saved on.
    pub src_w: u32,
    /// Source height it was saved on.
    pub src_h: u32,
}

impl QuickCrop {
    /// Reads the setting; `None` when absent or malformed.
    pub fn from_setting(v: &Value) -> Option<QuickCrop> {
        let o = v.as_object()?;
        let f = |k: &str| o.get(k)?.as_u64().map(|n| n as u32);
        Some(QuickCrop {
            x: f("x")?,
            y: f("y")?,
            w: f("w")?,
            h: f("h")?,
            src_w: f("src_w")?,
            src_h: f("src_h")?,
        })
    }

    /// The setting value.
    pub fn to_setting(&self) -> Value {
        json!({"x": self.x, "y": self.y, "w": self.w, "h": self.h,
               "src_w": self.src_w, "src_h": self.src_h})
    }

    /// The crop scaled to a `src_w`×`src_h` source; `None` unless both
    /// sides exceed 4.
    pub fn apply(&self, src_w: u32, src_h: u32) -> Option<(u32, u32, u32, u32)> {
        let sx = f64::from(src_w) / f64::from(self.src_w.max(1));
        let sy = f64::from(src_h) / f64::from(self.src_h.max(1));
        let x = (f64::from(self.x) * sx) as i64;
        let y = (f64::from(self.y) * sy) as i64;
        let w = ((f64::from(self.w) * sx) as i64).min(i64::from(src_w) - x);
        let h = ((f64::from(self.h) * sy) as i64).min(i64::from(src_h) - y);
        (w > 4 && h > 4).then_some((x as u32, y as u32, w as u32, h as u32))
    }
}

/// Fits a crop into a `src_w`×`src_h` source; `None` when a side drops
/// under 2.
pub fn clamp_crop(
    crop: (u32, u32, u32, u32),
    src_w: u32,
    src_h: u32,
) -> Option<(u32, u32, u32, u32)> {
    let (x, y, w, h) = crop;
    let x = x.min(src_w);
    let y = y.min(src_h);
    let w = w.min(src_w - x);
    let h = h.min(src_h - y);
    (w >= 2 && h >= 2).then_some((x, y, w, h))
}
