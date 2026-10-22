//! SQLite-backed per-clip metadata: tags, descriptions and editor drafts.

use crate::error::{CoreError, Result};
use crate::media_identity::FileIdentity;
use crate::paths::FthrPaths;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// Maximum number of clips that keep an editor draft.
const EDITOR_DRAFT_LIMIT: usize = 250;

/// Stat identity a draft is bound to; a mismatch makes the draft stale.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DraftFile {
    /// Modification time in nanoseconds since the epoch.
    pub mtime_ns: i128,
    /// Size in bytes.
    pub size: u64,
}

/// Non-destructive clip-editor snapshot persisted for a clip.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct EditorDraft {
    /// Draft schema version (currently 1).
    pub version: u32,
    /// Identity of the source file the draft was made for.
    pub file: DraftFile,
    /// Unix timestamp (seconds) of the last save.
    pub updated_at: f64,
    /// Opaque editor state; the editor crate owns its schema.
    pub state: Value,
}

/// Stored metadata of one clip.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct ClipEntry {
    /// User tag.
    pub tag: String,
    /// User description.
    pub description: String,
    /// Editor draft, when one is saved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor_draft: Option<EditorDraft>,
}

/// Clip metadata persisted in `clip_metadata.sqlite3`.
pub struct ClipMetadataStore {
    conn: Connection,
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn table_exists(conn: &Connection) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='entries'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Lenient parse: non-object values and malformed fields fall back to defaults.
fn parse_entry(raw: &str) -> ClipEntry {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
        return ClipEntry::default();
    };
    let text = |k: &str| {
        map.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    ClipEntry {
        tag: text("tag"),
        description: text("description"),
        editor_draft: map
            .get("editor_draft")
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
    }
}

fn to_json(entry: &ClipEntry) -> Result<String> {
    serde_json::to_string(entry).map_err(|e| CoreError::Invalid(format!("entry serialise: {e}")))
}

fn load(tx: &Transaction<'_>, k: &str) -> Result<Option<ClipEntry>> {
    Ok(tx
        .query_row("SELECT value FROM entries WHERE path=?1", [k], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .map(|raw| parse_entry(&raw)))
}

fn store(tx: &Transaction<'_>, k: &str, entry: &ClipEntry) -> Result<()> {
    tx.execute(
        "INSERT INTO entries VALUES (?1, ?2) \
         ON CONFLICT(path) DO UPDATE SET value=excluded.value",
        (k, to_json(entry)?),
    )?;
    Ok(())
}

impl ClipMetadataStore {
    /// Opens (creating if needed) the database, importing the legacy JSON once.
    ///
    /// A held lock surfaces as [`CoreError::Sqlite`]; there is no JSON fallback.
    pub fn open(paths: &FthrPaths) -> Result<Self> {
        let db = paths.clip_metadata_db();
        if let Some(parent) = db.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(&db)?;
        conn.busy_timeout(std::time::Duration::ZERO)?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        if !table_exists(&conn)? {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if !table_exists(&tx)? {
                tx.execute(
                    "CREATE TABLE entries (path TEXT PRIMARY KEY, value TEXT NOT NULL)",
                    [],
                )?;
                Self::import_legacy(&tx, &paths.legacy_clip_metadata_json())?;
            }
            tx.commit()?;
        }
        Ok(Self { conn })
    }

    fn import_legacy(tx: &Transaction<'_>, legacy: &Path) -> Result<()> {
        let Ok(text) = std::fs::read_to_string(legacy) else {
            return Ok(());
        };
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
            return Ok(());
        };
        for (path, value) in map {
            tx.execute(
                "INSERT OR REPLACE INTO entries VALUES (?1, ?2)",
                (path, value.to_string()),
            )?;
        }
        Ok(())
    }

    /// Returns the entry for `path`, or the default when unknown.
    pub fn get(&self, path: &Path) -> Result<ClipEntry> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM entries WHERE path=?1",
                [key(path)],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.map(|r| parse_entry(&r)).unwrap_or_default())
    }

    /// Sets the given fields, leaving `None` fields and any draft untouched.
    pub fn update(
        &mut self,
        path: &Path,
        tag: Option<&str>,
        description: Option<&str>,
    ) -> Result<()> {
        let k = key(path);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut entry = load(&tx, &k)?.unwrap_or_default();
        if let Some(t) = tag {
            entry.tag = t.to_owned();
        }
        if let Some(d) = description {
            entry.description = d.to_owned();
        }
        store(&tx, &k, &entry)?;
        tx.commit()?;
        Ok(())
    }

    /// Saves `draft` for `path`, then prunes to the 250 newest drafts.
    pub fn set_editor_draft(&mut self, path: &Path, draft: EditorDraft) -> Result<()> {
        let k = key(path);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut entry = load(&tx, &k)?.unwrap_or_default();
        entry.editor_draft = Some(draft);
        store(&tx, &k, &entry)?;
        Self::prune_drafts(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// Clears the oldest drafts (by `updated_at`) beyond the limit; entries stay.
    fn prune_drafts(tx: &Transaction<'_>) -> Result<()> {
        let mut drafts: Vec<(f64, String, ClipEntry)> = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT path, value FROM entries")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (p, raw) = row?;
                let entry = parse_entry(&raw);
                if let Some(d) = &entry.editor_draft {
                    drafts.push((d.updated_at, p, entry));
                }
            }
        }
        if drafts.len() <= EDITOR_DRAFT_LIMIT {
            return Ok(());
        }
        drafts.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, p, mut entry) in drafts.into_iter().skip(EDITOR_DRAFT_LIMIT) {
            entry.editor_draft = None;
            store(tx, &p, &entry)?;
        }
        Ok(())
    }

    /// Removes the draft of `path`, keeping its tag and description.
    pub fn clear_editor_draft(&mut self, path: &Path) -> Result<()> {
        let k = key(path);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(mut entry) = load(&tx, &k)?
            && entry.editor_draft.take().is_some()
        {
            store(&tx, &k, &entry)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Returns the draft only when its recorded file identity matches `identity`.
    pub fn editor_draft_if_current(
        &self,
        path: &Path,
        identity: &FileIdentity,
    ) -> Result<Option<EditorDraft>> {
        Ok(self
            .get(path)?
            .editor_draft
            .filter(|d| d.file.mtime_ns == identity.mtime_ns && d.file.size == identity.size))
    }

    /// Moves the entry of `from` to `to`; no-op when `from` has none.
    pub fn rename_path(&mut self, from: &Path, to: &Path) -> Result<()> {
        let (f, t) = (key(from), key(to));
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(raw) = tx
            .query_row("SELECT value FROM entries WHERE path=?1", [&f], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
        {
            tx.execute("DELETE FROM entries WHERE path=?1", [&f])?;
            tx.execute(
                "INSERT INTO entries VALUES (?1, ?2) \
                 ON CONFLICT(path) DO UPDATE SET value=excluded.value",
                (&t, raw),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Source duration reduced by trim and deleted segments of the current draft.
    ///
    /// Returns `source_duration` when there is no current, valid draft.
    pub fn edited_duration(&self, path: &Path, source_duration: f64) -> Result<f64> {
        let Ok(identity) = crate::media_identity::file_identity(path) else {
            return Ok(source_duration);
        };
        let Some(draft) = self.editor_draft_if_current(path, &identity)? else {
            return Ok(source_duration);
        };
        if draft.version != 1 {
            return Ok(source_duration);
        }
        Ok(retained_fraction(&draft.state).map_or(source_duration, |k| source_duration * k))
    }
}

/// Retained fraction of the source for a draft state, or `None` when invalid.
fn retained_fraction(state: &Value) -> Option<f64> {
    let state = state.as_object()?;
    let num = |v: Option<&Value>, default: f64| match v {
        None => Some(default),
        Some(v) => v.as_f64(),
    };
    let start = num(state.get("trim_start"), 0.0)?;
    let end = num(state.get("trim_end"), 1.0)?;
    let segments = state.get("segments")?.as_array()?;
    if !start.is_finite()
        || !end.is_finite()
        || !(0.0 <= start && start < end && end <= 1.0)
        || !(1..=500).contains(&segments.len())
    {
        return None;
    }
    let (mut kept, mut previous) = (0.0_f64, 0.0_f64);
    for seg in segments {
        let seg = seg.as_array()?;
        if seg.len() != 3 {
            return None;
        }
        let (left, right) = (seg[0].as_f64()?, seg[1].as_f64()?);
        let deleted = match &seg[2] {
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64()? != 0.0,
            Value::Null => false,
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        };
        if !left.is_finite()
            || !right.is_finite()
            || !(0.0 <= left && left < right && right <= 1.0)
            || (left - previous).abs() > 0.0001
        {
            return None;
        }
        previous = right;
        if !deleted {
            kept += (end.min(right) - start.max(left)).max(0.0);
        }
    }
    if (previous - 1.0).abs() > 0.0001 || kept <= 0.0 {
        return None;
    }
    Some(kept)
}
