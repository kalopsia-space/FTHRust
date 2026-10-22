//! Theme zip export and import.

/// Largest single asset (icon, sound or font) accepted from a theme archive.
pub const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;

use super::{ThemeData, ThemeStore};
use super::{default_capture_card_colors, default_colors, default_fonts, migrate};
use crate::error::{CoreError, Result};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

fn zip_err(e: zip::result::ZipError) -> CoreError {
    match e {
        zip::result::ZipError::Io(e) => CoreError::Io(e),
        other => CoreError::Invalid(format!("invalid theme archive: {other}")),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Folder {
    Icons,
    Sounds,
    Fonts,
}

/// Maps a zip entry name to a destination folder and plain file name.
///
/// Anything but `icons/<file>`, `sounds/<file>`, `fonts/<file>` is rejected.
fn classify(name: &str) -> Option<(Folder, String)> {
    let (dir, file) = name.split_once('/')?;
    let folder = match dir {
        "icons" => Folder::Icons,
        "sounds" => Folder::Sounds,
        "fonts" => Folder::Fonts,
        _ => return None,
    };
    is_plain_file_name(file).then(|| (folder, file.to_string()))
}

/// True for a bare file name: non-empty, not `.`/`..`, no separators or NUL.
pub(super) fn is_plain_file_name(name: &str) -> bool {
    !(name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
        || Path::new(name).is_absolute())
}

/// Keeps only entries whose value is a plain name present in `present`.
fn retain_present(
    map: BTreeMap<String, String>,
    present: &std::collections::BTreeSet<String>,
) -> BTreeMap<String, String> {
    map.into_iter()
        .filter(|(_, v)| is_plain_file_name(v) && present.contains(v))
        .collect()
}

fn string_map(v: Option<&serde_json::Value>) -> Result<BTreeMap<String, String>> {
    match v {
        None => Ok(BTreeMap::new()),
        Some(serde_json::Value::Object(o)) => Ok(o
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect()),
        Some(_) => Err(CoreError::Invalid("theme.json map is not an object".into())),
    }
}

fn add_file(
    zw: &mut zip::ZipWriter<std::fs::File>,
    opts: zip::write::SimpleFileOptions,
    disk: &Path,
    entry: &str,
) -> Result<()> {
    if disk.is_file() {
        zw.start_file(entry, opts).map_err(zip_err)?;
        zw.write_all(&std::fs::read(disk)?)?;
    }
    Ok(())
}

fn clear_dir(dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_file() {
            std::fs::remove_file(p)?;
        }
    }
    Ok(())
}

impl ThemeStore {
    /// Writes `theme.json` plus every referenced icon, sound and font to `dest`.
    pub fn export_zip(&self, dest: &Path) -> Result<()> {
        let mut zw = zip::ZipWriter::new(std::fs::File::create(dest)?);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let json = serde_json::to_string_pretty(&self.data).map_err(|source| CoreError::Json {
            path: dest.to_path_buf(),
            source,
        })?;
        zw.start_file("theme.json", opts).map_err(zip_err)?;
        zw.write_all(json.as_bytes())?;
        for name in self.data.icons.values() {
            let Some(name) = Path::new(name).file_name().map(|n| n.to_string_lossy()) else {
                continue;
            };
            add_file(
                &mut zw,
                opts,
                &self.paths.theme_icons_dir().join(&*name),
                &format!("icons/{name}"),
            )?;
        }
        for name in self.data.sounds.values() {
            let Some(name) = Path::new(name).file_name().map(|n| n.to_string_lossy()) else {
                continue;
            };
            add_file(
                &mut zw,
                opts,
                &self.paths.theme_sounds_dir().join(&*name),
                &format!("sounds/{name}"),
            )?;
        }
        let fonts: std::collections::BTreeSet<&String> = self.data.font_files.values().collect();
        for rel in fonts {
            if let Some(file) = Path::new(rel).file_name() {
                let file = file.to_string_lossy();
                add_file(
                    &mut zw,
                    opts,
                    &self.paths.theme_fonts_dir().join(&*file),
                    &format!("fonts/{file}"),
                )?;
            }
        }
        zw.finish().map_err(zip_err)?;
        Ok(())
    }

    /// Replaces the whole theme with the archive at `src`.
    ///
    /// Each asset inside the archive may be at most [`MAX_ASSET_BYTES`].
    ///
    /// The archive is fully read and validated before anything on disk or in
    /// memory changes; on error the current theme is untouched. Entries other
    /// than `theme.json` and plain files in `icons/`, `sounds/`, `fonts/` are
    /// ignored.
    pub fn import_zip(&mut self, src: &Path) -> Result<()> {
        let mut zf = zip::ZipArchive::new(std::fs::File::open(src)?).map_err(zip_err)?;
        let mut config = Vec::new();
        zf.by_name("theme.json")
            .map_err(|_| CoreError::Invalid("theme archive is missing theme.json".into()))?
            .read_to_end(&mut config)?;
        let value: serde_json::Value =
            serde_json::from_slice(&config).map_err(|source| CoreError::Json {
                path: src.to_path_buf(),
                source,
            })?;
        let obj = value
            .as_object()
            .ok_or_else(|| CoreError::Invalid("theme.json is not an object".into()))?;
        for key in [
            "colors",
            "icons",
            "sounds",
            "icon_tints",
            "capture_card",
            "fonts",
            "font_files",
        ] {
            if obj.get(key).is_some_and(|v| !v.is_object()) {
                return Err(CoreError::Invalid(format!(
                    "theme.json \"{key}\" is not an object"
                )));
            }
        }
        let mut assets: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        let mut present: [std::collections::BTreeSet<String>; 3] = Default::default();
        for i in 0..zf.len() {
            let entry = zf.by_index(i).map_err(zip_err)?;
            if entry.is_dir() {
                continue;
            }
            let Some((folder, file)) = entry.name().ok().and_then(|n| classify(&n)) else {
                continue;
            };
            let dir = match folder {
                Folder::Icons => self.paths.theme_icons_dir(),
                Folder::Sounds => self.paths.theme_sounds_dir(),
                Folder::Fonts => self.paths.theme_fonts_dir(),
            };
            if entry.size() > MAX_ASSET_BYTES {
                return Err(CoreError::Invalid(format!(
                    "theme asset {file} is too large ({} bytes; limit {MAX_ASSET_BYTES})",
                    entry.size()
                )));
            }
            present[folder as usize].insert(file.clone());
            let mut body = Vec::new();
            entry.take(MAX_ASSET_BYTES + 1).read_to_end(&mut body)?;
            if body.len() as u64 > MAX_ASSET_BYTES {
                return Err(CoreError::Invalid(format!(
                    "theme asset {file} is too large (limit {MAX_ASSET_BYTES} bytes)"
                )));
            }
            assets.push((dir.join(file), body));
        }

        let mut colors = default_colors();
        colors.extend(string_map(obj.get("colors"))?);
        let mut capture_card = default_capture_card_colors();
        capture_card.extend(string_map(obj.get("capture_card"))?);
        migrate::sanitize_colors(&mut colors, &mut capture_card);
        let mut fonts = default_fonts();
        fonts.extend(string_map(obj.get("fonts"))?);
        let mut icon_tints = string_map(obj.get("icon_tints"))?;
        if !obj.contains_key("icon_tints") {
            icon_tints.insert("_global".into(), super::DEFAULT_ICON_TINT.into());
        }
        let font_files = retain_present(string_map(obj.get("font_files"))?, &present[2]);
        let defaults = default_fonts();
        for (role, default) in &defaults {
            let family = fonts.get(role).cloned().unwrap_or_else(|| default.clone());
            if !migrate::BUILTIN_FONT_FAMILIES.contains(&family.as_str())
                && !font_files.contains_key(&family)
            {
                fonts.insert(role.clone(), default.clone());
            }
        }
        let data = ThemeData {
            palette_version: 4,
            typography_version: 2,
            colors,
            icons: retain_present(string_map(obj.get("icons"))?, &present[0]),
            sounds: retain_present(string_map(obj.get("sounds"))?, &present[1]),
            icon_tints,
            capture_card,
            fonts,
            font_files,
        };

        for dir in [
            self.paths.theme_icons_dir(),
            self.paths.theme_sounds_dir(),
            self.paths.theme_fonts_dir(),
        ] {
            clear_dir(&dir)?;
        }
        for (dest, body) in assets {
            std::fs::write(dest, body)?;
        }
        self.data = data;
        self.save()
    }
}
