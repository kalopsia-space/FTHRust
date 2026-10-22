//! Theme store: colour tokens, icon tints, custom assets and migrations.
//!
//! Mirrors the Python `ThemeManager` persisted in `~/.fthr/theme/theme.json`.

mod archive;
mod defaults;
mod migrate;

pub use defaults::*;
pub use migrate::BUILTIN_FONT_FAMILIES;

use crate::atomic_json::{read_json, write_json_atomic};
use crate::error::{CoreError, Result};
use crate::paths::FthrPaths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const ICON_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "ico", "svg"];
const SOUND_EXTS: &[&str] = &["mp3", "wav", "ogg", "flac", "m4a", "wma", "aac"];
const FONT_EXTS: &[&str] = &["ttf", "otf"];

/// Returns true for `#` followed by exactly six hex digits (any case).
pub fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Persisted theme document; field order matches the Python `theme.json`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ThemeData {
    /// Palette generation (current: 4).
    pub palette_version: u32,
    /// Typography generation (current: 2).
    pub typography_version: u32,
    /// Colour token to `#rrggbb`.
    pub colors: BTreeMap<String, String>,
    /// Logical icon name to custom file in the icons folder.
    pub icons: BTreeMap<String, String>,
    /// Sound key to custom file in the sounds folder.
    pub sounds: BTreeMap<String, String>,
    /// `_global` or per-icon tint colour.
    pub icon_tints: BTreeMap<String, String>,
    /// Capture-card colours.
    pub capture_card: BTreeMap<String, String>,
    /// Font role (`body`, `display`) to family.
    pub fonts: BTreeMap<String, String>,
    /// Imported family name to file in the fonts folder.
    pub font_files: BTreeMap<String, String>,
}

impl Default for ThemeData {
    fn default() -> Self {
        Self {
            palette_version: 4,
            typography_version: 2,
            colors: default_colors(),
            icons: BTreeMap::new(),
            sounds: BTreeMap::new(),
            icon_tints: BTreeMap::from([("_global".to_string(), DEFAULT_ICON_TINT.to_string())]),
            capture_card: default_capture_card_colors(),
            fonts: default_fonts(),
            font_files: BTreeMap::new(),
        }
    }
}

/// Theme data plus the on-disk theme folders.
pub struct ThemeStore {
    paths: FthrPaths,
    /// Current in-memory theme.
    pub data: ThemeData,
}

fn lower_ext(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn check_ext(source: &Path, allowed: &[&str], what: &str) -> Result<String> {
    let ext = lower_ext(source);
    if allowed.contains(&ext.as_str()) {
        Ok(ext)
    } else {
        Err(CoreError::Invalid(format!(
            "unsupported {what} file type: {}",
            source.display()
        )))
    }
}

/// Copies `source` to `dest` unless both name the same file (copying a file
/// onto itself would truncate it).
fn copy_unless_same(source: &Path, dest: &Path) -> Result<()> {
    if let (Ok(a), Ok(b)) = (source.canonicalize(), dest.canonicalize())
        && a == b
    {
        return Ok(());
    }
    std::fs::copy(source, dest)?;
    Ok(())
}

impl ThemeStore {
    /// Creates the theme folders and loads `theme.json`.
    ///
    /// A missing or unparsable file yields defaults (no backup is made).
    /// Migrations run in memory only; the file is not rewritten.
    pub fn load(paths: FthrPaths) -> Result<Self> {
        for d in [
            paths.theme_dir(),
            paths.theme_icons_dir(),
            paths.theme_sounds_dir(),
            paths.theme_fonts_dir(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        let data = match read_json(&paths.theme_file()) {
            Ok(Some(v)) if v.is_object() => migrate::merge_and_migrate(&v),
            Ok(_) | Err(CoreError::Json { .. }) => ThemeData::default(),
            Err(e) => return Err(e),
        };
        Ok(Self { paths, data })
    }

    /// Writes `theme.json` atomically (pretty, via `theme.json.tmp`).
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(self.paths.theme_dir())?;
        let value = serde_json::to_value(&self.data).map_err(|source| CoreError::Json {
            path: self.paths.theme_file(),
            source,
        })?;
        write_json_atomic(&self.paths.theme_file(), &value, true)
    }

    /// Colour for `token`, falling back to the shipped default (empty if unknown).
    pub fn color(&self, token: &str) -> &str {
        self.data
            .colors
            .get(token)
            .map(String::as_str)
            .or_else(|| {
                DEFAULT_COLOR_TABLE
                    .iter()
                    .find(|(k, _)| *k == token)
                    .map(|(_, v)| *v)
            })
            .unwrap_or("")
    }

    /// Capture-card colour for `key`; `#ffffff` when unknown.
    pub fn capture_card_color(&self, key: &str) -> &str {
        self.data
            .capture_card
            .get(key)
            .map(String::as_str)
            .or_else(|| {
                DEFAULT_CAPTURE_CARD_TABLE
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| *v)
            })
            .unwrap_or("#ffffff")
    }

    /// Tint for a bundled icon: per-file, else `_global`, else the default.
    ///
    /// `favicon.ico` is never tinted and returns `""`.
    pub fn icon_tint(&self, logical: &str) -> &str {
        if logical == "favicon.ico" {
            return "";
        }
        let tints = &self.data.icon_tints;
        tints
            .get(logical)
            .or_else(|| tints.get("_global"))
            .map(String::as_str)
            .unwrap_or(DEFAULT_ICON_TINT)
    }

    /// Tint for a user-imported icon: only an explicit per-file override.
    ///
    /// Custom icons are drawn untinted unless this returns `Some`.
    pub fn icon_tint_for_custom(&self, logical: &str) -> Option<&str> {
        self.data.icon_tints.get(logical).map(String::as_str)
    }

    /// Existing custom icon file for `logical`, if any.
    pub fn custom_icon_path(&self, logical: &str) -> Option<PathBuf> {
        let p = self
            .paths
            .theme_icons_dir()
            .join(self.data.icons.get(logical)?);
        p.exists().then_some(p)
    }

    /// Existing custom sound file for `key`, if any.
    pub fn custom_sound_path(&self, key: &str) -> Option<PathBuf> {
        let p = self
            .paths
            .theme_sounds_dir()
            .join(self.data.sounds.get(key)?);
        p.exists().then_some(p)
    }

    /// Copies `source` into the icons folder as `<stem of logical>.<source ext>`.
    pub fn set_custom_icon(&mut self, logical: &str, source: &Path) -> Result<()> {
        let ext = check_ext(source, ICON_EXTS, "icon")?;
        let stem = Path::new(logical)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| logical.to_string());
        let name = format!("{stem}.{ext}");
        copy_unless_same(source, &self.paths.theme_icons_dir().join(&name))?;
        self.data.icons.insert(logical.to_string(), name);
        Ok(())
    }

    /// Copies `source` into the sounds folder as `<key>.<source ext>`.
    pub fn set_custom_sound(&mut self, key: &str, source: &Path) -> Result<()> {
        if !CUSTOMIZABLE_SOUNDS.contains(&key) {
            return Err(CoreError::Invalid(format!("unknown sound key: {key}")));
        }
        let ext = check_ext(source, SOUND_EXTS, "sound")?;
        let name = format!("{key}.{ext}");
        copy_unless_same(source, &self.paths.theme_sounds_dir().join(&name))?;
        self.data.sounds.insert(key.to_string(), name);
        Ok(())
    }

    /// Copies a `.ttf`/`.otf` into the fonts folder and registers `families`.
    pub fn set_custom_font(&mut self, families: &[String], source: &Path) -> Result<()> {
        check_ext(source, FONT_EXTS, "font")?;
        let families: Vec<&str> = families
            .iter()
            .map(|f| f.trim())
            .filter(|f| !f.is_empty())
            .collect();
        if families.is_empty() {
            return Err(CoreError::Invalid(
                "the font does not expose a usable family name".into(),
            ));
        }
        let name = source
            .file_name()
            .ok_or_else(|| CoreError::Invalid("font path has no file name".into()))?
            .to_string_lossy()
            .into_owned();
        copy_unless_same(source, &self.paths.theme_fonts_dir().join(&name))?;
        for f in families {
            self.data.font_files.insert(f.to_string(), name.clone());
        }
        Ok(())
    }

    /// Imported `(family, path)` pairs whose files still exist.
    pub fn custom_font_paths(&self) -> Vec<(String, PathBuf)> {
        self.data
            .font_files
            .iter()
            .filter_map(|(family, file)| {
                let name = Path::new(file).file_name()?;
                let p = self.paths.theme_fonts_dir().join(name);
                p.is_file().then(|| (family.clone(), p))
            })
            .collect()
    }

    /// True when anything differs from the shipped theme.
    pub fn has_any_customization(&self) -> bool {
        self.data != ThemeData::default()
    }

    /// Resets in-memory data to defaults; imported files stay on disk.
    ///
    /// Unlike the Python, this does not call `save()`; callers do.
    pub fn reset_all(&mut self) {
        self.data = ThemeData::default();
    }
}
