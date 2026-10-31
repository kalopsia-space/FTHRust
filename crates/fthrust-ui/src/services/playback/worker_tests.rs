//! The video worker driven by sources of the tests' own: one that holds the
//! worker inside a seek, and counters on what the worker does while idle.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;

/// What a test needs to know about, and to hold, a seek in progress.
#[derive(Clone, Default)]
struct Gate {
    /// The worker is inside `seek`.
    entered: Arc<AtomicBool>,
    /// `seek` returns once this is set.
    release: Arc<AtomicBool>,
}

/// Numbered frames 33 ms apart.
struct Numbered {
    next: u64,
    gate: Option<Gate>,
    /// Set when the worker has ended and dropped its source.
    gone: Arc<AtomicBool>,
}

impl Numbered {
    fn new() -> Numbered {
        Numbered {
            next: 0,
            gate: None,
            gone: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl FrameSource for Numbered {
    fn seek(&mut self, ms: u64) {
        if let Some(g) = &self.gate {
            g.entered.store(true, Ordering::SeqCst);
            while !g.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        self.next = ms / 33;
    }

    fn next_frame(&mut self) -> Result<Option<Frame>, String> {
        let pts_ms = self.next * 33;
        self.next += 1;
        Ok(Some(Frame {
            pts_ms,
            width: 2,
            height: 2,
            rgba: vec![0; 16],
        }))
    }

    fn reopen(&self, _: &Path, _: u64) -> Option<Box<dyn FrameSource>> {
        None
    }
}

impl Drop for Numbered {
    fn drop(&mut self) {
        self.gone.store(true, Ordering::SeqCst);
    }
}

fn queued(w: &VideoWorker) -> usize {
    w.shared.frames.lock().unwrap().len()
}

fn passes(w: &VideoWorker) -> u64 {
    w.shared.passes.load(Ordering::Relaxed)
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_idle_worker_sleeps_instead_of_polling() {
    let w = VideoWorker::start(Box::new(Numbered::new()));
    // The first frame (the picture shown before anything plays), then nothing
    // to do: it used to look again every 4 ms, 250 wake-ups a second for as
    // long as the editor was open.
    wait_until("the first frame", || queued(&w) == 1);
    std::thread::sleep(Duration::from_millis(100));
    let before = passes(&w);
    std::thread::sleep(Duration::from_millis(500));
    let idle = passes(&w) - before;
    assert!(idle <= 1, "{idle} passes in half a second of doing nothing");
}

#[test]
fn a_command_wakes_an_idle_worker_at_once() {
    let w = VideoWorker::start(Box::new(Numbered::new()));
    wait_until("the first frame", || queued(&w) == 1);
    std::thread::sleep(Duration::from_millis(50));
    w.send(VideoCmd::Seek(990));
    wait_until("the seek", || w.shared.seeks.load(Ordering::Relaxed) == 1);
    wait_until("the frame at the seek", || {
        w.shared
            .frames
            .lock()
            .unwrap()
            .front()
            .is_some_and(|f| f.pts_ms == 990)
    });
}

#[test]
fn a_full_queue_waits_for_the_service_to_take_a_frame() {
    let w = VideoWorker::start(Box::new(Numbered::new()));
    w.send(VideoCmd::Play);
    wait_until("the queue to fill", || queued(&w) == VIDEO_AHEAD);
    std::thread::sleep(Duration::from_millis(100));
    let before = passes(&w);
    std::thread::sleep(Duration::from_millis(300));
    let waiting = passes(&w) - before;
    assert!(waiting <= 1, "{waiting} passes with nothing to decode");
    // The service shows one and wakes the worker, which reads one more.
    w.shared.frames.lock().unwrap().pop_front();
    w.send(VideoCmd::Wake);
    wait_until("the queue to fill again", || queued(&w) == VIDEO_AHEAD);
}

#[test]
fn dropping_the_worker_does_not_wait_for_a_seek_in_progress() {
    let gate = Gate::default();
    let mut source = Numbered::new();
    source.gate = Some(gate.clone());
    let gone = source.gone.clone();
    let w = VideoWorker::start(Box::new(source));
    wait_until("the first frame", || queued(&w) == 1);
    // A seek on a long clip decodes a whole GOP: here it takes 1.5 s.
    w.send(VideoCmd::Seek(5_000));
    wait_until("the seek to start", || gate.entered.load(Ordering::SeqCst));
    let release = gate.release.clone();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        release.store(true, Ordering::SeqCst);
    });
    let started = Instant::now();
    drop(w);
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_millis(500),
        "closing the editor waited {waited:?} for the worker"
    );
    releaser.join().unwrap();
    wait_until("the worker to end once the seek finished", || {
        gone.load(Ordering::SeqCst)
    });
}
