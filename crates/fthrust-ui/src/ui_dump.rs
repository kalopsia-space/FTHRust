//! The `FTHRUST_UI_DUMP` state dump for the UI drive bench: after each
//! painted frame the page, status, error bar, front dialog and every
//! fidelity marker go to a JSON file, atomically, at most 20 times a second
//! and only when something changed. Inert unless the variable is set.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::fidelity::Marker;
use crate::shell::error_bar::Level;
use crate::state::{AppState, Page};

/// Minimum gap between two writes.
pub const MIN_GAP: Duration = Duration::from_millis(50);

/// The dump for one frame.
pub fn snapshot(
    state: &AppState,
    markers: &[Marker],
    pixels_per_point: f32,
    frame: u64,
    time: f64,
) -> Value {
    let page = match state.page {
        Page::Clips => "clips",
        Page::Settings => "settings",
    };
    let markers: Vec<Value> = markers
        .iter()
        .map(|m| {
            let mut v = json!({
                "name": m.name,
                "x": m.rect.min.x,
                "y": m.rect.min.y,
                "w": m.rect.width(),
                "h": m.rect.height(),
            });
            if let Some(t) = &m.text {
                v["text"] = Value::String(t.content.clone());
            }
            v
        })
        .collect();
    let error_bar = state.error_bar.current().map(|e| {
        json!({
            "title": e.title,
            "detail": e.detail,
            "level": match e.level { Level::Error => "error", Level::Warning => "warning" },
        })
    });
    let dialog = state
        .dialogs
        .modal
        .front()
        .map(crate::dialogs::title_of)
        .or_else(|| state.dialogs.messages.last().map(crate::dialogs::title_of));
    json!({
        "frame": frame,
        "time": time,
        "pixels_per_point": pixels_per_point,
        "page": page,
        "status": state.status.text,
        "markers": markers,
        "error_bar": error_bar,
        "dialog": dialog,
    })
}

/// Writes dumps atomically, skipping unchanged content and throttling.
pub struct Writer {
    path: PathBuf,
    last_hash: Option<u64>,
    last_write: Option<Instant>,
}

fn content_hash(value: &Value) -> u64 {
    let mut v = value.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("frame");
        o.remove("time");
    }
    let s = v.to_string();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl Writer {
    /// A writer for `path`.
    pub fn new(path: PathBuf) -> Writer {
        Writer {
            path,
            last_hash: None,
            last_write: None,
        }
    }

    /// Writes `value` unless it is unchanged or too soon; `Ok(true)` when written.
    pub fn write(&mut self, value: &Value) -> std::io::Result<bool> {
        let hash = content_hash(value);
        if self.last_hash == Some(hash) {
            return Ok(false);
        }
        if let Some(t) = self.last_write
            && t.elapsed() < MIN_GAP
        {
            return Ok(false);
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(value)?)?;
        std::fs::rename(&tmp, &self.path)?;
        self.last_hash = Some(hash);
        self.last_write = Some(Instant::now());
        Ok(true)
    }
}

/// A writer when `FTHRUST_UI_DUMP` names a file.
pub fn from_env() -> Option<Writer> {
    let path = std::env::var_os("FTHRUST_UI_DUMP")?;
    if path.is_empty() {
        return None;
    }
    Some(Writer::new(PathBuf::from(path)))
}
