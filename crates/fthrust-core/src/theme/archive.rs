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
use crate::paths::FthrPaths;
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

/// The name `theme.json` is kept under in a staging directory's `old/`.
const KEPT_THEME_FILE: &str = "theme.json";

/// A directory next to the live theme folders that an import fills before
/// anything live changes. Its `new/<folder>` directories hold the extracted
/// assets and its `old/` the folders swapped out, with a copy of the
/// `theme.json` they belong to.
///
/// It is removed on drop, except when the import did not complete and a
/// swapped-out folder is still in `old/` (an undo that could not rename it
/// back): those are the user's files, and the recovery at the next start puts
/// them back.
struct Staging {
    root: PathBuf,
    /// The import completed: `theme.json` has been replaced.
    committed: bool,
}

impl Staging {
    const PREFIX: &'static str = ".import-";

    /// Creates the staging directories. What a crashed import left behind is
    /// dealt with first, as at startup: the folders it parked are put back,
    /// not deleted.
    fn create(paths: &FthrPaths) -> Result<Self> {
        let theme_dir = paths.theme_dir();
        std::fs::create_dir_all(&theme_dir)?;
        let recovery = recover_crashed_import(paths);
        if !recovery.put_back.is_empty() {
            tracing::warn!(
                "put back the theme folders a cut-short import had parked: {}",
                recovery.put_back.join(", ")
            );
        }
        let root = theme_dir.join(format!("{}{}", Self::PREFIX, uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&root)?;
        let staging = Self {
            root,
            committed: false,
        };
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

    /// Keeps a copy of the `theme.json` that is about to be replaced. The
    /// import replaces it last, so a recovery after a crash can tell from
    /// this copy whether the import got that far. No `theme.json` yet leaves
    /// nothing to keep.
    fn keep_theme_file(&self, live: &Path) -> Result<()> {
        match std::fs::copy(live, self.root.join("old").join(KEPT_THEME_FILE)) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Renames the swapped-out folder `name` back to `live`. A failure is
    /// logged and the folder stays parked.
    fn put_back(&self, name: &str, live: &Path) {
        if let Err(e) = std::fs::rename(self.parked(name), live) {
            tracing::error!("could not put back the theme {name} folder: {e}");
        }
    }

    /// Marks the import as complete: everything in the directory, the
    /// swapped-out folders included, may now be removed.
    fn commit(&mut self) {
        self.committed = true;
    }

    /// Whether a swapped-out folder is still in `old/`.
    fn holds_parked_folder(&self) -> bool {
        FOLDERS
            .iter()
            .any(|(_, name)| std::fs::symlink_metadata(self.parked(name)).is_ok())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.committed && self.holds_parked_folder() {
            tracing::error!(
                "the theme import did not complete and not every old folder could be put back; the rest stays in {} and is put back at the next start",
                self.root.join("old").display()
            );
            return;
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Undoes what an import left half done when the app was killed, from the
/// `.import-*` directories it left behind.
///
/// An import parks each live folder in its staging's `old/`, moves the staged
/// one in, and replaces `theme.json` last, after keeping a copy of the old
/// one in `old/`. While `theme.json` is still that copy the import did not
/// finish, and every folder it parked is put back over whatever stands in its
/// place (the staged copy when the swap got that far, nothing when the app
/// stopped between the two renames): together they are the theme that
/// `theme.json` describes. Once `theme.json` is not the copy any more the
/// import did finish, and only the leftover directory is removed. A directory
/// that cannot be dealt with stays where it is, with the old files in it, and
/// is reported in [`Recovery::stuck`].
///
/// Run before the live folders are created.
pub(super) fn recover_crashed_import(paths: &FthrPaths) -> Recovery {
    let mut recovery = Recovery::default();
    let Ok(entries) = std::fs::read_dir(paths.theme_dir()) else {
        return recovery;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(Staging::PREFIX)
        {
            continue;
        }
        let root = entry.path();
        match undo_unfinished_import(paths, &root, &mut recovery.put_back) {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&root);
            }
            Err(e) => {
                tracing::error!(
                    "could not undo the theme import left in {}: {e}",
                    root.display()
                );
                recovery.stuck.push((root, e.to_string()));
            }
        }
    }
    recovery
}

/// What [`recover_crashed_import`] did, for the user.
#[derive(Default)]
pub(super) struct Recovery {
    /// The folders it put back.
    pub(super) put_back: Vec<String>,
    /// The leftover staging directories it could not undo, with the reason.
    /// They stay as they are, and the next start tries again.
    pub(super) stuck: Vec<(PathBuf, String)>,
}

/// Puts back what the import staged in `root` parked, unless it finished.
///
/// A folder that stands in the place of a parked one is moved aside inside
/// `root`, not deleted: it is removed with the staging once the parked folder
/// is back, and goes back to its place if the parked one cannot be renamed.
fn undo_unfinished_import(
    paths: &FthrPaths,
    root: &Path,
    put_back: &mut Vec<String>,
) -> std::io::Result<()> {
    let old = root.join("old");
    if read_if_there(&paths.theme_file())? != read_if_there(&old.join(KEPT_THEME_FILE))? {
        return Ok(());
    }
    let aside_dir = root.join("undone");
    for (_, name) in FOLDERS {
        let parked = old.join(name);
        if std::fs::symlink_metadata(&parked).is_err() {
            continue;
        }
        let live = paths.theme_dir().join(name);
        let aside = aside_dir.join(name);
        let moved_aside = std::fs::symlink_metadata(&live).is_ok();
        if moved_aside {
            std::fs::create_dir_all(&aside_dir)?;
            std::fs::rename(&live, &aside)?;
        }
        if let Err(e) = std::fs::rename(&parked, &live) {
            if moved_aside {
                let _ = std::fs::rename(&aside, &live);
            }
            return Err(e);
        }
        put_back.push(name.to_owned());
    }
    Ok(())
}

/// The bytes of the file, or `None` when there is no such file.
fn read_if_there(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
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
    /// Puts the old folders back. One that cannot be renamed back stays
    /// parked, and the staging is then not removed (see [`Staging`]).
    fn undo(&mut self) {
        while let Some((name, live, had_old)) = self.done.pop() {
            let _ = std::fs::remove_dir_all(&live);
            if had_old {
                self.staging.put_back(name, &live);
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
                staging.put_back(name, &path);
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
    /// a failure to write `theme.json` at the end. If the app is killed
    /// before that last write, the next [`ThemeStore::load`] puts the old
    /// folders back. Entries other than `theme.json` and plain files in
    /// `icons/`, `sounds/`, `fonts/` are ignored (even ones in a compression
    /// the build cannot read).
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
        let mut staging = Staging::create(&self.paths)?;
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

        staging.keep_theme_file(&self.paths.theme_file())?;
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
        // The old folders in the staging are of no use now.
        staging.commit();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD_JSON: &[u8] = br#"{"old":true}"#;
    const NEW_JSON: &[u8] = br#"{"new":true}"#;

    /// A profile whose three theme folders each hold a file named `old`.
    fn home_with_a_theme() -> (tempfile::TempDir, FthrPaths) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = FthrPaths::new(tmp.path());
        for dir in [
            paths.theme_icons_dir(),
            paths.theme_sounds_dir(),
            paths.theme_fonts_dir(),
        ] {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("old"), b"OLD").unwrap();
        }
        std::fs::write(paths.theme_file(), OLD_JSON).unwrap();
        (tmp, paths)
    }

    /// What the import leaves when the process dies right after its swaps and
    /// before it replaces `theme.json`, made with the import's own steps. The
    /// staging is not dropped, as when the process is killed.
    fn killed_after_the_swaps(paths: &FthrPaths) {
        let staging = Staging::create(paths).unwrap();
        std::fs::write(staging.new_dir("icons").join("new.png"), b"NEW").unwrap();
        staging.keep_theme_file(&paths.theme_file()).unwrap();
        let _ = swap_in(
            &staging,
            [
                ("icons", paths.theme_icons_dir()),
                ("sounds", paths.theme_sounds_dir()),
                ("fonts", paths.theme_fonts_dir()),
            ],
        )
        .unwrap();
        std::mem::forget(staging);
    }

    fn staging_dirs(paths: &FthrPaths) -> usize {
        std::fs::read_dir(paths.theme_dir())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(Staging::PREFIX))
            .count()
    }

    #[test]
    fn an_import_killed_after_its_swaps_is_undone() {
        let (_t, paths) = home_with_a_theme();
        killed_after_the_swaps(&paths);
        assert!(paths.theme_icons_dir().join("new.png").exists());
        assert!(!paths.theme_icons_dir().join("old").exists());
        assert_eq!(staging_dirs(&paths), 1);

        let recovery = recover_crashed_import(&paths);

        assert_eq!(recovery.put_back, ["icons", "sounds", "fonts"]);
        assert!(recovery.stuck.is_empty());
        for dir in [
            paths.theme_icons_dir(),
            paths.theme_sounds_dir(),
            paths.theme_fonts_dir(),
        ] {
            assert_eq!(std::fs::read(dir.join("old")).unwrap(), b"OLD");
        }
        assert!(!paths.theme_icons_dir().join("new.png").exists());
        assert_eq!(std::fs::read(paths.theme_file()).unwrap(), OLD_JSON);
        assert_eq!(staging_dirs(&paths), 0);
    }

    #[test]
    fn an_import_killed_after_replacing_theme_json_is_kept() {
        let (_t, paths) = home_with_a_theme();
        killed_after_the_swaps(&paths);
        // The last step of the import, as `save` does it.
        std::fs::write(paths.theme_file(), NEW_JSON).unwrap();

        let recovery = recover_crashed_import(&paths);

        assert!(recovery.put_back.is_empty() && recovery.stuck.is_empty());
        assert!(paths.theme_icons_dir().join("new.png").exists());
        assert!(!paths.theme_icons_dir().join("old").exists());
        assert_eq!(std::fs::read(paths.theme_file()).unwrap(), NEW_JSON);
        assert_eq!(staging_dirs(&paths), 0, "the parked old files are removed");
    }

    #[test]
    fn the_next_import_puts_back_what_a_killed_one_parked() {
        let (_t, paths) = home_with_a_theme();
        killed_after_the_swaps(&paths);

        // The next import starts by dealing with the leftover, then makes
        // its own staging: the old files are not deleted from under it.
        let staging = Staging::create(&paths).unwrap();

        assert_eq!(
            std::fs::read(paths.theme_icons_dir().join("old")).unwrap(),
            b"OLD"
        );
        assert!(!paths.theme_icons_dir().join("new.png").exists());
        assert_eq!(staging_dirs(&paths), 1, "only the new import's staging");
        drop(staging);
        assert_eq!(staging_dirs(&paths), 0);
    }

    #[test]
    fn an_undo_that_cannot_rename_a_folder_back_leaves_it_in_the_staging() {
        let (_t, paths) = home_with_a_theme();
        let staging = Staging::create(&paths).unwrap();
        let root = staging.root.clone();
        staging.keep_theme_file(&paths.theme_file()).unwrap();
        // The import parked the icons and then failed ...
        std::fs::rename(paths.theme_icons_dir(), staging.parked("icons")).unwrap();
        // ... and the undo cannot rename them back: their place is somewhere
        // that does not exist.
        let mut swapped = Swapped {
            staging: &staging,
            done: vec![(
                "icons",
                paths.theme_dir().join("nowhere").join("icons"),
                true,
            )],
        };
        swapped.undo();
        drop(staging);

        assert_eq!(
            std::fs::read(root.join("old/icons/old")).unwrap(),
            b"OLD",
            "the old folder is not deleted with the staging"
        );
        // The next start puts it back.
        let recovery = recover_crashed_import(&paths);
        assert_eq!(recovery.put_back, ["icons"]);
        assert_eq!(
            std::fs::read(paths.theme_icons_dir().join("old")).unwrap(),
            b"OLD"
        );
        assert!(!root.exists());
    }

    #[test]
    fn a_completed_import_removes_its_staging_with_the_old_folders() {
        let (_t, paths) = home_with_a_theme();
        let mut staging = Staging::create(&paths).unwrap();
        std::fs::rename(paths.theme_icons_dir(), staging.parked("icons")).unwrap();
        assert!(staging.holds_parked_folder());
        staging.commit();
        drop(staging);
        assert_eq!(staging_dirs(&paths), 0);
    }
}
