//! Uploads (`UploadManager`): a queue of clips, each handed to the
//! `fthrust-uploader` program once its readiness allows, one at a time on a
//! worker thread. The app itself links no upload code; it writes a JSON
//! request to the program and reads one JSON line back.

use std::collections::{HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use fthrust_core::clip_readiness::ReadinessState;
use fthrust_core::export_presets::ExportPreset;
use fthrust_core::library_ownership::{Ownership, classify_media_path};
use fthrust_core::upload::history::{Entry, History};
use fthrust_core::upload::settings::Provider;
use fthrust_core::upload::{CATBOX_LIMIT_BYTES, validate};
use serde_json::{Map, Value, json};

use crate::library::page::NotDeleted;
use crate::services::CardKind;
use crate::shell::error_bar::{Action, Level};
use crate::state::{AppState, DialogKind, DialogResult, StatusStyle, StatusText};

const MIB: f64 = 1024.0 * 1024.0;
/// Files modified more recently are left for the next interval scan.
const WRITE_SETTLE: Duration = Duration::from_secs(30);
/// How long `UPLOADED …` stays in the status bar.
const STATUS_HOLD_S: f64 = 2.0;
const MISSING: &str = "The uploader component is missing from this installation.";
const NO_REASON: &str = "The upload service did not provide a failure reason.";
/// Card overlay while a clip uploads.
pub const UPLOADING: &str = "Uploading…";
/// Question-dialog tag prefix for "compress before upload?".
pub const COMPRESS_TAG: &str = "upload-compress:";
/// Error-bar action prefix for `RETRY NOW`.
pub const RETRY_ACTION: &str = "upload-retry:";
/// Error-bar action prefix for `UPLOAD AGAIN` (a clip uploaded without a link).
pub const AGAIN_ACTION: &str = "upload-again:";
/// Error-bar action that opens the upload settings.
pub const OPEN_SETTINGS_ACTION: &str = "open-upload-settings";

/// Process timeouts per request kind (`_invoke_plugin`).
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    /// `upload`.
    pub upload: Duration,
    /// `test_connection`.
    pub test: Duration,
    /// `sftp_save` / `sftp_forget`.
    pub credentials: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            upload: Duration::from_secs(600),
            test: Duration::from_secs(75),
            credentials: Duration::from_secs(30),
        }
    }
}

/// The uploader's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// Success.
    pub ok: bool,
    /// Message (the link on success).
    pub message: String,
    /// The upload settings are wrong (URL, token); retrying cannot help.
    pub settings_problem: bool,
    /// What the user should know about an upload that worked: no link came
    /// back, the history was not saved.
    pub warning: Option<String>,
    /// The history entry of the upload, as the uploader made it. The history
    /// file is where the app learns it; this is for when that write failed.
    pub upload_info: Option<Map<String, Value>>,
    /// The upload is confirmed and the settings ask for the clip to be
    /// deleted: the uploader leaves that to the app (see
    /// [`delete_uploaded`]).
    pub delete: bool,
}

impl Reply {
    /// A failure with `message`.
    pub fn fail(message: impl Into<String>) -> Reply {
        Reply {
            ok: false,
            message: message.into(),
            settings_problem: false,
            warning: None,
            upload_info: None,
            delete: false,
        }
    }
}

/// The running uploader process, so quitting can stop it.
#[derive(Clone, Default)]
pub struct ChildSlot(Arc<Mutex<Option<u32>>>);

impl ChildSlot {
    fn set(&self, pid: Option<u32>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = pid;
    }

    /// Kills the running process, if any.
    pub fn kill(&self) {
        if let Some(pid) = *self.0.lock().unwrap_or_else(|e| e.into_inner()) {
            // The uploader runs in its own process group; take any
            // children with it so nothing keeps its pipes open.
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

/// The uploader program: `$FTHRUST_UPLOADER`, else `fthrust-uploader` next
/// to this executable.
pub fn uploader_exe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FTHRUST_UPLOADER") {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join("fthrust-uploader");
    p.is_file().then_some(p)
}

/// Runs one request (`_invoke_plugin`): the request on stdin, the last
/// non-blank stdout line parsed as the reply; the process is killed when
/// `timeout` passes.
pub fn run_request(exe: &Path, request: &Value, timeout: Duration, slot: &ChildSlot) -> Reply {
    use std::os::unix::process::CommandExt;
    let mut child = match Command::new(exe)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Reply::fail(MISSING),
    };
    slot.set(Some(child.id()));
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(request.to_string().as_bytes());
    }
    let read = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut r) = r {
                let _ = r.read_to_string(&mut s);
            }
            s
        })
    };
    let out = read(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let err = read(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                slot.kill();
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    slot.set(None);
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    let Some(status) = status else {
        return Reply::fail("The uploader did not finish in time.");
    };
    let Some(line) = stdout.lines().rev().find(|l| !l.trim().is_empty()) else {
        let e = stderr.trim();
        return Reply::fail(if e.is_empty() {
            format!("Uploader exited with code {}", status.code().unwrap_or(-1))
        } else {
            e.to_owned()
        });
    };
    match serde_json::from_str::<Value>(line.trim()) {
        Ok(Value::Object(m)) => Reply {
            ok: m.get("ok").and_then(Value::as_bool).unwrap_or(false),
            message: m
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            settings_problem: m.get("settings_problem").and_then(Value::as_bool) == Some(true),
            warning: m
                .get("warning")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(str::to_owned),
            upload_info: match m.get("upload_info") {
                Some(Value::Object(info)) if !info.is_empty() => Some(info.clone()),
                _ => None,
            },
            delete: m.get("delete").and_then(Value::as_bool) == Some(true),
        },
        _ => Reply::fail("Uploader returned an invalid response."),
    }
}

/// Uploads are on.
pub fn uploader_enabled(state: &AppState) -> bool {
    state.uploader_settings.enabled()
}

/// The clips folder: the only place an upload may delete a clip from.
fn clips_root(state: &AppState) -> PathBuf {
    state
        .settings
        .clips_directory()
        .unwrap_or_else(|_| state.library.clips_root.clone())
}

fn approved_roots(state: &AppState) -> Vec<PathBuf> {
    validate::allowed_roots(&clips_root(state), &state.settings)
}

enum Job {
    Upload {
        path: PathBuf,
        roots: Vec<PathBuf>,
        clips_root: PathBuf,
    },
    Compressed {
        path: PathBuf,
        roots: Vec<PathBuf>,
        clips_root: PathBuf,
        provider: Provider,
        limit: u64,
    },
}

/// Who asked for an upload, which decides what a clip that is uploaded
/// already gets.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ask {
    /// The immediate and interval modes: skipped, silently.
    Auto,
    /// The user (share button, menu, editor, RETRY NOW): skipped too, but a
    /// clip with no link to copy is explained, with the way out.
    User,
    /// The user, after that explanation: uploaded again.
    Again,
}

struct Done {
    path: PathBuf,
    /// The uploaded file's key in the history (its canonical path), taken
    /// before the upload: auto-delete removes the file, and a path that is
    /// gone cannot be resolved (behind a symlink it would come out spelled
    /// differently from the uploader's key).
    key: PathBuf,
    reply: Reply,
    /// The uploaded compressed copy: path, size, plan summary.
    compressed: Option<(PathBuf, u64, String)>,
    /// Compression failed (`COMPRESSION FAILED` toast).
    compression_failed: bool,
}

/// The upload queue and its worker.
pub struct UploadService {
    exe: Option<PathBuf>,
    timeouts: Arc<Mutex<Timeouts>>,
    limit: u64,
    waiting: VecDeque<(PathBuf, bool)>,
    in_flight: HashSet<PathBuf>,
    tx: Option<Sender<Job>>,
    rx: Receiver<Done>,
    thread: Option<JoinHandle<()>>,
    child: ChildSlot,
    cancel: Arc<AtomicBool>,
    status: Option<(StatusText, Option<f64>)>,
    status_started: bool,
    history_mtime: Option<SystemTime>,
    next_scan: Option<f64>,
    scan: Option<Receiver<Vec<PathBuf>>>,
    now: f64,
}

impl UploadService {
    /// The service with the installed uploader program.
    pub fn new() -> UploadService {
        Self::with_exe(uploader_exe())
    }

    /// The service with a given program (`None`: not installed).
    pub fn with_exe(exe: Option<PathBuf>) -> UploadService {
        let (tx, jobs) = channel::<Job>();
        let (out, rx) = channel();
        let child = ChildSlot::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let timeouts = Arc::new(Mutex::new(Timeouts::default()));
        let worker = Worker {
            exe: exe.clone(),
            child: child.clone(),
            cancel: cancel.clone(),
            timeouts: timeouts.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("fthr-uploader".into())
            .spawn(move || {
                for job in jobs {
                    if worker.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let done =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.run(&job)))
                            .unwrap_or_else(|_| {
                                let path = match &job {
                                    Job::Upload { path, .. } | Job::Compressed { path, .. } => {
                                        path.clone()
                                    }
                                };
                                Done {
                                    key: path.clone(),
                                    path,
                                    reply: Reply::fail("The upload stopped unexpectedly."),
                                    compressed: None,
                                    compression_failed: false,
                                }
                            });
                    if out.send(done).is_err() {
                        break;
                    }
                }
            })
            .ok();
        UploadService {
            exe,
            timeouts,
            limit: CATBOX_LIMIT_BYTES,
            waiting: VecDeque::new(),
            in_flight: HashSet::new(),
            tx: Some(tx),
            rx,
            thread,
            child,
            cancel,
            status: None,
            status_started: false,
            history_mtime: None,
            next_scan: None,
            scan: None,
            now: 0.0,
        }
    }

    /// Process timeouts.
    pub fn set_timeouts(&mut self, t: Timeouts) {
        *self.timeouts.lock().unwrap_or_else(|e| e.into_inner()) = t;
    }

    /// The provider size limit (tests use a small one).
    #[doc(hidden)]
    pub fn set_limit_bytes_for_tests(&mut self, bytes: u64) {
        self.limit = bytes;
    }

    /// The program, if installed.
    pub fn exe(&self) -> Option<&Path> {
        self.exe.as_deref()
    }

    /// A clip is queued or uploading.
    pub fn is_busy(&self, path: &Path) -> bool {
        self.in_flight.contains(path) || self.waiting.iter().any(|(p, _)| p == path)
    }

    /// Anything is queued, uploading or being scanned.
    pub fn busy(&self) -> bool {
        !self.in_flight.is_empty() || !self.waiting.is_empty() || self.scan.is_some()
    }

    /// `enqueue_upload`, for a request from the user (share button, menu,
    /// editor): queues a finished video once; silently ignored while uploads
    /// are off, and for a clip that is uploaded already, unless that clip has
    /// no link to copy: then the bar says so and offers to upload it again.
    pub fn enqueue(&mut self, state: &mut AppState, path: PathBuf) {
        self.queue(state, path, false, Ask::User);
    }

    /// `enqueue_compressed_upload`: compress a copy, then upload it.
    pub fn enqueue_compressed(&mut self, state: &mut AppState, path: PathBuf) {
        self.queue(state, path, true, Ask::Auto);
    }

    fn queue(&mut self, state: &mut AppState, path: PathBuf, compress: bool, ask: Ask) {
        if !uploader_enabled(state) {
            return;
        }
        if !validate::is_uploadable(&path) {
            tracing::info!(path = %path.display(), "[Uploader] Refused incomplete clip path");
            return;
        }
        let path = library_path(state, path);
        if self.is_busy(&path) {
            return;
        }
        let key = resolved(&path);
        if ask != Ask::Again && state.upload_history.is_uploaded(&key) {
            if ask == Ask::User && state.upload_history.link(&key).is_none() {
                explain_no_link(state, &path);
            }
            return;
        }
        state.upload_status.insert(path.clone(), UPLOADING.into());
        self.waiting.push_back((path, compress));
    }

    /// Error-bar actions this service owns; returns whether it handled `id`.
    pub fn error_action(&mut self, state: &mut AppState, id: &str) -> bool {
        if let Some(p) = id.strip_prefix(RETRY_ACTION) {
            self.enqueue(state, PathBuf::from(p));
            return true;
        }
        if let Some(p) = id.strip_prefix(AGAIN_ACTION) {
            self.queue(state, PathBuf::from(p), false, Ask::Again);
            return true;
        }
        if id == OPEN_SETTINGS_ACTION {
            state.page = crate::state::Page::Settings;
            state.settings_tab = crate::state::Tab::General;
            return true;
        }
        false
    }

    /// The `UPLOADED …` status while it holds.
    pub fn transient_status(&mut self, now: f64) -> Option<StatusText> {
        if let Some((_, until)) = &mut self.status {
            match until {
                None => *until = Some(now + STATUS_HOLD_S),
                Some(t) if now >= *t => {
                    self.status = None;
                    return None;
                }
                _ => {}
            }
        }
        self.status.as_ref().map(|(s, _)| s.clone())
    }

    /// Moves the queue along: answers, readiness, scans, results.
    pub fn tick(&mut self, state: &mut AppState, now: f64) {
        self.now = now;
        self.reload_history(state);
        self.take_answers(state);
        for clip in std::mem::take(&mut state.saved_clips) {
            if uploader_enabled(state) && state.uploader_settings.str("upload_mode") == "immediate"
            {
                self.queue(state, clip, false, Ask::Auto);
            }
        }
        self.interval(state, now);
        self.start_ready(state);
        while let Ok(done) = self.rx.try_recv() {
            self.apply(state, done);
        }
    }

    fn reload_history(&mut self, state: &mut AppState) {
        let mtime = std::fs::metadata(state.paths.upload_history_file())
            .and_then(|m| m.modified())
            .ok();
        if mtime != self.history_mtime {
            self.history_mtime = mtime;
            state.upload_history = History::load(&state.paths);
        }
    }

    fn take_answers(&mut self, state: &mut AppState) {
        let mut keep = Vec::new();
        for r in std::mem::take(&mut state.dialogs.results) {
            match &r {
                DialogResult::Answer { tag, yes } if tag.starts_with(COMPRESS_TAG) => {
                    if *yes {
                        let path = PathBuf::from(&tag[COMPRESS_TAG.len()..]);
                        self.enqueue_compressed(state, path);
                    }
                }
                _ => keep.push(r),
            }
        }
        state.dialogs.results = keep;
    }

    fn interval(&mut self, state: &mut AppState, now: f64) {
        if let Some(rx) = &self.scan
            && let Ok(found) = rx.try_recv()
        {
            self.scan = None;
            for p in found {
                self.queue(state, p, false, Ask::Auto);
            }
        }
        let s = &state.uploader_settings;
        if !uploader_enabled(state) || s.str("upload_mode") != "interval" {
            self.next_scan = None;
            return;
        }
        let value = s
            .get("upload_interval_value")
            .and_then(Value::as_f64)
            .filter(|v| *v > 0.0)
            .unwrap_or(5.0);
        let unit = match s.str("upload_interval_unit") {
            "hours" => 3600.0,
            "days" => 86_400.0,
            _ => 60.0,
        };
        let period = value * unit;
        let due = *self.next_scan.get_or_insert(now + period);
        if now < due || self.scan.is_some() {
            return;
        }
        self.next_scan = Some(now + period);
        let roots = approved_roots(state);
        let (tx, rx) = channel();
        self.scan = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(scan(&roots));
        });
    }

    fn start_ready(&mut self, state: &mut AppState) {
        if !uploader_enabled(state) {
            // Switched off: what has not started is dropped quietly.
            for (path, _) in self.waiting.drain(..) {
                state.upload_status.remove(&path);
            }
            return;
        }
        // One at a time, so the enabled flag, the provider and the size
        // decision are taken when each upload really starts.
        if !self.in_flight.is_empty() {
            return;
        }
        let mut later = VecDeque::new();
        while let Some((path, compress)) = self.waiting.pop_front() {
            match state.readiness.state(&path) {
                ReadinessState::EngineCommitted | ReadinessState::Finalizing => {
                    later.push_back((path, compress));
                    continue;
                }
                ReadinessState::FinalizationFailed => {
                    tracing::warn!(path = %path.display(), "[Upload] Failed — Clip finalization failed; upload was not started");
                    state.upload_status.remove(&path);
                    continue;
                }
                _ => {}
            }
            if !uploader_enabled(state) {
                state.upload_status.remove(&path);
                continue;
            }
            let provider = state.uploader_settings.provider();
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let oversized = provider == Provider::Catbox && size > self.limit;
            if oversized && !compress && !state.uploader_settings.flag("upload_auto_compress") {
                state.upload_status.remove(&path);
                let name = file_name(&path);
                state.dialogs.push_modal(DialogKind::Question {
                    title: "Compress before upload?".into(),
                    message: format!(
                        "{name} is {:.1} MB. Catbox accepts files up to {} MB.\n\nAutomatically compress a copy and upload it? The original clip will stay unchanged.",
                        size as f64 / MIB,
                        self.limit / (1024 * 1024)
                    ),
                    tag: format!("{COMPRESS_TAG}{}", path.display()),
                });
                continue;
            }
            let roots = approved_roots(state);
            let clips_root = clips_root(state);
            let job = if oversized {
                Job::Compressed {
                    path: path.clone(),
                    roots,
                    clips_root,
                    provider,
                    limit: self.limit,
                }
            } else {
                Job::Upload {
                    path: path.clone(),
                    roots,
                    clips_root,
                }
            };
            self.in_flight.insert(path.clone());
            state.upload_status.insert(path, UPLOADING.into());
            if let Some(tx) = &self.tx {
                let _ = tx.send(job);
            }
            break;
        }
        later.extend(self.waiting.drain(..));
        self.waiting = later;
    }

    fn apply(&mut self, state: &mut AppState, done: Done) {
        self.in_flight.remove(&done.path);
        state.upload_status.remove(&done.path);
        let name = file_name(&done.path);
        if done.compression_failed {
            state.error_bar.push_error(
                "COMPRESSION FAILED",
                &done.reply.message,
                Level::Warning,
                Vec::new(),
                None,
                false,
            );
        }
        if !done.reply.ok {
            let detail = if done.reply.message.trim().is_empty() {
                NO_REASON.to_owned()
            } else {
                done.reply.message.trim().to_owned()
            };
            state.pending_cards.push(CardKind::UploadFailed {
                detail: detail.clone(),
            });
            let mut actions = Vec::new();
            if done.reply.settings_problem {
                actions.push(Action {
                    label: "OPEN UPLOAD SETTINGS".into(),
                    id: OPEN_SETTINGS_ACTION.into(),
                });
            }
            actions.push(Action {
                label: "RETRY NOW".into(),
                id: format!("{RETRY_ACTION}{}", done.path.display()),
            });
            state.error_bar.push_error(
                "UPLOAD FAILED",
                &detail,
                Level::Warning,
                actions,
                Some(format!("upload:{}", done.path.display())),
                true,
            );
            return;
        }
        let compressed = done.compressed.is_some();
        // The uploader records the upload in the history itself. When it
        // could not (it says so in its warning), its reply still carries the
        // entry: save it from here, and failing that keep it for this
        // session (below), so the card still offers the link and the clip is
        // not uploaded again.
        let unsaved = if done.reply.warning.is_some() {
            unsaved_entry(&state.paths, &done.key, done.reply.upload_info.as_ref())
        } else {
            None
        };
        if let Some((_, size, _)) = &done.compressed {
            let original = resolved(&done.path);
            if let Err(e) = History::update(&state.paths, |h| {
                h.alias_compressed(&original, &done.key, *size)
            }) {
                tracing::warn!(error = %e, "upload history alias not saved");
            }
        }
        self.history_mtime = None;
        self.reload_history(state);
        if let Some(entry) = unsaved {
            state.upload_history.record(&done.key, entry);
            if let Some((_, size, _)) = &done.compressed {
                let original = resolved(&done.path);
                state
                    .upload_history
                    .alias_compressed(&original, &done.key, *size);
            }
        }
        if let Some(warning) = &done.reply.warning {
            state.error_bar.push_error(
                "UPLOAD WARNING",
                warning,
                Level::Warning,
                Vec::new(),
                Some(format!("upload-warning:{}", done.path.display())),
                false,
            );
        }
        // The uploader says the clip may go; whether it does is up to the
        // library's rules, asked now rather than when the upload started.
        if done.reply.delete && done.reply.warning.is_none() {
            // The file that went out: the clip, or the compressed copy of it.
            let sent = done
                .compressed
                .as_ref()
                .map_or(done.path.as_path(), |(copy, _, _)| copy.as_path());
            delete_uploaded(state, sent);
        }
        let (text, kind) = if compressed {
            ("UPLOADED COMPRESSED COPY", "COMPRESSED COPY")
        } else {
            ("UPLOADED ORIGINAL", "ORIGINAL")
        };
        self.status = Some((
            StatusText {
                text: text.into(),
                style: StatusStyle::Idle,
            },
            None,
        ));
        self.status_started = true;
        state.pending_cards.push(CardKind::Upload {
            filename: format!("{name} · {kind}"),
        });
        if let Some((_, size, summary)) = &done.compressed {
            let message = format!(
                "Uploaded compressed copy ({:.1} MB); original preserved. {summary}",
                *size as f64 / MIB
            );
            state.error_bar.push_error(
                "COMPRESSED COPY UPLOADED",
                &message,
                Level::Warning,
                Vec::new(),
                None,
                false,
            );
        }
    }

    /// Stops the worker and kills a running uploader.
    pub fn shutdown(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.tx = None;
        self.child.kill();
        if let Some(t) = self.thread.take() {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !t.is_finished() && Instant::now() < deadline {
                self.child.kill();
                std::thread::sleep(Duration::from_millis(20));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

impl Default for UploadService {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for UploadService {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.child.kill();
    }
}

/// The library's own spelling of `path` (scans see resolved paths; cards,
/// renames and deletes use the library's), so one clip is one queue entry.
/// The records carry their resolved path: only `path` is resolved here, once.
fn library_path(state: &AppState, path: PathBuf) -> PathBuf {
    if state.library.records.iter().any(|r| r.path == path) {
        return path;
    }
    let real = resolved(&path);
    state
        .library
        .records
        .iter()
        .find(|r| r.identity == real)
        .map(|r| r.path.clone())
        .unwrap_or(path)
}

/// The entry the uploader made for `uploaded`, if the history file lacks it
/// after the uploader's own write (it failed) and ours (`None`: the file has
/// it, or the reply had none).
fn unsaved_entry(
    paths: &fthrust_core::FthrPaths,
    uploaded: &Path,
    info: Option<&Map<String, Value>>,
) -> Option<Entry> {
    let entry = Entry(info?.clone());
    if History::load(paths).get(uploaded).is_some() {
        return None;
    }
    match History::update(paths, |h| h.record(uploaded, entry.clone())) {
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(error = %e, "upload history entry not saved");
            Some(entry)
        }
    }
}

/// Auto-delete: removes the file an upload confirmed, `sent`, the way the bin
/// button would (`library::page::remove_clip`). The uploader leaves this to
/// the app because only the app knows what has become of the clip since the
/// upload began: an export or share that reads it keeps it (and the bar says
/// so), and an editor opened on it closes with it. Nothing outside the clips
/// folder is ever removed, imported libraries least of all.
fn delete_uploaded(state: &mut AppState, sent: &Path) {
    // Switched off while the upload ran: the setting counts at this moment.
    if !state.uploader_settings.flag("upload_auto_delete") {
        return;
    }
    let imports = crate::library::page::imported_roots(state);
    let ours =
        classify_media_path(sent, &state.library.clips_root, &imports) == Ownership::FthrOwned;
    // Gone already (deleted in a file manager meanwhile): the library's
    // watcher takes it from here.
    if !ours || !sent.is_file() {
        return;
    }
    let name = file_name(sent);
    match crate::library::page::remove_clip(state, sent) {
        Ok(()) => {}
        Err(NotDeleted::Busy) => notice(
            state,
            "UPLOAD WARNING",
            &format!(
                "{name} was uploaded, but not deleted: an export or share of it is queued or running. Delete it yourself when that has finished."
            ),
            Vec::new(),
            format!("upload-warning:{}", sent.display()),
            false,
        ),
        Err(NotDeleted::Failed(e)) => notice(
            state,
            "UPLOAD WARNING",
            &format!("Uploaded, but local delete failed: {e}"),
            Vec::new(),
            format!("upload-warning:{}", sent.display()),
            false,
        ),
    }
}

/// A bar entry about what an upload came to. Shown whatever "Show capture
/// error messages" says: it holds what nothing else tells (a link, why a
/// clip was kept, why a button does nothing).
fn notice(
    state: &mut AppState,
    title: &str,
    detail: &str,
    actions: Vec<Action>,
    key: String,
    dismiss_on_action: bool,
) {
    let detail: String = detail.trim().chars().take(240).collect();
    state.error_bar.push(
        title,
        &detail,
        Level::Warning,
        actions,
        Some(key),
        dismiss_on_action,
    );
}

/// The user asked to upload a clip that is uploaded already but has no link
/// (its server sent none back, so the card offers "Upload clip" and nothing
/// would happen): say so, and offer to upload it again.
fn explain_no_link(state: &mut AppState, path: &Path) {
    let name = file_name(path);
    state.error_bar.push_error(
        "ALREADY UPLOADED",
        &format!(
            "{name} was uploaded, but its server sent no link back, so there is nothing to copy. Upload it again to try for one."
        ),
        Level::Warning,
        vec![Action {
            label: "UPLOAD AGAIN".into(),
            id: format!("{AGAIN_ACTION}{}", path.display()),
        }],
        Some(format!("{AGAIN_ACTION}{}", path.display())),
        true,
    );
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Completed videos under `roots` last written at least 30 s ago.
fn scan(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = roots.to_vec();
    let mut seen = HashSet::new();
    while let Some(dir) = stack.pop() {
        if !seen.insert(dir.clone()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                stack.push(p);
            } else if validate::is_uploadable(&p)
                && meta
                    .modified()
                    .ok()
                    .and_then(|m| m.elapsed().ok())
                    .is_some_and(|age| age >= WRITE_SETTLE)
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

struct Worker {
    exe: Option<PathBuf>,
    child: ChildSlot,
    cancel: Arc<AtomicBool>,
    timeouts: Arc<Mutex<Timeouts>>,
}

impl Worker {
    fn upload(&self, path: &Path, roots: &[PathBuf], clips_root: &Path) -> Reply {
        let Some(exe) = &self.exe else {
            return Reply::fail(MISSING);
        };
        if !path.is_file() {
            return Reply::fail("File no longer exists (deleted before upload)");
        }
        // `clips_root` is the only folder whose clips the uploader reports as
        // deletable after an upload (the app does the deleting); imported
        // folders are in `roots` but never deleted from.
        let request = json!({
            "action": "upload",
            "path": path,
            "allowed_roots": roots,
            "clips_root": clips_root,
        });
        let timeout = self
            .timeouts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .upload;
        let reply = run_request(exe, &request, timeout, &self.child);
        if !reply.ok && reply.message.is_empty() {
            return Reply::fail("Upload failed");
        }
        reply
    }

    fn run(&self, job: &Job) -> Done {
        match job {
            Job::Upload {
                path,
                roots,
                clips_root,
            } => {
                // The key before the upload: auto-delete may remove the file.
                let key = resolved(path);
                Done {
                    path: path.clone(),
                    key,
                    reply: self.upload(path, roots, clips_root),
                    compressed: None,
                    compression_failed: false,
                }
            }
            Job::Compressed {
                path,
                roots,
                clips_root,
                provider,
                limit,
            } => match compress(path, clips_root, *provider, *limit, &self.cancel) {
                Err((message, failed)) => Done {
                    path: path.clone(),
                    key: path.clone(),
                    reply: Reply::fail(message),
                    compressed: None,
                    compression_failed: failed,
                },
                Ok((copy, size, summary)) => {
                    let key = resolved(&copy);
                    let reply = self.upload(&copy, roots, clips_root);
                    let ok = reply.ok;
                    Done {
                        path: path.clone(),
                        key,
                        reply,
                        compressed: ok.then_some((copy, size, summary)),
                        compression_failed: false,
                    }
                }
            },
        }
    }
}

/// `_compress_then_upload`'s copy: the whole clip planned to the
/// provider's limit (60 fps, H.264, 128 kbps audio, 94 % margin) into
/// `Shared/<stem>_<provider>_upload_<HH-MM-SS>[_N].mp4`. The error flag
/// marks failures that show `COMPRESSION FAILED`.
fn compress(
    source: &Path,
    clips_root: &Path,
    provider: Provider,
    limit: u64,
    cancel: &AtomicBool,
) -> Result<(PathBuf, u64, String), (String, bool)> {
    use fthrust_core::export::filters::{MixRole, MixSource};
    use fthrust_core::export::graph::ExportSpec;
    use fthrust_media::export::{ExportState, Job, Quality};

    let shared = clips_root.join("Shared");
    std::fs::create_dir_all(&shared)
        .map_err(|e| (format!("Could not create compression folder: {e}"), false))?;
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let stamp = format!("{:02}-{:02}-{:02}", now.hour(), now.minute(), now.second());
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id = provider.id();
    let mut output = shared.join(format!("{stem}_{id}_upload_{stamp}.mp4"));
    let mut n = 2;
    while output.exists() {
        output = shared.join(format!("{stem}_{id}_upload_{stamp}_{n}.mp4"));
        n += 1;
    }
    let failed = |e: String| (format!("Automatic compression failed: {e}"), true);
    let facts = fthrust_media::export::media_facts(source).map_err(failed)?;
    let mut title = id.to_owned();
    if let Some(first) = title.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    let preset = ExportPreset {
        preset_id: format!("upload-{id}"),
        name: format!("{title} upload"),
        target_size_mb: limit as f64 / MIB,
        target_width: None,
        target_height: None,
        target_fps: Some(60.0),
        video_codec: "h264".into(),
        audio_bitrate_kbps: 128,
        built_in: false,
        safety_margin: 0.94,
    };
    let plan = fthrust_core::export::plan::plan_export(&facts, &preset, None);
    let spec = ExportSpec {
        ranges_s: vec![(0.0, facts.duration_s.max(0.1))],
        crop: None,
        effects: Default::default(),
        stretch: 1.0,
        speed: 1.0,
        preserve_pitch: false,
        sources: if facts.has_audio {
            vec![MixSource {
                audio_index: Some(0),
                available: true,
                role: MixRole::Direct,
                gain_percent: 100,
                muted: false,
            }]
        } else {
            Vec::new()
        },
        master_percent: 100,
        has_manifest_tracks: false,
        has_audio: facts.has_audio,
        watermark: None,
        audio_bitrate_kbps: plan.audio_kbps.max(32),
        extra_video_filters: plan.preset_filters(&facts),
        force_video_encode: true,
        force_audio_encode: facts.has_audio,
    };
    let graph = fthrust_core::export::graph::plan(&spec).map_err(failed)?;
    let job = Job {
        source: source.to_path_buf(),
        output: output.clone(),
        plan: graph,
        quality: Quality::Quality,
        video_bitrate_kbps: Some(plan.video_kbps),
        max_rate_kbps: Some(plan.video_kbps * 115 / 100),
        expected_duration_s: facts.duration_s,
        target_size_mb: Some(preset.target_size_mb),
        output_fps: plan.output_fps(&facts),
        decode_from_s: 0.0,
        copy_metadata: false,
        inactivity_s: None,
        fail_first_encoder: false,
    };
    let r = fthrust_media::export::run(&job, cancel, &mut |_| {}, &mut |_| {});
    if r.state != ExportState::Completed {
        let _ = std::fs::remove_file(&output);
        let line = r
            .detail
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("Export failed")
            .to_owned();
        return Err(failed(line));
    }
    let size = std::fs::metadata(&output)
        .map_err(|e| (format!("Compressed file could not be verified: {e}"), false))?
        .len();
    if size > limit {
        let _ = std::fs::remove_file(&output);
        return Err((
            "Compressed file still exceeds the provider limit; upload was not started".into(),
            false,
        ));
    }
    Ok((output, size, plan.summary()))
}
