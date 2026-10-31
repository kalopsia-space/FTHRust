//! The library scan service: one cancellable scan at a time (a follow-up
//! request is retained), a coalesced filesystem watcher on the clips root,
//! its subfolders and the import roots, and a 30 s timer refresh.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fthrust_core::clip_files::is_partial_name;
use fthrust_core::clip_readiness::canonical_key;
use fthrust_core::library_index::{LibraryRecord, ScanRoots, is_media_name, scan_with_dirs};
use notify::Watcher;
use notify::event::{CreateKind, ModifyKind, RemoveKind};

/// Why a scan ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanKind {
    /// The page asked (startup, folder change, manual refresh).
    Foreground,
    /// The filesystem watcher saw a change.
    Watcher,
    /// The 30 s timer.
    Timer,
}

/// What the service reports.
#[derive(Clone, Debug)]
pub enum ScanEvent {
    /// The first discovery batch of a scan.
    Batch {
        /// Scan generation.
        generation: u64,
        /// Records so far.
        records: Vec<LibraryRecord>,
    },
    /// A scan finished.
    Done {
        /// Scan generation.
        generation: u64,
        /// Why it ran.
        kind: ScanKind,
        /// Every record.
        records: Vec<LibraryRecord>,
        /// Directories now watched.
        watched: Vec<PathBuf>,
        /// Wall time.
        elapsed_ms: u64,
    },
    /// A scan failed (storage unavailable).
    Failed {
        /// Why it ran.
        kind: ScanKind,
        /// The error.
        error: String,
    },
}

/// Watcher events are coalesced for this long.
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(200);
/// Periodic refresh.
pub const TIMER_REFRESH: Duration = Duration::from_secs(30);

struct Shared {
    roots: Mutex<ScanRoots>,
    pending: Mutex<Option<ScanKind>>,
    cancel: Mutex<Arc<AtomicBool>>,
    stop: AtomicBool,
    generation: Mutex<u64>,
    /// Files written under their final name (a manual recording), by the name
    /// they were given and by their real name: their data changes start no
    /// scan.
    muted: Mutex<Vec<PathBuf>>,
}

/// The service handle.
pub struct ScanService {
    shared: Arc<Shared>,
    events: Receiver<ScanEvent>,
    forward: Arc<Mutex<Option<Sender<ScanEvent>>>>,
    wake: Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ScanService {
    /// Starts the worker and the watcher for `clips_root` and `imported`.
    pub fn start(clips_root: PathBuf, imported: Vec<PathBuf>) -> ScanService {
        let shared = Arc::new(Shared {
            roots: Mutex::new(ScanRoots {
                owned: clips_root,
                imported,
            }),
            pending: Mutex::new(None),
            cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
            stop: AtomicBool::new(false),
            generation: Mutex::new(0),
            muted: Mutex::new(Vec::new()),
        });
        let (event_tx, event_rx) = channel();
        let (wake_tx, wake_rx) = channel::<()>();
        let worker_shared = shared.clone();
        let watch_wake = wake_tx.clone();
        let forward: Arc<Mutex<Option<Sender<ScanEvent>>>> = Arc::new(Mutex::new(None));
        let forward_w = forward.clone();
        let thread = std::thread::Builder::new()
            .name("fthrust-library-scan".into())
            .spawn(move || {
                worker(
                    worker_shared,
                    Emit {
                        direct: event_tx,
                        forward: forward_w,
                    },
                    wake_rx,
                    watch_wake,
                )
            })
            .expect("spawn scan worker");
        ScanService {
            shared,
            events: event_rx,
            forward,
            wake: wake_tx,
            thread: Some(thread),
        }
    }

    /// Asks for a scan; an active scan is cancelled and one follow-up kept.
    pub fn request(&self, kind: ScanKind) {
        {
            let mut pending = self
                .shared
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // A foreground request outranks a watcher or timer one.
            *pending = Some(match (*pending, kind) {
                (Some(ScanKind::Foreground), _) | (_, ScanKind::Foreground) => ScanKind::Foreground,
                (_, k) => k,
            });
        }
        if let Ok(c) = self.shared.cancel.lock() {
            c.store(true, Ordering::Relaxed);
        }
        let _ = self.wake.send(());
    }

    /// Changes the roots (clips directory or import folders) and rescans.
    pub fn set_roots(&self, clips_root: PathBuf, imported: Vec<PathBuf>) {
        if let Ok(mut r) = self.shared.roots.lock() {
            *r = ScanRoots {
                owned: clips_root,
                imported,
            };
        }
        self.request(ScanKind::Foreground);
    }

    /// A file is being written under its final name, as a manual recording
    /// is: writes to it start no scan, which would walk the whole tree for
    /// every fragment of the recording. It appearing, going and being renamed
    /// still do. Nothing written in between is seen, so whoever calls
    /// [`Self::unmute_writes`] also asks for a scan.
    pub fn mute_writes(&self, path: &Path) {
        let mut muted = self.shared.muted.lock().unwrap_or_else(|e| e.into_inner());
        for name in [path.to_path_buf(), canonical_key(path)] {
            if !muted.contains(&name) {
                muted.push(name);
            }
        }
    }

    /// The writing is over: the file's data changes count again, whichever
    /// name it is given here.
    pub fn unmute_writes(&self, path: &Path) {
        let key = canonical_key(path);
        self.shared
            .muted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|m| m != path && canonical_key(m) != key);
    }

    /// The files whose writes are ignored, under the names they were given.
    pub fn muted_writes(&self) -> Vec<PathBuf> {
        self.shared
            .muted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Scan results (direct channel).
    pub fn events(&self) -> &Receiver<ScanEvent> {
        &self.events
    }

    /// Also sends every event to `tx` (the services outbox).
    pub fn forward_to(&self, tx: Sender<ScanEvent>) {
        if let Ok(mut f) = self.forward.lock() {
            *f = Some(tx);
        }
    }

    /// Stops the worker.
    pub fn stop(mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Ok(c) = self.shared.cancel.lock() {
            c.store(true, Ordering::Relaxed);
        }
        let _ = self.wake.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ScanService {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        let _ = self.wake.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Emit {
    direct: Sender<ScanEvent>,
    forward: Arc<Mutex<Option<Sender<ScanEvent>>>>,
}

impl Emit {
    fn send(&self, ev: ScanEvent) {
        // Once a forwarder is installed it is the only reader: the direct
        // channel would otherwise fill with every scan's record list.
        if let Ok(f) = self.forward.lock()
            && let Some(tx) = f.as_ref()
        {
            let _ = tx.send(ev);
            return;
        }
        let _ = self.direct.send(ev);
    }
}

/// Whether a filesystem event can change what a scan lists. A save grows its
/// `.partial` file for as long as it runs, and the engine writes sidecars next
/// to the clips; neither is something the library shows. A manual recording is
/// written straight into its final name, which the library does list, so the
/// writes to it are told apart by name: `muted` holds the files being written
/// (see [`ScanService::mute_writes`]). A scan for each write would walk the
/// whole tree for as long as the recording runs, during a game.
pub fn watcher_event_matters(ev: &notify::Event, muted: &[PathBuf]) -> bool {
    let changed = matches!(
        ev.kind,
        notify::EventKind::Create(_) | notify::EventKind::Remove(_) | notify::EventKind::Modify(_)
    );
    if !changed {
        return false;
    }
    // A new or removed folder is a game that appears or goes; a rename can
    // be a file taking its final name or a move into or out of the tree.
    let structural = matches!(
        ev.kind,
        notify::EventKind::Create(CreateKind::Folder)
            | notify::EventKind::Remove(RemoveKind::Folder)
            | notify::EventKind::Modify(ModifyKind::Name(_))
    );
    // Writing to a file adds, removes and renames nothing.
    let write =
        matches!(ev.kind, notify::EventKind::Modify(k) if !matches!(k, ModifyKind::Name(_)));
    ev.paths.is_empty()
        || ev.paths.iter().any(|p| {
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                return true;
            };
            !is_partial_name(name)
                && (structural || is_media_name(name))
                && !(write && is_muted(p, muted))
        })
}

/// Whether `path` is one of the files in `muted`. Events name a file as the
/// watched folder is spelled, which need not be how the caller spelled it, so
/// a file of the same name is also compared by its real path.
fn is_muted(path: &Path, muted: &[PathBuf]) -> bool {
    muted.iter().any(|m| m == path)
        || (muted.iter().any(|m| m.file_name() == path.file_name())
            && muted.contains(&canonical_key(path)))
}

fn worker(shared: Arc<Shared>, events: Emit, wake: Receiver<()>, watch_wake: Sender<()>) {
    let watcher_flag = Arc::new(AtomicBool::new(false));
    let flag = watcher_flag.clone();
    let watch_shared = shared.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        // inotify reports our own directory listings as `Access(Open)`;
        // only real changes count, or every scan would trigger the next.
        let Ok(ev) = res else { return };
        let matters = {
            let muted = watch_shared.muted.lock().unwrap_or_else(|e| e.into_inner());
            watcher_event_matters(&ev, &muted)
        };
        if matters {
            flag.store(true, Ordering::Relaxed);
            let _ = watch_wake.send(());
        }
    })
    .ok();
    let mut watched: Vec<PathBuf> = Vec::new();
    let mut last_timer = Instant::now();
    let mut watch_due: Option<Instant> = None;
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        // Coalesce watcher events.
        if watcher_flag.swap(false, Ordering::Relaxed) {
            watch_due = Some(Instant::now() + WATCH_DEBOUNCE);
        }
        if let Some(due) = watch_due
            && Instant::now() >= due
        {
            watch_due = None;
            let mut pending = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
            if pending.is_none() {
                *pending = Some(ScanKind::Watcher);
            }
        }
        if last_timer.elapsed() >= TIMER_REFRESH {
            last_timer = Instant::now();
            let mut pending = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
            if pending.is_none() {
                *pending = Some(ScanKind::Timer);
            }
        }
        let kind = shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some(kind) = kind else {
            let timeout = watch_due
                .map(|d| d.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_millis(500));
            let _ = wake.recv_timeout(timeout.min(Duration::from_millis(500)));
            continue;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut c) = shared.cancel.lock() {
            *c = cancel.clone();
        }
        let generation = {
            let mut g = shared.generation.lock().unwrap_or_else(|e| e.into_inner());
            *g += 1;
            *g
        };
        let roots = shared
            .roots
            .lock()
            .map(|r| r.clone())
            .unwrap_or_else(|e| e.into_inner().clone());
        let started = Instant::now();
        let mut first = true;
        let result = scan_with_dirs(&roots, &cancel, |batch| {
            if first {
                first = false;
                events.send(ScanEvent::Batch {
                    generation,
                    records: batch,
                });
            }
        });
        if cancel.load(Ordering::Relaxed) {
            continue;
        }
        match result {
            Ok((records, dirs)) => {
                if let Some(w) = watcher.as_mut() {
                    for old in watched.iter().filter(|d| !dirs.contains(d)) {
                        let _ = w.unwatch(old);
                    }
                    for new in dirs.iter().filter(|d| !watched.contains(d)) {
                        let _ = w.watch(new, notify::RecursiveMode::NonRecursive);
                    }
                }
                watched = dirs.clone();
                events.send(ScanEvent::Done {
                    generation,
                    kind,
                    records,
                    watched: dirs,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                });
            }
            Err(e) => {
                events.send(ScanEvent::Failed {
                    kind,
                    error: e.to_string(),
                });
            }
        }
    }
}
