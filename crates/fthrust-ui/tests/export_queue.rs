//! The export queue (`core/export_queue.py`): FIFO admission under the
//! parallel limit, record pruning, cancel and shutdown.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fthrust_core::FthrPaths;
use fthrust_core::export::graph::{AudioOut, GraphPlan, MapRef, VideoOut};
use fthrust_core::settings::SettingsStore;
use fthrust_media::export::{ExportResult, ExportState, Job, Quality};
use fthrust_ui::services::export::{ExportQueue, Runner, TaskView};
use serde_json::json;

fn plan() -> GraphPlan {
    GraphPlan {
        seek_s: None,
        duration_s: None,
        filter_graph: None,
        video_map: MapRef::FirstVideo,
        audio_map: None,
        video: VideoOut::Copy,
        audio: AudioOut::None,
        audio_filter: None,
        shortest: false,
    }
}

fn builder() -> Box<dyn FnOnce(&Path) -> Job + Send> {
    Box::new(|output: &Path| Job {
        source: PathBuf::from("/clips/a.mp4"),
        output: output.to_path_buf(),
        plan: plan(),
        quality: Quality::Quality,
        video_bitrate_kbps: None,
        max_rate_kbps: None,
        expected_duration_s: 1.0,
        target_size_mb: None,
        output_fps: None,
        decode_from_s: 0.0,
        inactivity_s: None,
        fail_first_encoder: false,
    })
}

fn settings(tmp: &Path, parallel: bool, limit: i64) -> SettingsStore {
    let mut s = SettingsStore::load(FthrPaths::new(tmp)).unwrap();
    s.set("parallel_exports", json!(parallel));
    s.set("parallel_export_limit", json!(limit));
    s
}

/// A runner that blocks until `release` is set (or cancel), counting how
/// many run at once.
struct Gate {
    release: AtomicBool,
    running: AtomicUsize,
    peak: AtomicUsize,
    order: Mutex<Vec<PathBuf>>,
}

fn gated() -> (Arc<Gate>, Runner) {
    let gate = Arc::new(Gate {
        release: AtomicBool::new(false),
        running: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        order: Mutex::new(Vec::new()),
    });
    let g = gate.clone();
    let runner: Runner = Arc::new(move |job, cancel, status, progress| {
        g.order.lock().unwrap().push(job.output.clone());
        let now = g.running.fetch_add(1, Ordering::SeqCst) + 1;
        g.peak.fetch_max(now, Ordering::SeqCst);
        status("Exporting · software");
        progress(42.0);
        while !g.release.load(Ordering::SeqCst) && !cancel.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        g.running.fetch_sub(1, Ordering::SeqCst);
        if cancel.load(Ordering::SeqCst) {
            ExportResult {
                state: ExportState::Cancelled,
                detail: "Export cancelled".into(),
            }
        } else {
            ExportResult {
                state: ExportState::Completed,
                detail: String::new(),
            }
        }
    });
    (gate, runner)
}

fn wait_for(q: &ExportQueue, f: impl Fn(&[TaskView]) -> bool) -> Vec<TaskView> {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let t = q.tasks();
        if f(&t) || Instant::now() > end {
            return t;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn fifo_one_at_a_time() {
    let tmp = tempfile::tempdir().unwrap();
    let (gate, runner) = gated();
    let q = ExportQueue::with_runner(runner);
    q.configure(&settings(tmp.path(), false, 3));
    let ids: Vec<_> = (0..3)
        .map(|_| {
            q.submit(
                PathBuf::from("/clips/a.mp4"),
                tmp.path(),
                Quality::Quality,
                builder(),
                1.0,
            )
        })
        .collect();
    let t = wait_for(&q, |t| {
        t.iter()
            .any(|t| t.status == "Exporting · software" && t.progress == 42.0)
    });
    assert_eq!(
        t.iter().filter(|t| t.status == "Queued").count(),
        2,
        "{t:?}"
    );
    assert_eq!(t[0].id, ids[0]);
    assert_eq!(t[0].status, "Exporting · software");
    assert_eq!(t[0].progress, 42.0);
    assert!(t[0].output.to_string_lossy().contains("a_export_"));
    gate.release.store(true, Ordering::SeqCst);
    let t = wait_for(&q, |t| t.iter().all(|t| t.done));
    assert!(
        t.iter()
            .all(|t| t.status == "Completed" && t.progress == 100.0),
        "{t:?}"
    );
    assert_eq!(gate.peak.load(Ordering::SeqCst), 1);
    let order = gate.order.lock().unwrap().clone();
    let outputs: Vec<_> = t.iter().map(|t| t.output.clone()).collect();
    assert_eq!(order, outputs, "FIFO admission");
}

#[test]
fn parallel_limit_clamped() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(ExportQueue::limit_for(&settings(tmp.path(), true, 9)), 4);
    assert_eq!(ExportQueue::limit_for(&settings(tmp.path(), true, 1)), 2);
    assert_eq!(ExportQueue::limit_for(&settings(tmp.path(), true, 3)), 3);
    assert_eq!(ExportQueue::limit_for(&settings(tmp.path(), false, 4)), 1);
    let (gate, runner) = gated();
    let q = ExportQueue::with_runner(runner);
    q.configure(&settings(tmp.path(), true, 9));
    for _ in 0..6 {
        q.submit(
            PathBuf::from("/clips/a.mp4"),
            tmp.path(),
            Quality::Fast,
            builder(),
            1.0,
        );
    }
    wait_for(&q, |t| {
        t.iter()
            .filter(|t| t.status == "Exporting · software")
            .count()
            == 4
    });
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(gate.peak.load(Ordering::SeqCst), 4);
    gate.release.store(true, Ordering::SeqCst);
    wait_for(&q, |t| t.iter().all(|t| t.done));
}

#[test]
fn records_pruned_to_newest_50_plus_running() {
    let tmp = tempfile::tempdir().unwrap();
    let (gate, runner) = gated();
    let q = ExportQueue::with_runner(runner);
    // The first job stays running; the next 54 finish immediately.
    let first = q.submit(
        PathBuf::from("/clips/a.mp4"),
        tmp.path(),
        Quality::Quality,
        builder(),
        1.0,
    );
    wait_for(&q, |t| t[0].status == "Exporting · software");
    let fast: Runner = Arc::new(|_, _, _, _| ExportResult {
        state: ExportState::Completed,
        detail: String::new(),
    });
    q.set_runner(fast);
    q.configure(&settings(tmp.path(), true, 4));
    for _ in 0..54 {
        q.submit(
            PathBuf::from("/clips/b.mp4"),
            tmp.path(),
            Quality::Quality,
            builder(),
            1.0,
        );
        wait_for(&q, |t| t.iter().filter(|t| !t.done).count() <= 1);
    }
    let t = q.tasks();
    // Before the last submit 54 records existed; the newest 50 plus the
    // running first one survive, then the new one is appended.
    assert_eq!(t.len(), 52, "{}", t.len());
    assert_eq!(t[0].id, first);
    gate.release.store(true, Ordering::SeqCst);
}

#[test]
fn cancel_marks_cancelling_then_cancelled() {
    let tmp = tempfile::tempdir().unwrap();
    let (_gate, runner) = gated();
    let q = ExportQueue::with_runner(runner);
    let id = q.submit(
        PathBuf::from("/clips/a.mp4"),
        tmp.path(),
        Quality::Quality,
        builder(),
        1.0,
    );
    wait_for(&q, |t| t[0].status == "Exporting · software");
    assert!(q.active_for(Path::new("/clips/a.mp4")));
    q.cancel(id);
    let status = q.tasks()[0].status.clone();
    assert!(status == "Cancelling" || status == "Cancelled", "{status}");
    let t = wait_for(&q, |t| t[0].done);
    assert_eq!(t[0].status, "Cancelled");
    assert_eq!(t[0].detail, "Export cancelled");
    assert!(!q.active_for(Path::new("/clips/a.mp4")));
}

#[test]
fn shutdown_cancels_within_bound() {
    let tmp = tempfile::tempdir().unwrap();
    let (_gate, runner) = gated();
    let q = ExportQueue::with_runner(runner);
    q.configure(&settings(tmp.path(), true, 2));
    for _ in 0..3 {
        q.submit(
            PathBuf::from("/clips/a.mp4"),
            tmp.path(),
            Quality::Quality,
            builder(),
            1.0,
        );
    }
    wait_for(&q, |t| {
        t.iter()
            .filter(|t| t.status == "Exporting · software")
            .count()
            == 2
    });
    let started = Instant::now();
    q.shutdown(Duration::from_secs(12));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(q.tasks().iter().all(|t| t.done), "{:?}", q.tasks());
}

#[test]
fn counts_match_reference_label_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let results = Arc::new(Mutex::new(vec![
        ExportState::Completed,
        ExportState::Failed,
        ExportState::TimedOut,
        ExportState::Cancelled,
    ]));
    let r = results.clone();
    let runner: Runner = Arc::new(move |_, _, _, _| {
        let state = r.lock().unwrap().remove(0);
        ExportResult {
            state,
            detail: if state == ExportState::Completed {
                String::new()
            } else {
                "boom".into()
            },
        }
    });
    let q = ExportQueue::with_runner(runner);
    assert_eq!(q.counts(), (0, 0, 0));
    for _ in 0..4 {
        q.submit(
            PathBuf::from("/clips/a.mp4"),
            tmp.path(),
            Quality::Quality,
            builder(),
            1.0,
        );
    }
    let t = wait_for(&q, |t| t.iter().all(|t| t.done));
    let statuses: Vec<_> = t.iter().map(|t| t.status.as_str()).collect();
    assert_eq!(statuses, ["Completed", "Failed", "Timed_Out", "Cancelled"]);
    assert_eq!(t[1].detail, "boom");
    assert_eq!(t[0].detail, "");
    // pending, failed (Failed + Timed_Out), completed; Cancelled not counted.
    assert_eq!(q.counts(), (0, 2, 1));
    assert_eq!(ExportQueue::label_suffix(q.counts()), " · 2 failed");
    assert_eq!(ExportQueue::label_suffix((3, 2, 1)), " · 3 pending");
    assert_eq!(ExportQueue::label_suffix((0, 0, 5)), " · 5 completed");
    assert_eq!(ExportQueue::label_suffix((0, 0, 0)), "");
}

#[test]
fn lossless_output_is_mkv() {
    let tmp = tempfile::tempdir().unwrap();
    let (gate, runner) = gated();
    let q = ExportQueue::with_runner(runner);
    q.submit(
        PathBuf::from("/clips/my clip.mp4"),
        tmp.path(),
        Quality::Lossless,
        builder(),
        1.0,
    );
    let t = q.tasks();
    let name = t[0]
        .output
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with("my clip_export_") && name.ends_with(".mkv"),
        "{name}"
    );
    assert_eq!(t[0].output.parent(), Some(tmp.path()));
    gate.release.store(true, Ordering::SeqCst);
    wait_for(&q, |t| t[0].done);
}
