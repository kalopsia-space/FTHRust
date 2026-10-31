//! The `settings.json` store: defaults, load, save and quarantine.

mod defaults;
mod migrate;

pub use defaults::defaults;

use crate::atomic_json::{Loaded, load_json};
use crate::{CoreError, FthrPaths, Result, fsutil, write_json_atomic};
use serde_json::{Map, Value};
use std::path::{Component, Path, PathBuf};

/// In-memory view of `~/.fthr/settings.json`.
#[derive(Debug)]
pub struct SettingsStore {
    paths: FthrPaths,
    data: Map<String, Value>,
    /// Built-in defaults for this home, used by `get_or_default`.
    defaults: Map<String, Value>,
    dirty: bool,
    /// Why `save` is refused: the file on disk could not be read (or set
    /// aside), so what is in memory is not built from it and must not
    /// replace it.
    save_blocked: Option<String>,
    /// Problems met while loading, for the UI to report.
    issues: Vec<String>,
}

/// Merges `over` into `base`, one level deep.
///
/// When both values for a key are objects they are unioned with `over`
/// winning per child key; otherwise the `over` value replaces the base one.
pub fn merge_one_level(
    mut base: Map<String, Value>,
    over: Map<String, Value>,
) -> Map<String, Value> {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(Value::Object(b)), Value::Object(o)) => {
                for (ck, cv) in o {
                    b.insert(ck, cv);
                }
            }
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
    base
}

impl SettingsStore {
    /// Loads settings from disk.
    ///
    /// A missing file yields the defaults. A file that fails to parse, or
    /// parses to something other than an object, is renamed to
    /// `settings.json.corrupt` (replacing an older one) and defaults are used.
    ///
    /// Loading does not fail on a damaged or unreadable file, nor when the
    /// migrated settings cannot be written back (a full disk must not stop
    /// the app from opening): the problem is logged and kept in
    /// [`startup_issues`](Self::startup_issues). A file that could not be
    /// read, or could not be set aside, is never replaced: [`save`](Self::save)
    /// refuses, so defaults cannot overwrite the user's settings.
    pub fn load(paths: FthrPaths) -> Result<Self> {
        let file = paths.settings_file();
        let base = defaults(&paths);
        let mut issues = Vec::new();
        let mut save_blocked = None;
        let (mut data, loaded, existed) = match load_json(&file) {
            Loaded::Value(Value::Object(loaded)) => {
                (merge_one_level(base.clone(), loaded.clone()), loaded, true)
            }
            Loaded::Missing => (base.clone(), Map::new(), false),
            // A transient read error (permissions, EIO, too many open files)
            // must not touch the user's file: use defaults for this run and
            // refuse to save over it.
            Loaded::Unreadable(e) => {
                tracing::warn!("could not read {}: {e}; using defaults", file.display());
                save_blocked = Some(format!(
                    "it could not be read ({e}); the default settings are used for this session"
                ));
                issues.push(format!(
                    "settings.json could not be read ({e}). The default settings are used and the file is left as it is, so changes are not saved this session."
                ));
                (base.clone(), Map::new(), false)
            }
            // Non-object content, a parse failure or invalid UTF-8 is
            // quarantined, as the reference does.
            Loaded::Value(_) | Loaded::Corrupt => {
                match fsutil::quarantine(&file) {
                    Ok(target) => issues.push(format!(
                        "settings.json was damaged. It was moved to {} and the default settings are used.",
                        target.display()
                    )),
                    Err(e) => {
                        tracing::warn!("could not quarantine {}: {e}", file.display());
                        save_blocked = Some(format!(
                            "it is damaged and could not be set aside ({e}); the default settings are used for this session"
                        ));
                        issues.push(format!(
                            "settings.json is damaged and could not be set aside ({e}). The default settings are used and the file is left as it is, so changes are not saved this session."
                        ));
                    }
                }
                (base.clone(), Map::new(), false)
            }
        };
        let changed = migrate::migrate(&mut data, &loaded, &paths, existed);
        let mut store = Self {
            paths,
            data,
            defaults: base,
            dirty: false,
            save_blocked,
            issues,
        };
        if changed {
            store.dirty = true;
            // A full or read-only disk must not stop the app from opening
            // (the user may need it to delete clips): the migrated settings
            // are used from memory and the failure is reported.
            if let Err(e) = store.save() {
                tracing::error!("could not save the migrated settings: {e}");
                store.issues.push(format!(
                    "The migrated settings could not be saved ({e}). They are used for this session."
                ));
            }
        }
        Ok(store)
    }

    /// Problems met while loading (an unreadable or damaged file, a failed
    /// write-back of the migrated settings), worded for the user. The app
    /// keeps running with what it could load; the UI reports these.
    pub fn startup_issues(&self) -> &[String] {
        &self.issues
    }

    /// Returns the whole settings map, including in-memory `_` keys.
    pub fn data(&self) -> &Map<String, Value> {
        &self.data
    }

    /// Returns the value for `key`, if present.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.data.get(key)
    }

    /// Returns `key` as a bool, if present and a bool.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key)?.as_bool()
    }

    /// Returns `key` as an i64, if present and an integer.
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key)?.as_i64()
    }

    /// Returns `key` as an f64, if present and a number.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key)?.as_f64()
    }

    /// Returns `key` as a string slice, if present and a string.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    /// Returns the stored value, falling back to the built-in default.
    ///
    /// # Panics
    ///
    /// Panics if `key` is not present in [`defaults`]; use it only for keys
    /// of the reference settings table.
    pub fn get_or_default(&self, key: &str) -> &Value {
        if let Some(v) = self.data.get(key) {
            return v;
        }
        self.defaults
            .get(key)
            .unwrap_or_else(|| panic!("unknown settings key: {key}"))
    }

    /// Sets `key` and marks the store dirty.
    pub fn set(&mut self, key: &str, value: Value) {
        self.data.insert(key.to_string(), value);
        self.dirty = true;
    }

    /// Removes `key` and marks the store dirty.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.dirty = true;
        self.data.shift_remove(key)
    }

    /// Writes the settings as indent-2 JSON, omitting keys that start with `_`.
    ///
    /// Fails with [`CoreError::Protected`] when the file could not be read at
    /// load: the settings in memory are then defaults, not the user's.
    pub fn save(&mut self) -> Result<()> {
        if let Some(reason) = &self.save_blocked {
            return Err(CoreError::Protected {
                path: self.paths.settings_file(),
                reason: reason.clone(),
            });
        }
        let out: Map<String, Value> = self
            .data
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        write_json_atomic(&self.paths.settings_file(), &Value::Object(out), true)?;
        self.dirty = false;
        Ok(())
    }

    /// Returns the expanded, normalised clips directory.
    ///
    /// Expands `~`, `$VAR` and `${VAR}`, then normalises `.` and `..`
    /// lexically. Errors with [`CoreError::Invalid`] if empty or relative.
    pub fn clips_directory(&self) -> Result<PathBuf> {
        self.resolve_dir("clips_directory")
    }

    /// Returns the recording directory, or `<clips>/Recordings` when the key
    /// is absent or empty.
    pub fn recording_directory(&self) -> Result<PathBuf> {
        match self.get_str("recording_directory") {
            Some(s) if !s.is_empty() => self.resolve_dir("recording_directory"),
            _ => Ok(self.clips_directory()?.join("Recordings")),
        }
    }

    fn resolve_dir(&self, key: &str) -> Result<PathBuf> {
        let raw = self.get_str(key).unwrap_or("");
        if raw.is_empty() {
            return Err(CoreError::Invalid(format!("{key} is empty")));
        }
        let home = self.paths.root().parent().map(Path::to_path_buf);
        let expanded = expand(raw, home.as_deref());
        let clean = normalize(Path::new(&expanded));
        if !clean.is_absolute() {
            return Err(CoreError::Invalid(format!("{key} is not absolute: {raw}")));
        }
        Ok(clean)
    }
}

/// Expands and lexically normalises a stored directory string.
///
/// Returns `None` when `raw` is empty or does not expand to an absolute path.
pub(crate) fn resolve_storage_path(raw: &str, paths: &FthrPaths) -> Option<PathBuf> {
    if raw.is_empty() {
        return None;
    }
    let home = paths.root().parent().map(Path::to_path_buf);
    let clean = normalize(Path::new(&expand(raw, home.as_deref())));
    clean.is_absolute().then_some(clean)
}

/// Expands a leading `~` and `$VAR` / `${VAR}`; unknown variables stay as written.
fn expand(s: &str, home: Option<&Path>) -> String {
    let mut rest = s;
    let mut out = String::new();
    if let Some(home) = home
        && (s == "~" || s.starts_with("~/"))
    {
        out.push_str(&home.to_string_lossy());
        rest = &s[1..];
    }
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' {
            let (name, end) = if b.get(i + 1) == Some(&b'{') {
                match rest[i + 2..].find('}') {
                    Some(n) => (&rest[i + 2..i + 2 + n], i + 3 + n),
                    None => ("", i),
                }
            } else {
                let n = rest[i + 1..]
                    .bytes()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
                    .count();
                (&rest[i + 1..i + 1 + n], i + 1 + n)
            };
            if !name.is_empty() {
                if let Ok(v) = std::env::var(name) {
                    out.push_str(&v);
                    i = end;
                    continue;
                }
                out.push_str(&rest[i..end]);
                i = end;
                continue;
            }
        }
        let ch = rest[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Lexically removes `.` and resolves `..` without touching the filesystem.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !p.is_absolute() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Names of the subdirectories created under the clips directory.
pub fn clips_subdirectories() -> [&'static str; 5] {
    ["Desktop", "Recordings", "Exported", "Shared", "Screenshots"]
}

/// Creates `dir` and its five standard subdirectories.
pub fn prepare_clips_storage(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for sub in clips_subdirectories() {
        std::fs::create_dir_all(dir.join(sub))?;
    }
    Ok(())
}
