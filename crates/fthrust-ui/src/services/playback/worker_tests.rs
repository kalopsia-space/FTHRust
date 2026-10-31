//! The video worker driven by a source of the tests' own, counting what the
//! worker does while it has nothing to do.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::*;

/// Numbered frames 33 ms apart.
struct Numbered {
    next: u64,
}

impl FrameSource for Numbered {
    fn seek(&mut self, ms: u64) {
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
    let w = VideoWorker::start(Box::new(Numbered { next: 0 }));
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
    let w = VideoWorker::start(Box::new(Numbered { next: 0 }));
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
    let w = VideoWorker::start(Box::new(Numbered { next: 0 }));
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
