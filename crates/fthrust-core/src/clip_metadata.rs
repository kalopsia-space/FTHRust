//! SQLite-backed per-clip metadata: tags, descriptions and editor drafts.

use crate::error::{CoreError, Result};
use crate::media_identity::FileIdentity;
use crate::paths::FthrPaths;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
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

/// A raw stored row; unknown keys and unparseable fields are preserved.
type Row = Map<String, Value>;

/// Parses a stored value into a raw row; non-objects count as an empty row.
fn parse_row(raw: &str) -> Row {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => map,
        _ => Row::new(),
    }
}

/// Typed, lenient view of a raw row.
fn entry_from_row(row: &Row) -> ClipEntry {
    let text = |k: &str| {
        row.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    ClipEntry {
        tag: text("tag"),
        description: text("description"),
        editor_draft: row
            .get("editor_draft")
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
    }
}

/// Compact JSON with `tag`, `description`, `editor_draft` first, then other keys.
fn row_to_json(row: &Row) -> Result<String> {
    let err = |e: serde_json::Error| CoreError::Invalid(format!("entry serialise: {e}"));
    let mut out = String::from("{");
    let mut first = true;
    let known = ["tag", "description", "editor_draft"];
    let ordered = known
        .iter()
        .filter_map(|k| row.get_key_value(*k))
        .chain(row.iter().filter(|(k, _)| !known.contains(&k.as_str())));
    for (k, v) in ordered {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&serde_json::to_string(k).map_err(err)?);
        out.push(':');
        out.push_str(&serde_json::to_string(v).map_err(err)?);
    }
    out.push('}');
    Ok(out)
}

fn load(tx: &Transaction<'_>, k: &str) -> Result<Option<Row>> {
    Ok(tx
        .query_row("SELECT value FROM entries WHERE path=?1", [k], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .map(|raw| parse_row(&raw)))
}

fn store(tx: &Transaction<'_>, k: &str, row: &Row) -> Result<()> {
    tx.execute(
        "INSERT INTO entries VALUES (?1, ?2) \
         ON CONFLICT(path) DO UPDATE SET value=excluded.value",
        (k, row_to_json(row)?),
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
        Ok(raw
            .map(|r| entry_from_row(&parse_row(&r)))
            .unwrap_or_default())
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
        let mut row = load(&tx, &k)?.unwrap_or_default();
        if let Some(t) = tag {
            row.insert("tag".into(), Value::String(t.to_owned()));
        }
        if let Some(d) = description {
            row.insert("description".into(), Value::String(d.to_owned()));
        }
        store(&tx, &k, &row)?;
        tx.commit()?;
        Ok(())
    }

    /// Saves `draft` for `path`, then prunes to the 250 newest drafts.
    pub fn set_editor_draft(&mut self, path: &Path, draft: EditorDraft) -> Result<()> {
        let k = key(path);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut row = load(&tx, &k)?.unwrap_or_default();
        let value = serde_json::to_value(&draft)
            .map_err(|e| CoreError::Invalid(format!("draft serialise: {e}")))?;
        row.insert("editor_draft".into(), value);
        store(&tx, &k, &row)?;
        Self::prune_drafts(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// Clears the oldest drafts (by `updated_at`, ties by path) beyond the limit.
    fn prune_drafts(tx: &Transaction<'_>) -> Result<()> {
        let mut drafts: Vec<(f64, String, Row)> = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT path, value FROM entries")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (p, raw) = row?;
                let row = parse_row(&raw);
                if let Some(Value::Object(d)) = row.get("editor_draft") {
                    let updated = d.get("updated_at").and_then(Value::as_f64).unwrap_or(0.0);
                    drafts.push((updated, p, row));
                }
            }
        }
        if drafts.len() <= EDITOR_DRAFT_LIMIT {
            return Ok(());
        }
        drafts.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        for (_, p, mut row) in drafts.into_iter().skip(EDITOR_DRAFT_LIMIT) {
            row.remove("editor_draft");
            store(tx, &p, &row)?;
        }
        Ok(())
    }

    /// Removes the draft of `path`, keeping its tag and description.
    pub fn clear_editor_draft(&mut self, path: &Path) -> Result<()> {
        let k = key(path);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(mut row) = load(&tx, &k)?
            && row.remove("editor_draft").is_some()
        {
            store(&tx, &k, &row)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Returns the draft only when it is version 1, has object state and its
    /// recorded file identity matches `identity`.
    pub fn editor_draft_if_current(
        &self,
        path: &Path,
        identity: &FileIdentity,
    ) -> Result<Option<EditorDraft>> {
        Ok(self.get(path)?.editor_draft.filter(|d| {
            d.version == 1
                && d.state.is_object()
                && d.file.mtime_ns == identity.mtime_ns
                && d.file.size == identity.size
        }))
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
