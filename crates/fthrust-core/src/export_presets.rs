//! Export presets stored in `~/.fthr/export_presets.json`.

use crate::atomic_json::{read_json, write_json_atomic};
use crate::error::{CoreError, Result};
use crate::paths::FthrPaths;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

const DISCORD_ID: &str = "discord";

fn default_codec() -> String {
    "h264".into()
}
fn default_audio() -> u32 {
    128
}
fn default_margin() -> f64 {
    0.94
}

/// A target-size export profile.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ExportPreset {
    /// Stable identifier (a slug of the name when created by the UI).
    pub preset_id: String,
    /// Display name.
    pub name: String,
    /// Target file size in megabytes (1..=10000).
    pub target_size_mb: f64,
    /// Target width in pixels; must be set together with the height.
    #[serde(default)]
    pub target_width: Option<u32>,
    /// Target height in pixels; must be set together with the width.
    #[serde(default)]
    pub target_height: Option<u32>,
    /// Target frame rate (1..=240); `0` or absent means keep the source rate.
    #[serde(default)]
    pub target_fps: Option<f64>,
    /// `h264` or `source`.
    #[serde(default = "default_codec")]
    pub video_codec: String,
    /// Audio bitrate in kbps (32..=512).
    #[serde(default = "default_audio")]
    pub audio_bitrate_kbps: u32,
    /// Whether the preset ships with the app and cannot be replaced.
    #[serde(default)]
    pub built_in: bool,
    /// Fraction of the target size actually aimed for (0.80..=0.99).
    #[serde(default = "default_margin")]
    pub safety_margin: f64,
}

/// Treats `None` and `0` as unset, range-checking anything else.
fn dimension(value: Option<u32>, label: &str) -> std::result::Result<Option<u32>, String> {
    match value {
        None | Some(0) => Ok(None),
        Some(n) if (16..=8192).contains(&n) => Ok(Some(n & !1)),
        Some(_) => Err(format!("Target {label} must be between 16 and 8192 pixels")),
    }
}

impl ExportPreset {
    /// Checks every field against the same rules as the Python `validated()`.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Preset name is required".into());
        }
        if !self.target_size_mb.is_finite() || !(1.0..=10_000.0).contains(&self.target_size_mb) {
            return Err("Target file size must be between 1 MB and 10,000 MB".into());
        }
        let w = dimension(self.target_width, "width")?;
        let h = dimension(self.target_height, "height")?;
        if w.is_some() != h.is_some() {
            return Err("Target width and height must be set together".into());
        }
        if let Some(fps) = self.target_fps
            && fps != 0.0
            && (!fps.is_finite() || !(1.0..=240.0).contains(&fps))
        {
            return Err("Target FPS must be between 1 and 240".into());
        }
        let codec = if self.video_codec.is_empty() {
            "h264".to_string()
        } else {
            self.video_codec.to_lowercase()
        };
        if codec != "h264" && codec != "source" {
            return Err("This installation supports H.264 export only".into());
        }
        if !(32..=512).contains(&self.audio_bitrate_kbps) {
            return Err("Audio bitrate must be between 32 and 512 kbps".into());
        }
        if !(0.80..=0.99).contains(&self.safety_margin) {
            return Err("Safety margin must be between 80% and 99%".into());
        }
        Ok(())
    }

    /// The built-in Discord preset (10 MB, the Linux value).
    pub fn builtin_discord() -> Self {
        Self {
            preset_id: DISCORD_ID.into(),
            name: "Discord \u{b7} 10 MB".into(),
            target_size_mb: 10.0,
            target_width: None,
            target_height: None,
            target_fps: Some(60.0),
            video_codec: "h264".into(),
            audio_bitrate_kbps: 96,
            built_in: true,
            safety_margin: 0.93,
        }
    }

    /// The ad-hoc 200 MB preset used for Catbox uploads.
    pub fn catbox_adhoc() -> Self {
        Self {
            preset_id: "upload-catbox".into(),
            name: "Catbox upload".into(),
            target_size_mb: 200.0,
            target_width: None,
            target_height: None,
            target_fps: Some(60.0),
            video_codec: "h264".into(),
            audio_bitrate_kbps: 128,
            built_in: false,
            safety_margin: 0.94,
        }
    }

    /// Lowercase alphanumeric slug with single dashes, at most 64 characters.
    pub fn slug(name: &str) -> String {
        let text: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_lowercase().collect::<String>()
                } else {
                    "-".into()
                }
            })
            .collect();
        let joined = text
            .split('-')
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let cut: String = joined.chars().take(64).collect();
        if cut.is_empty() { "preset".into() } else { cut }
    }

    /// Applies the Python normalisation: trimmed name, derived id, even
    /// dimensions, lowercase codec, zero fps/dimensions become unset.
    fn normalized(&self) -> Self {
        let name = self.name.trim().to_string();
        let id = self.preset_id.trim();
        Self {
            preset_id: if id.is_empty() {
                Self::slug(&name)
            } else {
                id.to_string()
            },
            target_width: self.target_width.filter(|n| *n != 0).map(|n| n & !1),
            target_height: self.target_height.filter(|n| *n != 0).map(|n| n & !1),
            target_fps: self.target_fps.filter(|f| *f != 0.0),
            video_codec: if self.video_codec.is_empty() {
                "h264".into()
            } else {
                self.video_codec.to_lowercase()
            },
            name,
            ..self.clone()
        }
    }
}

/// Built-in plus user-created export presets.
#[derive(Debug, Clone)]
pub struct ExportPresetStore {
    path: PathBuf,
}

impl ExportPresetStore {
    /// Creates a store over the export presets file of `paths`.
    pub fn new(paths: &FthrPaths) -> Self {
        Self {
            path: paths.export_presets_file(),
        }
    }

    /// Built-in Discord preset followed by the custom presets in file order.
    pub fn all(&self) -> Result<Vec<ExportPreset>> {
        let mut v = vec![ExportPreset::builtin_discord()];
        v.extend(self.custom()?);
        Ok(v)
    }

    /// Custom presets in file order. A missing or unreadable file is empty;
    /// invalid entries and duplicate (or `discord`) ids are skipped.
    pub fn custom(&self) -> Result<Vec<ExportPreset>> {
        Ok(self.read_entries().0)
    }

    /// Reads the file into the presets this version understands and the raw
    /// entries it does not (unparseable or invalid), so a rewrite can carry
    /// the latter through untouched. Duplicate and `discord` ids are dropped.
    fn read_entries(&self) -> (Vec<ExportPreset>, Vec<Value>) {
        let Ok(Some(Value::Array(items))) = read_json(&self.path) else {
            return (Vec::new(), Vec::new());
        };
        let mut seen = vec![DISCORD_ID.to_string()];
        let mut out = Vec::new();
        let mut preserved = Vec::new();
        for item in items {
            let parsed = serde_json::from_value::<ExportPreset>(item.clone())
                .ok()
                .map(|p| p.normalized())
                .filter(|p| p.validate().is_ok());
            let Some(p) = parsed else {
                if item.get("preset_id").and_then(Value::as_str) != Some(DISCORD_ID) {
                    preserved.push(item);
                }
                continue;
            };
            if seen.contains(&p.preset_id) {
                continue;
            }
            seen.push(p.preset_id.clone());
            out.push(ExportPreset {
                built_in: false,
                ..p
            });
        }
        (out, preserved)
    }

    /// Looks a preset up by id, built-ins included.
    pub fn get(&self, id: &str) -> Result<Option<ExportPreset>> {
        Ok(self.all()?.into_iter().find(|p| p.preset_id == id))
    }

    /// Saves a custom preset, replacing any with the same id; returns the
    /// normalised preset. Built-ins and the `discord` id are refused.
    pub fn save(&self, preset: &ExportPreset) -> Result<ExportPreset> {
        let p = preset.normalized();
        p.validate().map_err(CoreError::Invalid)?;
        if p.preset_id == DISCORD_ID || p.built_in {
            return Err(CoreError::Invalid(
                "Built-in presets cannot be replaced".into(),
            ));
        }
        let (mut list, preserved) = self.read_entries();
        list.retain(|x| x.preset_id != p.preset_id);
        list.push(p.clone());
        self.write(&list, &preserved)?;
        Ok(p)
    }

    /// Deletes a custom preset; returns whether it existed.
    pub fn delete(&self, id: &str) -> Result<bool> {
        if id == DISCORD_ID {
            return Ok(false);
        }
        let (list, preserved) = self.read_entries();
        let remaining: Vec<_> = list.iter().filter(|p| p.preset_id != id).cloned().collect();
        if remaining.len() == list.len() {
            return Ok(false);
        }
        self.write(&remaining, &preserved)?;
        Ok(true)
    }

    /// Writes the understood presets followed by the preserved raw entries.
    fn write(&self, list: &[ExportPreset], preserved: &[Value]) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut items = serde_json::to_value(list)
            .map_err(|source| CoreError::Json {
                path: self.path.clone(),
                source,
            })?
            .as_array()
            .cloned()
            .unwrap_or_default();
        items.extend(preserved.iter().cloned());
        write_json_atomic(&self.path, &Value::Array(items), true)
    }
}
