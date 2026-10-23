//! `~/.fthr/uploader/upload_history.json`: successful uploads keyed by the
//! clip's absolute path.

use std::path::Path;

use serde_json::{Map, Value, json};

use super::validate::split_url;
use crate::FthrPaths;

/// One history entry (the uploader writes `status`, `provider`,
/// `uploaded_at`, `url`, `raw_url`, `favorite` and provider extras).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry(pub Map<String, Value>);

impl Entry {
    fn str(&self, key: &str) -> &str {
        self.0.get(key).and_then(Value::as_str).unwrap_or("")
    }
}

/// A link the card can copy or open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The URL.
    pub url: String,
    /// An `sftp://` location rather than a web link.
    pub sftp: bool,
}

/// The whole history file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    entries: Map<String, Value>,
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl History {
    /// Reads the file; anything unreadable is an empty history.
    pub fn load(paths: &FthrPaths) -> Self {
        let entries = std::fs::read_to_string(paths.upload_history_file())
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default();
        Self { entries }
    }

    /// Writes the file atomically (indent 2, insertion order).
    pub fn save(&self, paths: &FthrPaths) -> crate::Result<()> {
        let path = paths.upload_history_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::write_json_atomic(&path, &Value::Object(self.entries.clone()), true)
    }

    /// Load, change and save as one step under an exclusive lock on
    /// `upload_history.json.lock`: the app and the uploader both write the
    /// file.
    pub fn update(paths: &FthrPaths, f: impl FnOnce(&mut History)) -> crate::Result<History> {
        std::fs::create_dir_all(paths.uploader_dir())?;
        let mut lock_name = paths.upload_history_file().into_os_string();
        lock_name.push(".lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(std::path::PathBuf::from(lock_name))?;
        let _lock = nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive)
            .map_err(|(_, e)| std::io::Error::from(e))?;
        let mut h = History::load(paths);
        f(&mut h);
        h.save(paths)?;
        Ok(h)
    }

    /// Copies the legacy `~/.fthr/upload_history.json` when the new file
    /// does not exist yet.
    pub fn migrate_legacy(paths: &FthrPaths) -> crate::Result<()> {
        let legacy = paths.legacy_upload_history_file();
        let new = paths.upload_history_file();
        if legacy.is_file() && !new.exists() {
            std::fs::create_dir_all(paths.uploader_dir())?;
            std::fs::copy(&legacy, &new)?;
        }
        Ok(())
    }

    /// The successful entry for `path` (`get_upload_info`).
    pub fn get(&self, path: &Path) -> Option<Entry> {
        match self.entries.get(&key(path)) {
            Some(Value::Object(m)) if m.get("status").and_then(Value::as_str) == Some("ok") => {
                Some(Entry(m.clone()))
            }
            _ => None,
        }
    }

    /// Whether `path` counts as uploaded: a successful entry that has not
    /// expired (favourites never expire; unparseable expiries are ignored).
    pub fn is_uploaded(&self, path: &Path) -> bool {
        let Some(e) = self.get(path) else {
            return false;
        };
        let expires = e.str("expires_at").trim().to_owned();
        let favourite = super::truthy(e.0.get("favorite"));
        if !expires.is_empty() && !favourite {
            let fmt = time::format_description::well_known::Rfc3339;
            if let Ok(at) = time::OffsetDateTime::parse(&expires, &fmt) {
                return at > time::OffsetDateTime::now_utc();
            }
        }
        true
    }

    /// The copyable link (`ClipCard._upload_url`): `url` then `raw_url`,
    /// http(s) as-is, `sftp://` only with a host, no user info, a path and
    /// a valid port.
    pub fn link(&self, path: &Path) -> Option<Link> {
        let e = self.get(path)?;
        for k in ["url", "raw_url"] {
            let v = e.str(k).trim();
            if v.starts_with("https://") || v.starts_with("http://") {
                return Some(Link {
                    url: v.to_owned(),
                    sftp: false,
                });
            }
            if v.starts_with("sftp://") && valid_sftp(v) {
                return Some(Link {
                    url: v.to_owned(),
                    sftp: true,
                });
            }
        }
        None
    }

    /// Stores `entry` for `path` (the uploader records successes only, but
    /// any entry is stored; `get` filters).
    pub fn record(&mut self, path: &Path, entry: Entry) {
        self.entries.insert(key(path), Value::Object(entry.0));
    }

    /// Points `original` at the entry of its uploaded compressed copy.
    pub fn alias_compressed(&mut self, original: &Path, compressed: &Path, size: u64) {
        let mut m = match self.entries.get(&key(compressed)) {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        m.insert("uploaded_copy".into(), json!("compressed"));
        m.insert("uploaded_path".into(), json!(key(compressed)));
        m.insert("uploaded_size_bytes".into(), json!(size));
        m.insert("original_preserved".into(), json!(true));
        self.entries.insert(key(original), Value::Object(m));
    }

    /// Moves the entry of a renamed clip.
    pub fn rename(&mut self, old: &Path, new: &Path) -> bool {
        match self.entries.remove(&key(old)) {
            Some(v) => {
                self.entries.insert(key(new), v);
                true
            }
            None => false,
        }
    }
}

fn valid_sftp(url: &str) -> bool {
    let Some(u) = split_url(url) else {
        return false;
    };
    !u.host.is_empty()
        && !u.has_userinfo
        && u.port_ok
        && u.path.starts_with('/')
        && u.path.len() > 1
}
