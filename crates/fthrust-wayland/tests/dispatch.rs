use std::collections::VecDeque;
use std::time::{Duration, Instant};

use fthrust_wayland::dispatch::{
    DispatchError, PollEvents, WaitOps, WaitResult, dispatch_until, wait_for_event,
};

#[derive(Default)]
struct Fake {
    pending: VecDeque<usize>,
    polls: VecDeque<Result<PollEvents, DispatchError>>,
    flush: VecDeque<Result<(), DispatchError>>,
    reads: usize,
    cancels: usize,
    prepared: usize,
    poll_calls: Vec<(bool, Duration)>,
    error: Option<String>,
}

impl WaitOps for Fake {
    fn dispatch_pending(&mut self) -> Result<usize, DispatchError> {
        Ok(self.pending.pop_front().unwrap_or(0))
    }
    fn prepare_read(&mut self) -> bool {
        self.prepared += 1;
        true
    }
    fn cancel_read(&mut self) {
        self.cancels += 1;
    }
    fn read_events(&mut self) -> Result<(), DispatchError> {
        self.reads += 1;
        Ok(())
    }
    fn flush(&mut self) -> Result<(), DispatchError> {
        self.flush.pop_front().unwrap_or(Ok(()))
    }
    fn poll(&mut self, want_write: bool, timeout: Duration) -> Result<PollEvents, DispatchError> {
        self.poll_calls.push((want_write, timeout));
        match self.polls.pop_front() {
            Some(r) => r,
            None => {
                std::thread::sleep(timeout);
                Ok(PollEvents::default())
            }
        }
    }
    fn protocol_error(&self) -> Option<String> {
        self.error.clone()
    }
}

fn readable() -> Result<PollEvents, DispatchError> {
    Ok(PollEvents {
        readable: true,
        writable: false,
        hangup: false,
    })
}

#[test]
fn event_before_deadline() {
    let mut f = Fake {
        pending: VecDeque::from([0, 1]),
        polls: VecDeque::from([readable()]),
        ..Fake::default()
    };
    let r = wait_for_event(&mut f, Instant::now() + Duration::from_secs(1), &|| true);
    assert_eq!(r, WaitResult::EventReceived);
    assert_eq!((f.reads, f.cancels, f.prepared), (1, 0, 1));
}

#[test]
fn no_event_is_timeout_without_busy_spin() {
    let mut f = Fake::default();
    let started = Instant::now();
    let r = wait_for_event(&mut f, started + Duration::from_millis(120), &|| true);
    assert_eq!(r, WaitResult::Timeout);
    assert!(started.elapsed() >= Duration::from_millis(110));
    assert!(f.poll_calls.len() <= 4, "{:?}", f.poll_calls);
    assert!(
        f.poll_calls
            .iter()
            .all(|(_, t)| *t <= Duration::from_millis(50))
    );
    assert_eq!(f.cancels, f.prepared, "every prepared read was cancelled");
    assert_eq!(f.reads, 0);
}

#[test]
fn cancellation_cancels_prepared_read() {
    let mut f = Fake {
        polls: VecDeque::from([Ok(PollEvents::default())]),
        ..Fake::default()
    };
    let calls = std::cell::Cell::new(0);
    let keep = || {
        calls.set(calls.get() + 1);
        calls.get() <= 1
    };
    let r = wait_for_event(&mut f, Instant::now() + Duration::from_secs(5), &keep);
    assert_eq!(r, WaitResult::Cancelled);
    assert_eq!((f.cancels, f.reads), (1, 0));
}

#[test]
fn hangup_is_disconnected() {
    let mut f = Fake {
        polls: VecDeque::from([Ok(PollEvents {
            readable: false,
            writable: false,
            hangup: true,
        })]),
        ..Fake::default()
    };
    let r = wait_for_event(&mut f, Instant::now() + Duration::from_secs(1), &|| true);
    assert_eq!(r, WaitResult::Disconnected);
    assert_eq!(f.cancels, 1);
    let mut f = Fake {
        polls: VecDeque::from([Err(DispatchError::Disconnected)]),
        ..Fake::default()
    };
    assert_eq!(
        wait_for_event(&mut f, Instant::now() + Duration::from_secs(1), &|| true),
        WaitResult::Disconnected
    );
}

#[test]
fn pending_events_dispatched_before_prepare_read() {
    let mut f = Fake {
        pending: VecDeque::from([2]),
        ..Fake::default()
    };
    let r = wait_for_event(&mut f, Instant::now() + Duration::from_secs(1), &|| true);
    assert_eq!(r, WaitResult::EventReceived);
    assert!(f.poll_calls.is_empty());
    assert_eq!(f.prepared, 0);
}

#[test]
fn flush_would_block_polls_for_writable() {
    let mut f = Fake {
        pending: VecDeque::from([0, 1]),
        flush: VecDeque::from([Err(DispatchError::WouldBlock)]),
        polls: VecDeque::from([readable()]),
        ..Fake::default()
    };
    let r = wait_for_event(&mut f, Instant::now() + Duration::from_secs(1), &|| true);
    assert_eq!(r, WaitResult::EventReceived);
    assert!(f.poll_calls[0].0, "polled for writable");
}

#[test]
fn protocol_error_is_error() {
    let mut f = Fake {
        pending: VecDeque::from([0, 1]),
        polls: VecDeque::from([readable()]),
        error: Some("bad object".into()),
        ..Fake::default()
    };
    assert_eq!(
        wait_for_event(&mut f, Instant::now() + Duration::from_secs(1), &|| true),
        WaitResult::Error
    );
}

#[test]
fn dispatch_until_completes_on_predicate() {
    let mut f = Fake {
        pending: VecDeque::from([0, 1, 0, 1]),
        polls: VecDeque::from([readable(), readable()]),
        ..Fake::default()
    };
    let mut events = 0;
    let r = dispatch_until(
        &mut f,
        Instant::now() + Duration::from_secs(1),
        &|| true,
        &mut || {
            events += 1;
            events >= 3
        },
    );
    assert_eq!(r, WaitResult::EventReceived);
    assert_eq!(f.reads, 2);
    let mut f = Fake::default();
    let r = dispatch_until(
        &mut f,
        Instant::now() + Duration::from_millis(60),
        &|| true,
        &mut || false,
    );
    assert_eq!(r, WaitResult::Timeout);
}
