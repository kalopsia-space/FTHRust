//! Editor snapshots, the undo stack and the draft `state` codec
//! (`EditorSnapshot`, `_editor_snapshot_to_draft`,
//! `_editor_snapshot_from_draft`, `ui/clip_viewer.py:372-500`).

use serde_json::{Value, json};

use super::timeline::{Segment, Timeline};

/// Undo depth.
pub const UNDO_LIMIT: usize = 100;

/// Colour and effect values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// −100..100.
    pub exposure: i32,
    /// −100..100.
    pub contrast: i32,
    /// −100..100.
    pub saturation: i32,
    /// −100..100.
    pub temperature: i32,
    /// 0..100.
    pub sharpness: i32,
    /// 0..100.
    pub vignette: i32,
}

/// `(key, min, max)` in the reference's order.
pub const EFFECT_LIMITS: [(&str, i32, i32); 6] = [
    ("exposure", -100, 100),
    ("contrast", -100, 100),
    ("saturation", -100, 100),
    ("temperature", -100, 100),
    ("sharpness", 0, 100),
    ("vignette", 0, 100),
];

impl Effects {
    /// The value of `key` (one of `EFFECT_LIMITS`).
    pub fn get(&self, key: &str) -> i32 {
        match key {
            "exposure" => self.exposure,
            "contrast" => self.contrast,
            "saturation" => self.saturation,
            "temperature" => self.temperature,
            "sharpness" => self.sharpness,
            "vignette" => self.vignette,
            _ => 0,
        }
    }

    /// Sets `key`, clamped to its range.
    pub fn set(&mut self, key: &str, value: i32) {
        let Some(&(_, lo, hi)) = EFFECT_LIMITS.iter().find(|(k, _, _)| *k == key) else {
            return;
        };
        let v = value.clamp(lo, hi);
        match key {
            "exposure" => self.exposure = v,
            "contrast" => self.contrast = v,
            "saturation" => self.saturation = v,
            "temperature" => self.temperature = v,
            "sharpness" => self.sharpness = v,
            _ => self.vignette = v,
        }
    }

    /// Every value clamped to its range.
    pub fn clamped(&self) -> Effects {
        let mut out = Effects::default();
        for (key, _, _) in EFFECT_LIMITS {
            out.set(key, self.get(key));
        }
        out
    }

    /// All zero.
    pub fn is_neutral(&self) -> bool {
        *self == Effects::default()
    }
}

/// One reversible edit state; playback position and the view are excluded.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// Trim start.
    pub trim_start: f64,
    /// Trim end.
    pub trim_end: f64,
    /// Segments.
    pub segments: Vec<Segment>,
    /// Selected segment.
    pub selected: usize,
    /// Crop `(x, y, w, h)` in source pixels.
    pub crop: Option<(u32, u32, u32, u32)>,
    /// Horizontal stretch ratio, 0.25..4.
    pub stretch: f64,
    /// Colour and effects.
    pub effects: Effects,
    /// Playback/export speed, 0.25..2.
    pub speed: f64,
    /// Keep audio pitch while changing speed.
    pub preserve_pitch: bool,
}

impl Snapshot {
    /// The state of `t` plus the non-timeline edit fields.
    pub fn from_timeline(
        t: &Timeline,
        crop: Option<(u32, u32, u32, u32)>,
        stretch: f64,
        effects: Effects,
        speed: f64,
        preserve_pitch: bool,
    ) -> Snapshot {
        Snapshot {
            trim_start: t.trim_start,
            trim_end: t.trim_end,
            segments: t.segments.clone(),
            selected: t.selected,
            crop,
            stretch,
            effects,
            speed,
            preserve_pitch,
        }
    }

    /// Writes the timeline part into `t` (the view is left alone).
    pub fn apply_to(&self, t: &mut Timeline) {
        t.trim_start = self.trim_start;
        t.trim_end = self.trim_end;
        t.segments = self.segments.clone();
        t.selected = self.selected.min(self.segments.len().saturating_sub(1));
    }
}

/// Undo and redo stacks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UndoStack {
    /// Older states, newest last.
    pub undo: Vec<Snapshot>,
    /// Undone states, newest last.
    pub redo: Vec<Snapshot>,
}

impl UndoStack {
    /// Records `before` when it differs from `current` (`_commit_editor_change`).
    pub fn commit(&mut self, before: Snapshot, current: &Snapshot) -> bool {
        if before == *current {
            return false;
        }
        self.undo.push(before);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        true
    }

    /// The state to restore, saving `current` for redo.
    pub fn undo(&mut self, current: &Snapshot) -> Option<Snapshot> {
        let s = self.undo.pop()?;
        self.redo.push(current.clone());
        Some(s)
    }

    /// The state to re-apply, saving `current` for undo.
    pub fn redo(&mut self, current: &Snapshot) -> Option<Snapshot> {
        let s = self.redo.pop()?;
        self.undo.push(current.clone());
        Some(s)
    }

    /// Something to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Something to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

/// The draft `state` object.
pub fn encode_state(s: &Snapshot) -> Value {
    let effects: serde_json::Map<String, Value> = EFFECT_LIMITS
        .iter()
        .map(|(k, _, _)| ((*k).to_owned(), json!(s.effects.get(k))))
        .collect();
    json!({
        "trim_start": s.trim_start,
        "trim_end": s.trim_end,
        "segments": s.segments.iter().map(|g| json!([g.start, g.end, g.deleted])).collect::<Vec<_>>(),
        "selected_segment": s.selected,
        "crop_rect": s.crop.map(|(x, y, w, h)| json!([x, y, w, h])),
        "stretch_ratio": s.stretch,
        "effects": effects,
        "speed_rate": s.speed,
        "preserve_pitch": s.preserve_pitch,
    })
}

/// Python `float()` of a JSON value: numbers, and numeric strings.
fn as_float(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

/// Python `int()` of a JSON value (floats truncate).
fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// Python truthiness for the segment `deleted` flag.
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

/// Validates a draft `state`; `None` means "use the defaults".
pub fn decode_state(v: &Value) -> Option<Snapshot> {
    let o = v.as_object()?;
    let num = |key: &str, default: f64| match o.get(key) {
        None => Some(default),
        Some(x) => as_float(x),
    };
    let trim_start = num("trim_start", 0.0)?;
    let trim_end = num("trim_end", 1.0)?;
    let stretch = num("stretch_ratio", 1.0)?;
    let speed = num("speed_rate", 1.0)?;
    if ![trim_start, trim_end, stretch, speed]
        .iter()
        .all(|f| f.is_finite())
        || !(0.0 <= trim_start && trim_start < trim_end && trim_end <= 1.0)
        || !(0.25..=4.0).contains(&stretch)
        || !(0.25..=2.0).contains(&speed)
    {
        return None;
    }
    let raw = o.get("segments")?.as_array()?;
    if !(1..=500).contains(&raw.len()) {
        return None;
    }
    let mut segments = Vec::with_capacity(raw.len());
    let mut previous_end = 0.0;
    for (i, r) in raw.iter().enumerate() {
        let r = r.as_array().filter(|r| r.len() == 3)?;
        let start = as_float(&r[0])?;
        let end = as_float(&r[1])?;
        if !start.is_finite()
            || !end.is_finite()
            || start < 0.0
            || end > 1.0
            || end <= start
            || (i == 0 && start.abs() > 0.0001)
            || (i > 0 && (start - previous_end).abs() > 0.0001)
        {
            return None;
        }
        segments.push(Segment {
            start,
            end,
            deleted: truthy(&r[2]),
        });
        previous_end = end;
    }
    if (previous_end - 1.0).abs() > 0.0001 || segments.iter().all(|s| s.deleted) {
        return None;
    }
    let selected = o
        .get("selected_segment")
        .map_or(Some(0), as_int)
        .unwrap_or(0)
        .clamp(0, segments.len() as i64 - 1) as usize;
    let crop = o
        .get("crop_rect")
        .and_then(Value::as_array)
        .filter(|c| c.len() == 4)
        .and_then(|c| {
            let v: Option<Vec<i64>> = c.iter().map(as_int).collect();
            v
        })
        .filter(|c| {
            (0..=100_000).contains(&c[0])
                && (0..=100_000).contains(&c[1])
                && (2..=100_000).contains(&c[2])
                && (2..=100_000).contains(&c[3])
        })
        .map(|c| (c[0] as u32, c[1] as u32, c[2] as u32, c[3] as u32));
    let raw_effects = o.get("effects").and_then(Value::as_object);
    let mut effects = Effects::default();
    for (key, lo, hi) in EFFECT_LIMITS {
        let value = raw_effects
            .and_then(|e| e.get(key))
            .and_then(as_int)
            .unwrap_or(0)
            .clamp(i64::from(lo), i64::from(hi));
        effects.set(key, value as i32);
    }
    let preserve_pitch = o
        .get("preserve_pitch")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    Some(Snapshot {
        trim_start,
        trim_end,
        segments,
        selected,
        crop,
        stretch,
        effects,
        speed,
        preserve_pitch,
    })
}
