//! Theme zip export and import.
//!
//! Importing needs nothing beyond Stored and Deflate entries, which is all the
//! `zip` crate is built with here.

/// Largest single asset (icon, sound or font) accepted from a theme archive.
pub const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
/// Largest total size of the assets of one theme archive. A real theme is a
/// few megabytes; the limit keeps an archive built to inflate (a zip bomb)
/// from filling the disk.
pub const MAX_TOTAL_ASSET_BYTES: u64 = 256 * 1024 * 1024;
/// Most entries a theme archive may hold, directories and ignored files
/// included.
pub const MAX_ENTRIES: usize = 1000;
/// Largest `theme.json` accepted (a real one is a few kilobytes).
pub const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

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

/// Reads the whole archive entry `name`, failing when it is longer than
/// `limit` bytes.
fn read_entry(zf: &mut zip::ZipArchive<std::fs::File>, name: &str, limit: u64) -> Result<Vec<u8>> {
    let entry = zf
        .by_name(name)
        .map_err(|_| CoreError::Invalid(format!("theme archive is missing {name}")))?;
    let mut body = Vec::new();
    entry.take(limit + 1).read_to_end(&mut body)?;
    if body.len() as u64 > limit {
        return Err(CoreError::Invalid(format!(
            "{name} in the theme archive is too large (limit {limit} bytes)"
        )));
    }
    Ok(body)
}

const FOLDERS: [(Folder, &str); 3] = [
    (Folder::Icons, "icons"),
    (Folder::Sounds, "sounds"),
    (Folder::Fonts, "fonts"),
];

/// A directory next to the live theme folders that an import fills before
/// anything live changes. Its `new/<folder>` directories hold the extracted
/// assets and its `old/` the folders swapped out. It is removed on drop.
struct Staging {
    root: PathBuf,
}

impl Staging {
    const PREFIX: &'static str = ".import-";

    /// Creates the staging directories and removes the ones a crashed import
    /// left behind.
    fn create(theme_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(theme_dir)?;
        if let Ok(entries) = std::fs::read_dir(theme_dir) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(Self::PREFIX)
                {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        let root = theme_dir.join(format!("{}{}", Self::PREFIX, uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&root)?;
        let staging = Self { root };
        std::fs::create_dir(staging.root.join("old"))?;
        for (_, name) in FOLDERS {
            std::fs::create_dir_all(staging.new_dir(name))?;
        }
        Ok(staging)
    }

    fn new_dir(&self, name: &str) -> PathBuf {
        self.root.join("new").join(name)
    }

    fn parked(&self, name: &str) -> PathBuf {
        self.root.join("old").join(name)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The three live folders after an import swapped them for the staged ones;
/// puts the old ones back unless committed.
struct Swapped<'a> {
    staging: &'a Staging,
    /// `(folder name, live path, whether an old folder was parked)`.
    done: Vec<(&'static str, PathBuf, bool)>,
}

impl Swapped<'_> {
    fn undo(&mut self) {
        while let Some((name, live, had_old)) = self.done.pop() {
            let _ = std::fs::remove_dir_all(&live);
            if had_old {
                let _ = std::fs::rename(self.staging.parked(name), &live);
            }
        }
    }
}

/// Moves the staged folders into place, parking the live ones. On failure
/// every folder is back as it was.
fn swap_in<'a>(staging: &'a Staging, live: [(&'static str, PathBuf); 3]) -> Result<Swapped<'a>> {
    let mut swapped = Swapped {
        staging,
        done: Vec::new(),
    };
    for (name, path) in live {
        let had_old = std::fs::symlink_metadata(&path).is_ok();
        if had_old && let Err(e) = std::fs::rename(&path, staging.parked(name)) {
            swapped.undo();
            return Err(e.into());
        }
        if let Err(e) = std::fs::rename(staging.new_dir(name), &path) {
            if had_old {
                let _ = std::fs::rename(staging.parked(name), &path);
            }
            swapped.undo();
            return Err(e.into());
        }
        swapped.done.push((name, path, had_old));
    }
    Ok(swapped)
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
    /// Each asset inside the archive may be at most [`MAX_ASSET_BYTES`], the
    /// assets together at most [`MAX_TOTAL_ASSET_BYTES`], `theme.json` at most
    /// [`MAX_CONFIG_BYTES`], and the archive may hold at most [`MAX_ENTRIES`]
    /// entries; anything over is refused, and a compressed entry is never
    /// inflated past its limit.
    ///
    /// The assets are extracted to a directory next to the live folders and
    /// swapped in only when the whole archive has been read and validated;
    /// the current theme and its files are untouched on any error, including
    /// a failure to write `theme.json` at the end. Entries other than
    /// `theme.json` and plain files in `icons/`, `sounds/`, `fonts/` are
    /// ignored (even ones in a compression the build cannot read).
    pub fn import_zip(&mut self, src: &Path) -> Result<()> {
        let mut zf = zip::ZipArchive::new(std::fs::File::open(src)?).map_err(zip_err)?;
        if zf.len() > MAX_ENTRIES {
            return Err(CoreError::Invalid(format!(
                "theme archive has too many entries ({}; limit {MAX_ENTRIES})",
                zf.len()
            )));
        }
        let config = read_entry(&mut zf, "theme.json", MAX_CONFIG_BYTES)?;
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
        let staging = Staging::create(&self.paths.theme_dir())?;
        let mut present: [std::collections::BTreeSet<String>; 3] = Default::default();
        let mut total = 0u64;
        for i in 0..zf.len() {
            // Looked at without inflating it: an entry that is ignored is
            // never decompressed, whatever its compression method.
            let (folder, file, declared) = {
                let entry = zf.by_index_raw(i).map_err(zip_err)?;
                if entry.is_dir() {
                    continue;
                }
                let Some((folder, file)) = entry.name().ok().and_then(|n| classify(&n)) else {
                    continue;
                };
                (folder, file, entry.size())
            };
            if declared > MAX_ASSET_BYTES {
                return Err(CoreError::Invalid(format!(
                    "theme asset {file} is too large ({declared} bytes; limit {MAX_ASSET_BYTES})"
                )));
            }
            let entry = zf.by_index(i).map_err(zip_err)?;
            let dest = staging.new_dir(FOLDERS[folder as usize].1).join(&file);
            // The declared size can lie: only what is actually inflated counts.
            let mut out = std::fs::File::create(&dest)?;
            let written = std::io::copy(&mut entry.take(MAX_ASSET_BYTES + 1), &mut out)?;
            if written > MAX_ASSET_BYTES {
                return Err(CoreError::Invalid(format!(
                    "theme asset {file} is too large (limit {MAX_ASSET_BYTES} bytes)"
                )));
            }
            total += written;
            if total > MAX_TOTAL_ASSET_BYTES {
                return Err(CoreError::Invalid(format!(
                    "theme assets are too large together (limit {MAX_TOTAL_ASSET_BYTES} bytes)"
                )));
            }
            present[folder as usize].insert(file);
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

        let mut swapped = swap_in(
            &staging,
            [
                ("icons", self.paths.theme_icons_dir()),
                ("sounds", self.paths.theme_sounds_dir()),
                ("fonts", self.paths.theme_fonts_dir()),
            ],
        )?;
        let previous = std::mem::replace(&mut self.data, data);
        if let Err(e) = self.save() {
            self.data = previous;
            swapped.undo();
            return Err(e);
        }
        Ok(())
    }
}
