//! Capture presets stored in `~/.fthr/presets.json`.

use crate::atomic_json::{Loaded, load_json, write_json_atomic};
use crate::error::Result;
use crate::fsutil;
use crate::paths::FthrPaths;
use serde_json::{Map, Value};
use std::path::PathBuf;

/// The settings a capture preset may carry.
pub const PRESET_KEYS: [&str; 8] = [
    "clip_length",
    "framerate",
    "resolution",
    "bitrate_level",
    "encoder_pref",
    "codec_pref",
    "encoder_preset",
    "audio_capture_enabled",
];

/// Keys older versions stored that must no longer be reactivated.
const RETIRED_KEYS: [&str; 4] = [
    "extended_clip_length",
    "multiband_audio_enabled",
    "audio_categories",
    "auto_crop_enabled",
];

/// Named capture presets backed by `presets.json`.
#[derive(Debug, Clone)]
pub struct PresetsStore {
    path: PathBuf,
}

impl PresetsStore {
    /// Creates a store over the presets file of `paths`.
    pub fn new(paths: &FthrPaths) -> Self {
        Self {
            path: paths.presets_file(),
        }
    }

    /// Reads all presets for display: a missing, unreadable or non-object
    /// file is empty.
    fn read(&self) -> Map<String, Value> {
        self.read_checked().unwrap_or_default()
    }

    /// Reads all presets, failing when the file cannot be read: a caller that
    /// writes back must not build the new file from nothing. A missing or
    /// non-object file is empty. Retired keys are stripped and the file
    /// rewritten when any were present.
    fn read_checked(&self) -> Result<Map<String, Value>> {
        let mut all = match load_json(&self.path) {
            Loaded::Value(Value::Object(all)) => all,
            Loaded::Missing => return Ok(Map::new()),
            // A transient read error: keep the file and report it, so a save
            // cannot replace the user's presets with just the new one.
            Loaded::Unreadable(e) => {
                tracing::warn!("could not read {}: {e}", self.path.display());
                return Err(crate::CoreError::Protected {
                    path: self.path.clone(),
                    reason: format!("it could not be read ({e})"),
                });
            }
            // Corrupt or non-object content is quarantined so a later save
            // cannot silently overwrite the user's presets.
            Loaded::Value(_) | Loaded::Corrupt => {
                if let Err(e) = fsutil::quarantine(&self.path) {
                    tracing::warn!("could not quarantine {}: {e}", self.path.display());
                }
                return Ok(Map::new());
            }
        };
        let mut removed = false;
        for values in all.values_mut() {
            if let Value::Object(m) = values {
                for key in RETIRED_KEYS {
                    removed |= m.remove(key).is_some();
                }
            }
        }
        if removed {
            let _ = write_json_atomic(&self.path, &Value::Object(all.clone()), true);
        }
        Ok(all)
    }

    fn write(&self, all: Map<String, Value>) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_json_atomic(&self.path, &Value::Object(all), true)
    }

    /// Preset names in sorted order.
    pub fn list_names(&self) -> Result<Vec<String>> {
        let mut names: Vec<String> = self.read().keys().cloned().collect();
        names.sort();
        Ok(names)
    }

    /// Loads one preset, restricted to [`PRESET_KEYS`].
    pub fn load(&self, name: &str) -> Result<Option<Map<String, Value>>> {
        Ok(match self.read().get(name) {
            Some(Value::Object(m)) => Some(filter_keys(m)),
            _ => None,
        })
    }

    /// Saves `values` under `name`, keeping only [`PRESET_KEYS`]. Fails
    /// without touching the file when it cannot be read.
    pub fn save(&self, name: &str, values: &Map<String, Value>) -> Result<()> {
        let mut all = self.read_checked()?;
        all.insert(name.to_string(), Value::Object(filter_keys(values)));
        self.write(all)
    }

    /// Deletes a preset; returns whether it existed. Fails without touching
    /// the file when it cannot be read.
    pub fn delete(&self, name: &str) -> Result<bool> {
        let mut all = self.read_checked()?;
        let existed = all.remove(name).is_some();
        self.write(all)?;
        Ok(existed)
    }
}

fn filter_keys(m: &Map<String, Value>) -> Map<String, Value> {
    m.iter()
        .filter(|(k, _)| PRESET_KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}
