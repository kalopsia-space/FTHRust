//! Field-diagnostics sessions under `~/.fthr/diagnostics`.
//!
//! Ports the session parts of `reference/FTHR_UI/core/field_diagnostics.py`:
//! one `session-<utc>-<id>` directory per run holding `events.jsonl` and
//! `diagnostic-summary.json`, an `active-session.json` marker that reveals
//! unclean shutdowns (removed by [`DiagnosticsSession::close`]), retention of
//! the five newest sessions, and the privacy-bounded zip export, whose
//! `ui.log` and `engine.log` are the tails of `logs/fthr.log` and of the
//! engine's captured stderr, `logs/engine.log`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde_json::{Map, Value, json};
use time::OffsetDateTime;
use time::macros::format_description;

use super::redact::{redact_json, redact_path};
use crate::error::Result;
use crate::paths::FthrPaths;

/// Schema version of every event and summary record.
pub const SCHEMA_VERSION: u32 = 1;
/// Cap on `events.jsonl`.
pub const MAX_EVENT_BYTES: u64 = 4 * 1024 * 1024;
/// Cap on each log included in an export.
pub const MAX_EXPORTED_LOG_BYTES: u64 = 1024 * 1024;
/// Sessions kept on disk.
pub const MAX_RETAINED_SESSIONS: usize = 5;
/// Marker file name under the diagnostics root.
pub const MARKER_NAME: &str = "active-session.json";
/// README shipped inside every export.
pub const README_TEXT: &str = "FTHR Clips Diagnostic Report\n\
============================\n\n\
This ZIP was explicitly exported by the user for alpha support.\n\
It contains bounded diagnostic events and logs only. It does not\n\
contain clips, screenshots, audio, credentials, browser data, or\n\
an arbitrary filesystem listing. Paths and known secrets are\n\
redacted. The report is never uploaded automatically.\n";

const OMITTED_PREFIX: &str = "[earlier diagnostic log content omitted]\n";

fn utc_now_text() -> String {
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]+00:00"
        ))
        .unwrap_or_else(|_| "1970-01-01T00:00:00.000+00:00".into())
}

fn dir_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .unwrap_or_else(|_| "19700101T000000Z".into())
}

#[derive(Debug)]
struct Inner {
    event_bytes: u64,
    dropped_events: u64,
    summary: Map<String, Value>,
}

/// One diagnostics session.
#[derive(Debug)]
pub struct DiagnosticsSession {
    session_id: String,
    app_version: String,
    root: PathBuf,
    dir: PathBuf,
    started_utc: String,
    started_at: Instant,
    max_event_bytes: u64,
    export_log_bytes: u64,
    /// The engine's captured stderr, `logs/engine.log`.
    engine_log: PathBuf,
    previous: Option<Value>,
    closed: AtomicBool,
    inner: Mutex<Inner>,
}

impl DiagnosticsSession {
    /// Starts a session with the reference's limits.
    pub fn start(paths: &FthrPaths, app_version: &str) -> Result<Self> {
        Self::start_with_limits(
            paths,
            app_version,
            MAX_EVENT_BYTES,
            MAX_EXPORTED_LOG_BYTES,
            MAX_RETAINED_SESSIONS,
        )
    }

    /// Starts a session with explicit limits: the `events.jsonl` cap, the
    /// per-log export cap, and how many sessions to retain.
    pub fn start_with_limits(
        paths: &FthrPaths,
        app_version: &str,
        max_event_bytes: u64,
        export_log_bytes: u64,
        retained_sessions: usize,
    ) -> Result<Self> {
        let root = paths.diagnostics_dir();
        fs::create_dir_all(&root)?;
        let session_id = uuid::Uuid::new_v4().to_string();
        let short_id = session_id[..8].to_string();
        let dir = root.join(format!("session-{}-{}", dir_timestamp(), short_id));
        fs::create_dir(&dir)?;
        let started_utc = utc_now_text();
        let previous = read_previous_marker(&root.join(MARKER_NAME), &session_id);
        write_marker(&root.join(MARKER_NAME), &session_id, &started_utc)?;
        enforce_retention(&root, retained_sessions.max(1));

        let mut summary = Map::new();
        summary.insert("schema_version".into(), json!(SCHEMA_VERSION));
        summary.insert("session_id".into(), json!(session_id));
        summary.insert("app_version".into(), json!(app_version));
        summary.insert("started_utc".into(), json!(started_utc));
        summary.insert("status".into(), json!("running"));
        summary.insert("requested_configuration".into(), json!({}));
        summary.insert("actual_configuration".into(), json!({}));
        summary.insert("adapter_topology".into(), json!({}));
        summary.insert("hardware".into(), json!({}));
        summary.insert("displays".into(), json!([]));
        summary.insert("health".into(), json!({}));
        summary.insert("timings".into(), json!({}));
        summary.insert(
            "privacy".into(),
            json!({
                "paths_redacted": true,
                "credentials_redacted": true,
                "media_included": false,
                "automatic_upload": false,
            }),
        );
        if let Some(prev) = &previous {
            summary.insert(
                "previous_session".into(),
                json!({
                    "status": "did_not_shut_down_cleanly",
                    "session_id": prev.get("session_id").cloned().unwrap_or(json!("unavailable")),
                }),
            );
        }

        let session = Self {
            session_id,
            app_version: app_version.to_string(),
            root,
            dir,
            started_utc,
            started_at: Instant::now(),
            max_event_bytes,
            export_log_bytes,
            engine_log: paths.engine_log(),
            previous,
            closed: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                event_bytes: 0,
                dropped_events: 0,
                summary,
            }),
        };
        session.event(
            "application",
            "session_started",
            json!({ "app_version": session.app_version }),
        );
        if let Some(prev) = &session.previous {
            session.event(
                "application",
                "previous_session_unclean",
                json!({
                    "previous_session_id": prev.get("session_id").cloned().unwrap_or(json!("unavailable")),
                    "previous_started_utc": prev.get("started_utc").cloned().unwrap_or(json!("unavailable")),
                }),
            );
        }
        Ok(session)
    }

    /// The session's UUID string.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The session directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The diagnostics root this session lives under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Start time, ISO 8601 UTC with milliseconds.
    pub fn started_utc(&self) -> &str {
        &self.started_utc
    }

    /// The engine's captured stderr, `logs/engine.log` (the file the UI's
    /// engine driver writes); its tail becomes `engine.log` in the report.
    pub fn engine_log_path(&self) -> PathBuf {
        self.engine_log.clone()
    }

    /// The `events.jsonl` path.
    pub fn events_path(&self) -> PathBuf {
        self.dir.join("events.jsonl")
    }

    /// The marker left by a previous session that never closed, if any.
    pub fn previous_unclean(&self) -> Option<&Value> {
        self.previous.as_ref()
    }

    /// Events dropped because `events.jsonl` reached its cap.
    pub fn dropped_events(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .dropped_events
    }

    /// Appends one event record; `fields` must be an object (other values
    /// are ignored). Returns false when the record was dropped by the cap.
    pub fn event(&self, subsystem: &str, event: &str, fields: Value) -> bool {
        self.emit(subsystem, event, None, fields)
    }

    /// Appends one event record with an optional `state`.
    pub fn emit(
        &self,
        subsystem: &str,
        event: &str,
        state: Option<&str>,
        mut fields: Value,
    ) -> bool {
        let mut record = Map::new();
        record.insert("schema_version".into(), json!(SCHEMA_VERSION));
        record.insert("timestamp_utc".into(), json!(utc_now_text()));
        record.insert(
            "monotonic_ms".into(),
            json!(self.started_at.elapsed().as_millis() as u64),
        );
        record.insert("session_id".into(), json!(self.session_id));
        record.insert("subsystem".into(), json!(subsystem));
        record.insert("event".into(), json!(event));
        if let Some(state) = state {
            record.insert("state".into(), json!(state));
        }
        redact_json(&mut fields);
        if let Some(obj) = fields.as_object()
            && !obj.is_empty()
        {
            record.insert("fields".into(), fields);
        }
        let mut line = serde_json::to_string(&Value::Object(record)).unwrap_or_default();
        line.push('\n');
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.event_bytes + line.len() as u64 > self.max_event_bytes {
            inner.dropped_events += 1;
            return false;
        }
        let appended = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.events_path())
            .and_then(|mut f| f.write_all(line.as_bytes()));
        match appended {
            Ok(()) => {
                inner.event_bytes += line.len() as u64;
                true
            }
            Err(_) => {
                inner.dropped_events += 1;
                false
            }
        }
    }

    /// Merges `extra` (an object) into the summary and writes
    /// `diagnostic-summary.json`.
    pub fn write_summary(&self, extra: &Value) -> Result<()> {
        let summary = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(obj) = extra.as_object() {
                for (k, v) in obj {
                    inner.summary.insert(k.clone(), v.clone());
                }
            }
            inner.summary.clone()
        };
        write_pretty_sorted(&self.dir.join("diagnostic-summary.json"), &summary)
    }

    /// The current summary.
    pub fn summary(&self) -> Map<String, Value> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .summary
            .clone()
    }

    /// Ends the session. The app calls this when its event loop has ended,
    /// `clean` being false when it ended on an error.
    ///
    /// Records `session_ended`, writes `diagnostic-summary.json` with the
    /// status `closed_cleanly` (or `abnormal`) and, for a clean end, removes
    /// `active-session.json` if it still names this session. The marker is
    /// what the next launch reads to say that the previous session "did not
    /// shut down cleanly": without this it stays forever and every launch
    /// reports the one before as a crash. Calling it again does nothing.
    pub fn close(&self, clean: bool) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.emit(
            "application",
            "session_ended",
            Some(if clean { "clean" } else { "abnormal" }),
            json!({}),
        );
        let ended = json!({
            "status": if clean { "closed_cleanly" } else { "abnormal" },
            "session_end": {
                "clean": clean,
                "elapsed_ms": self.started_at.elapsed().as_millis() as u64,
            },
        });
        if let Err(e) = self.write_summary(&ended) {
            tracing::warn!("could not write the diagnostics summary: {e}");
        }
        if clean {
            remove_marker_if_ours(&self.root.join(MARKER_NAME), &self.session_id);
        }
    }

    /// Writes the privacy-bounded report zip (`.zip` is appended when
    /// missing) and returns its path. `app_log` is the application log whose
    /// bounded tail becomes `ui.log`.
    pub fn export_zip(&self, destination: &Path, app_log: &Path) -> Result<PathBuf> {
        let mut dest = destination.to_path_buf();
        if dest
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            != Some("zip".into())
        {
            let mut name = dest.as_os_str().to_owned();
            name.push(".zip");
            dest = PathBuf::from(name);
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let events = events_tail(&self.events_path(), self.max_event_bytes);
        let mut summary = self.summary();
        summary.insert(
            "export".into(),
            json!({
                "created_utc": utc_now_text(),
                "files": ["diagnostic-summary.json", "events.jsonl", "ui.log", "engine.log", "README.txt"],
                "events_truncated": events.truncated,
                "events_invalid_lines": events.invalid_lines,
            }),
        );
        let events = events.text;
        let ui_log = bounded_tail(app_log, self.export_log_bytes);
        let engine_log = bounded_tail(&self.engine_log_path(), self.export_log_bytes);
        let mut tmp_name = dest.as_os_str().to_owned();
        tmp_name.push(".tmp");
        let tmp = PathBuf::from(tmp_name);
        let result = (|| -> Result<()> {
            let file = File::create(&tmp)?;
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            let mut sorted = String::new();
            sorted.push_str(&pretty_sorted(&summary));
            sorted.push('\n');
            zip.start_file("diagnostic-summary.json", opts)?;
            zip.write_all(sorted.as_bytes())?;
            zip.start_file("events.jsonl", opts)?;
            zip.write_all(events.as_bytes())?;
            zip.start_file("ui.log", opts)?;
            zip.write_all(ui_log.as_bytes())?;
            zip.start_file("engine.log", opts)?;
            zip.write_all(engine_log.as_bytes())?;
            zip.start_file("README.txt", opts)?;
            zip.write_all(README_TEXT.as_bytes())?;
            zip.finish()?;
            fs::rename(&tmp, &dest)?;
            Ok(())
        })();
        let _ = fs::remove_file(&tmp);
        result?;
        Ok(dest)
    }
}

impl From<zip::result::ZipError> for crate::error::CoreError {
    fn from(e: zip::result::ZipError) -> Self {
        crate::error::CoreError::Invalid(format!("zip: {e}"))
    }
}

fn read_previous_marker(marker: &Path, session_id: &str) -> Option<Value> {
    let text = fs::read_to_string(marker).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let obj = value.as_object()?;
    if obj.get("session_id").and_then(Value::as_str) == Some(session_id) {
        return None;
    }
    Some(value)
}

/// Removes the marker when it names `session_id`: one that names another
/// session belongs to a run that is still going (or crashed) and stays.
fn remove_marker_if_ours(marker: &Path, session_id: &str) {
    let ours = fs::read_to_string(marker)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|v| v.get("session_id").and_then(Value::as_str) == Some(session_id));
    if ours && let Err(e) = fs::remove_file(marker) {
        tracing::warn!("could not remove {}: {e}", marker.display());
    }
}

fn write_marker(marker: &Path, session_id: &str, started_utc: &str) -> Result<()> {
    let mut payload = BTreeMap::new();
    payload.insert("pid", json!(std::process::id()));
    payload.insert("session_id", json!(session_id));
    payload.insert("started_utc", json!(started_utc));
    let text = serde_json::to_string(&payload).unwrap_or_default();
    let tmp = marker.with_extension("tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, marker)?;
    Ok(())
}

fn enforce_retention(root: &Path, keep: usize) {
    let Ok(resolved_root) = root.canonicalize() else {
        return;
    };
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut sessions: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("session-"))
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((modified, e.path()))
        })
        .collect();
    sessions.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, path) in sessions.into_iter().skip(keep) {
        if let Ok(resolved) = path.canonicalize()
            && resolved.parent() == Some(resolved_root.as_path())
            && resolved
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("session-"))
        {
            let _ = fs::remove_dir_all(resolved);
        }
    }
}

fn sorted_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, Value> =
                map.iter().map(|(k, v)| (k, sorted_value(v))).collect();
            let mut out = Map::new();
            for (k, v) in sorted {
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted_value).collect()),
        other => other.clone(),
    }
}

fn pretty_sorted(map: &Map<String, Value>) -> String {
    serde_json::to_string_pretty(&sorted_value(&Value::Object(map.clone()))).unwrap_or_default()
}

fn write_pretty_sorted(path: &Path, map: &Map<String, Value>) -> Result<()> {
    let mut text = pretty_sorted(map);
    text.push('\n');
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// The exported `events.jsonl`.
struct EventsTail {
    /// One JSON object per line.
    text: String,
    /// Older records were left out to stay within the byte limit.
    truncated: bool,
    /// Lines that were not valid JSON (a record cut short by a crash) and
    /// were dropped.
    invalid_lines: usize,
}

/// The newest records of `events.jsonl` that fit in `max_bytes`, as valid
/// JSON lines. Each line is parsed, redacted again as JSON (so a rule added
/// since a session wrote it still applies) and written back; text-level
/// redaction would turn `"auth":"<redacted>"` into `"auth":<redacted>`, which
/// is not JSON. A line cut off by the byte limit or by a crash is dropped,
/// and a missing file is an empty one: the file never holds a line that is
/// not a record.
fn events_tail(path: &Path, max_bytes: u64) -> EventsTail {
    let mut out = EventsTail {
        text: String::new(),
        truncated: false,
        invalid_lines: 0,
    };
    let Ok(mut file) = File::open(path) else {
        return out;
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = size.saturating_sub(max_bytes);
    let mut data = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err()
        || file.take(max_bytes).read_to_end(&mut data).is_err()
    {
        return out;
    }
    if start > 0 {
        // The first line starts before the window: drop its tail end.
        out.truncated = true;
        let skip = data
            .iter()
            .position(|b| *b == b'\n')
            .map_or(data.len(), |p| p + 1);
        data.drain(..skip);
    }
    let mut lines: std::collections::VecDeque<String> = String::from_utf8_lossy(&data)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| match serde_json::from_str::<Value>(line) {
            Ok(mut record) => {
                redact_json(&mut record);
                serde_json::to_string(&record).ok()
            }
            Err(_) => {
                out.invalid_lines += 1;
                None
            }
        })
        .collect();
    // Redaction can lengthen a line ("<redacted>" for a short secret): keep
    // the newest records within the limit.
    let mut total: u64 = lines.iter().map(|l| l.len() as u64 + 1).sum();
    while total > max_bytes {
        let Some(oldest) = lines.pop_front() else {
            break;
        };
        total -= oldest.len() as u64 + 1;
        out.truncated = true;
    }
    for line in lines {
        out.text.push_str(&line);
        out.text.push('\n');
    }
    out
}

/// Reads at most `max_bytes` from the end of `path`, redacted, prefixed with
/// a note when earlier content was omitted; a missing or unreadable file
/// yields a one-line placeholder.
pub fn bounded_tail(path: &Path, max_bytes: u64) -> String {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) => return format!("[diagnostic log unavailable: {:?}]\n", e.kind()),
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = size.saturating_sub(max_bytes);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return "[diagnostic log unavailable: Seek]\n".into();
    }
    let mut data = Vec::new();
    if file.take(max_bytes).read_to_end(&mut data).is_err() {
        return "[diagnostic log unavailable: Read]\n".into();
    }
    let clean = redact_path(&String::from_utf8_lossy(&data));
    if size <= max_bytes && clean.len() as u64 <= max_bytes {
        return clean;
    }
    let budget = (max_bytes as usize).saturating_sub(OMITTED_PREFIX.len());
    let mut cut = clean.len().saturating_sub(budget);
    while cut < clean.len() && !clean.is_char_boundary(cut) {
        cut += 1;
    }
    format!("{OMITTED_PREFIX}{}", &clean[cut..])
}
