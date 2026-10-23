//! The export queue (`core/export_queue.py`): editor exports run on worker
//! threads, admitted first-in first-out under the parallel limit; share
//! exports take the same slots without being listed. The UI reads
//! snapshots through [`ExportQueue::tasks`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fthrust_core::export::naming::export_name;
use fthrust_core::settings::SettingsStore;
use fthrust_media::export::{ExportResult, ExportState, Job, Quality, random_hex};

/// Runs one job: `(job, cancel, on_status, on_progress)`.
pub type Runner = Arc<
    dyn Fn(&Job, &AtomicBool, &mut dyn FnMut(&str), &mut dyn FnMut(f64)) -> ExportResult
        + Send
        + Sync,
>;

/// A queued export's id.
pub type TaskId = u64;

/// Records kept besides running ones.
const KEEP: usize = 50;

/// How often a waiting job re-checks its cancel flag.
const WAIT_STEP: Duration = Duration::from_millis(50);

/// A snapshot of one queued export (`ExportTask`).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskView {
    /// Id.
    pub id: TaskId,
    /// The clip.
    pub source: PathBuf,
    /// The output path.
    pub output: PathBuf,
    /// Quality.
    pub quality: Quality,
    /// Status line text.
    pub status: String,
    /// Percent.
    pub progress: f64,
    /// Failure text.
    pub detail: String,
    /// Finished (any terminal state).
    pub done: bool,
}

struct Record {
    view: TaskView,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct Inner {
    records: Vec<Record>,
    /// Admission tickets in arrival order.
    waiting: VecDeque<u64>,
    active: usize,
    limit: usize,
    next_id: u64,
    runner: Runner,
}

struct Shared {
    inner: Mutex<Inner>,
    slots: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn update(&self, id: TaskId, f: impl FnOnce(&mut TaskView)) {
        if let Some(r) = self.lock().records.iter_mut().find(|r| r.view.id == id) {
            f(&mut r.view);
        }
    }

    /// Waits for `ticket`'s turn and a free slot; `None` when cancelled
    /// first. On `Some` the slot is held and the runner returned.
    fn admit(&self, ticket: u64, cancel: &AtomicBool) -> Option<Runner> {
        let mut inner = self.lock();
        loop {
            if cancel.load(Ordering::Relaxed) {
                inner.waiting.retain(|&t| t != ticket);
                self.slots.notify_all();
                return None;
            }
            if inner.waiting.front() == Some(&ticket) && inner.active < inner.limit {
                inner.waiting.pop_front();
                inner.active += 1;
                self.slots.notify_all();
                return Some(inner.runner.clone());
            }
            inner = self
                .slots
                .wait_timeout(inner, WAIT_STEP)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn release(&self) {
        let mut inner = self.lock();
        inner.active = inner.active.saturating_sub(1);
        self.slots.notify_all();
    }

    /// Admits, runs and releases; a cancel while waiting ends as cancelled.
    fn run(
        &self,
        ticket: u64,
        job: &Job,
        cancel: &AtomicBool,
        on_status: &mut dyn FnMut(&str),
        on_progress: &mut dyn FnMut(f64),
    ) -> ExportResult {
        let Some(runner) = self.admit(ticket, cancel) else {
            return ExportResult {
                state: ExportState::Cancelled,
                detail: "Export cancelled".into(),
            };
        };
        on_status("Preparing");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runner(job, cancel, on_status, on_progress)
        }))
        .unwrap_or_else(|_| ExportResult {
            state: ExportState::Failed,
            detail: "Export worker failed".into(),
        });
        self.release();
        result
    }
}

/// The queue. Clones share it.
#[derive(Clone)]
pub struct ExportQueue {
    shared: Arc<Shared>,
}

impl Default for ExportQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ExportQueue {
    /// A queue running exports in-process.
    pub fn new() -> ExportQueue {
        ExportQueue::with_runner(Arc::new(|job, cancel, status, progress| {
            fthrust_media::export::run(job, cancel, status, progress)
        }))
    }

    /// A queue with another runner (tests).
    pub fn with_runner(runner: Runner) -> ExportQueue {
        ExportQueue {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    records: Vec::new(),
                    waiting: VecDeque::new(),
                    active: 0,
                    limit: 1,
                    next_id: 1,
                    runner,
                }),
                slots: Condvar::new(),
            }),
        }
    }

    /// Replaces the runner for jobs admitted from now on (tests).
    pub fn set_runner(&self, runner: Runner) {
        self.shared.lock().runner = runner;
    }

    /// The slot count for the settings: 1, or `parallel_export_limit`
    /// clamped to 2..4 when `parallel_exports` is on.
    pub fn limit_for(settings: &SettingsStore) -> usize {
        if !settings.get_bool("parallel_exports").unwrap_or(false) {
            return 1;
        }
        let requested = settings
            .get_i64("parallel_export_limit")
            .or_else(|| {
                settings
                    .get_str("parallel_export_limit")
                    .and_then(|s| s.trim().parse().ok())
            })
            .unwrap_or(2);
        requested.clamp(2, 4) as usize
    }

    /// `configure`: applies the limit and wakes waiting jobs.
    pub fn configure(&self, settings: &SettingsStore) {
        self.shared.lock().limit = Self::limit_for(settings);
        self.shared.slots.notify_all();
    }

    /// Queues an editor export of `source` into `directory`. `build`
    /// receives the output path and returns the job.
    pub fn submit(
        &self,
        source: PathBuf,
        directory: &Path,
        quality: Quality,
        build: Box<dyn FnOnce(&Path) -> Job + Send>,
        expected_s: f64,
    ) -> TaskId {
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "clip".into());
        let output = directory.join(export_name(
            &stem,
            &random_hex(12),
            quality == Quality::Lossless,
        ));
        let mut job = build(&output);
        job.output = output.clone();
        if job.expected_duration_s <= 0.0 {
            job.expected_duration_s = expected_s;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let mut inner = self.shared.lock();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.waiting.push_back(id);
        let len = inner.records.len();
        if len > KEEP {
            let mut kept: Vec<Record> = inner
                .records
                .drain(..len - KEEP)
                .filter(|r| !r.view.done)
                .collect();
            kept.append(&mut inner.records);
            inner.records = kept;
        }
        inner.records.push(Record {
            view: TaskView {
                id,
                source,
                output,
                quality,
                status: "Queued".into(),
                progress: 0.0,
                detail: String::new(),
                done: false,
            },
            cancel: cancel.clone(),
            thread: None,
        });
        let shared = self.shared.clone();
        let thread = std::thread::Builder::new()
            .name("fthr-export".into())
            .spawn(move || {
                let result = shared.run(
                    id,
                    &job,
                    &cancel,
                    &mut |s: &str| shared.update(id, |v| v.status = s.to_owned()),
                    &mut |p| shared.update(id, |v| v.progress = p),
                );
                shared.update(id, |v| {
                    v.status = result.state.status().to_owned();
                    v.detail = if result.state == ExportState::Completed {
                        String::new()
                    } else {
                        result.detail
                    };
                    if result.state == ExportState::Completed {
                        v.progress = 100.0;
                    }
                    v.done = true;
                });
                shared.slots.notify_all();
            });
        match thread {
            Ok(handle) => {
                if let Some(r) = inner.records.iter_mut().find(|r| r.view.id == id) {
                    r.thread = Some(handle);
                }
            }
            Err(e) => {
                inner.waiting.retain(|&t| t != id);
                if let Some(r) = inner.records.iter_mut().find(|r| r.view.id == id) {
                    r.view.status = "Failed".into();
                    r.view.detail = e.to_string();
                    r.view.done = true;
                }
            }
        }
        id
    }

    /// Runs a share export on the calling thread through the same slots;
    /// not listed in [`ExportQueue::tasks`].
    pub fn run_share(
        &self,
        job: &Job,
        cancel: &AtomicBool,
        on_status: &mut dyn FnMut(&str),
        on_progress: &mut dyn FnMut(f64),
    ) -> ExportResult {
        let ticket = {
            let mut inner = self.shared.lock();
            let t = inner.next_id;
            inner.next_id += 1;
            inner.waiting.push_back(t);
            t
        };
        on_status("Queued");
        self.shared.run(ticket, job, cancel, on_status, on_progress)
    }

    /// Adds a finished-looking record without a worker (fixtures).
    #[doc(hidden)]
    pub fn insert_for_tests(&self, view: TaskView) {
        self.shared.lock().records.push(Record {
            view,
            cancel: Arc::new(AtomicBool::new(false)),
            thread: None,
        });
    }

    /// Snapshots, oldest first.
    pub fn tasks(&self) -> Vec<TaskView> {
        self.shared
            .lock()
            .records
            .iter()
            .map(|r| r.view.clone())
            .collect()
    }

    /// Requests cancel and shows `Cancelling` until the worker ends.
    pub fn cancel(&self, id: TaskId) {
        let mut inner = self.shared.lock();
        if let Some(r) = inner.records.iter_mut().find(|r| r.view.id == id)
            && !r.view.done
        {
            r.cancel.store(true, Ordering::Relaxed);
            r.view.status = "Cancelling".into();
        }
        self.shared.slots.notify_all();
    }

    /// `(pending, failed, completed)` for the editor's button label.
    pub fn counts(&self) -> (usize, usize, usize) {
        let inner = self.shared.lock();
        let v = inner.records.iter().map(|r| &r.view);
        let pending = v.clone().filter(|t| !t.done).count();
        let failed = v
            .clone()
            .filter(|t| t.done && (t.status == "Failed" || t.status == "Timed_Out"))
            .count();
        let completed = v.filter(|t| t.status == "Completed").count();
        (pending, failed, completed)
    }

    /// The "Export jobs" button suffix for [`ExportQueue::counts`].
    pub fn label_suffix((pending, failed, completed): (usize, usize, usize)) -> String {
        if pending > 0 {
            format!(" · {pending} pending")
        } else if failed > 0 {
            format!(" · {failed} failed")
        } else if completed > 0 {
            format!(" · {completed} completed")
        } else {
            String::new()
        }
    }

    /// Whether `source` has an export queued or running.
    pub fn active_for(&self, source: &Path) -> bool {
        self.shared
            .lock()
            .records
            .iter()
            .any(|r| !r.view.done && r.view.source == source)
    }

    /// Cancels every unfinished export and waits for the workers against
    /// one shared deadline.
    pub fn shutdown(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let handles: Vec<JoinHandle<()>> = {
            let mut inner = self.shared.lock();
            for r in inner.records.iter().filter(|r| !r.view.done) {
                r.cancel.store(true, Ordering::Relaxed);
            }
            inner
                .records
                .iter_mut()
                .filter_map(|r| r.thread.take())
                .collect()
        };
        self.shared.slots.notify_all();
        for h in handles {
            while !h.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }
}
