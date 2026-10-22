//! Bounded, cancellable Wayland event dispatch (ports `wayland_dispatch.cpp`).
//!
//! Every wait is sliced into polls of at most 50 ms so cancellation and
//! deadlines are honoured promptly, and every prepared read is paired with
//! exactly one `read_events` or `cancel_read`.

use std::time::{Duration, Instant};

use wayland_client::backend::{ReadEventsGuard, WaylandError};
use wayland_client::{Connection, EventQueue};

/// Longest single poll.
pub const POLL_SLICE: Duration = Duration::from_millis(50);

/// How a bounded wait ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitResult {
    /// At least one event was dispatched.
    EventReceived,
    /// The deadline passed with nothing to dispatch.
    Timeout,
    /// `keep_running` returned false.
    Cancelled,
    /// The compositor connection is gone.
    Disconnected,
    /// A protocol or I/O error.
    Error,
}

impl WaitResult {
    /// The reference's name for the result.
    pub fn name(self) -> &'static str {
        match self {
            WaitResult::EventReceived => "event",
            WaitResult::Timeout => "timeout",
            WaitResult::Cancelled => "cancelled",
            WaitResult::Disconnected => "disconnected",
            WaitResult::Error => "error",
        }
    }
}

/// Why a dispatch primitive failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchError {
    /// The socket would block (flush: poll for writable first).
    WouldBlock,
    /// The connection is gone.
    Disconnected,
    /// Anything else.
    Other(String),
}

/// What `poll` observed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PollEvents {
    /// Events can be read.
    pub readable: bool,
    /// Pending writes can be flushed.
    pub writable: bool,
    /// The peer hung up or the descriptor errored.
    pub hangup: bool,
}

/// The libwayland/poll seam. Production uses [`WaylandOps`]; tests use a
/// scripted fake.
pub trait WaitOps {
    /// Dispatches already-read events; returns how many.
    fn dispatch_pending(&mut self) -> Result<usize, DispatchError>;
    /// Prepares to read; `false` means events are pending and must be
    /// dispatched first.
    fn prepare_read(&mut self) -> bool;
    /// Abandons a prepared read.
    fn cancel_read(&mut self);
    /// Reads events after a readable poll (consumes the prepared read).
    fn read_events(&mut self) -> Result<(), DispatchError>;
    /// Flushes pending requests.
    fn flush(&mut self) -> Result<(), DispatchError>;
    /// Waits up to `timeout` for the connection to become readable (and
    /// writable when `want_write`).
    fn poll(&mut self, want_write: bool, timeout: Duration) -> Result<PollEvents, DispatchError>;
    /// A fatal protocol error the compositor sent, if any.
    fn protocol_error(&self) -> Option<String>;
}

fn map_err(e: DispatchError) -> WaitResult {
    match e {
        DispatchError::Disconnected => WaitResult::Disconnected,
        _ => WaitResult::Error,
    }
}

/// Waits for and dispatches at least one event, or reports why not.
pub fn wait_for_event(
    ops: &mut dyn WaitOps,
    deadline: Instant,
    keep_running: &dyn Fn() -> bool,
) -> WaitResult {
    loop {
        match ops.dispatch_pending() {
            Ok(n) if n > 0 => return WaitResult::EventReceived,
            Ok(_) => {}
            Err(e) => return map_err(e),
        }
        if let Some(message) = ops.protocol_error() {
            tracing::warn!("wayland protocol error: {message}");
            return WaitResult::Error;
        }
        if !keep_running() {
            return WaitResult::Cancelled;
        }
        let now = Instant::now();
        if now >= deadline {
            return WaitResult::Timeout;
        }
        if !ops.prepare_read() {
            continue;
        }
        let want_write = match ops.flush() {
            Ok(()) => false,
            Err(DispatchError::WouldBlock) => true,
            Err(e) => {
                ops.cancel_read();
                return map_err(e);
            }
        };
        let slice = deadline.saturating_duration_since(now).min(POLL_SLICE);
        let events = match ops.poll(want_write, slice) {
            Ok(events) => events,
            Err(e) => {
                ops.cancel_read();
                return map_err(e);
            }
        };
        if events.hangup {
            ops.cancel_read();
            return WaitResult::Disconnected;
        }
        if !events.readable {
            ops.cancel_read();
            if !keep_running() {
                return WaitResult::Cancelled;
            }
            if Instant::now() >= deadline {
                return WaitResult::Timeout;
            }
            continue;
        }
        if let Err(e) = ops.read_events() {
            return map_err(e);
        }
    }
}

/// Dispatches until `complete` returns true, or the deadline, cancellation
/// or an error ends the wait.
pub fn dispatch_until(
    ops: &mut dyn WaitOps,
    deadline: Instant,
    keep_running: &dyn Fn() -> bool,
    complete: &mut dyn FnMut() -> bool,
) -> WaitResult {
    loop {
        if complete() {
            return WaitResult::EventReceived;
        }
        match wait_for_event(ops, deadline, keep_running) {
            WaitResult::EventReceived => {}
            other => return other,
        }
    }
}

/// The production [`WaitOps`] over a `wayland-client` connection and one
/// event queue whose state is `S`.
pub struct WaylandOps<S> {
    conn: Connection,
    queue: EventQueue<S>,
    state: S,
    guard: Option<ReadEventsGuard>,
}

impl<S> WaylandOps<S> {
    /// Wraps a connection, its queue and the queue's state.
    pub fn new(conn: Connection, queue: EventQueue<S>, state: S) -> Self {
        Self {
            conn,
            queue,
            state,
            guard: None,
        }
    }

    /// The queue state.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// The queue state, mutably.
    pub fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }

    /// The connection.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// The event queue.
    pub fn queue(&self) -> &EventQueue<S> {
        &self.queue
    }

    /// Consumes the wrapper, returning the queue state.
    pub fn into_state(self) -> S {
        self.state
    }
}

fn wayland_error(e: WaylandError) -> DispatchError {
    match e {
        WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock => {
            DispatchError::WouldBlock
        }
        WaylandError::Io(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::NotConnected
            ) =>
        {
            DispatchError::Disconnected
        }
        WaylandError::Io(io) => DispatchError::Other(io.to_string()),
        WaylandError::Protocol(p) => DispatchError::Other(p.message),
    }
}

impl<S> WaitOps for WaylandOps<S> {
    fn dispatch_pending(&mut self) -> Result<usize, DispatchError> {
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| match e {
                wayland_client::DispatchError::Backend(b) => wayland_error(b),
                other => DispatchError::Other(other.to_string()),
            })
    }

    fn prepare_read(&mut self) -> bool {
        match self.queue.prepare_read() {
            Some(guard) => {
                self.guard = Some(guard);
                true
            }
            None => false,
        }
    }

    fn cancel_read(&mut self) {
        self.guard = None;
    }

    fn read_events(&mut self) -> Result<(), DispatchError> {
        match self.guard.take() {
            Some(guard) => guard.read().map(|_| ()).map_err(wayland_error),
            None => Ok(()),
        }
    }

    fn flush(&mut self) -> Result<(), DispatchError> {
        self.conn.flush().map_err(wayland_error)
    }

    fn poll(&mut self, want_write: bool, timeout: Duration) -> Result<PollEvents, DispatchError> {
        use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
        let Some(guard) = self.guard.as_ref() else {
            return Err(DispatchError::Other("poll without a prepared read".into()));
        };
        let fd = guard.connection_fd();
        let mut flags = PollFlags::POLLIN;
        if want_write {
            flags |= PollFlags::POLLOUT;
        }
        let mut fds = [PollFd::new(fd, flags)];
        let timeout = PollTimeout::try_from(timeout.as_millis().min(i32::MAX as u128) as i32)
            .unwrap_or(PollTimeout::MAX);
        match poll(&mut fds, timeout) {
            Ok(_) => {
                let revents = fds[0].revents().unwrap_or(PollFlags::empty());
                Ok(PollEvents {
                    readable: revents.contains(PollFlags::POLLIN),
                    writable: revents.contains(PollFlags::POLLOUT),
                    hangup: revents
                        .intersects(PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL),
                })
            }
            Err(nix::errno::Errno::EINTR) => Ok(PollEvents::default()),
            Err(e) => Err(DispatchError::Other(e.to_string())),
        }
    }

    fn protocol_error(&self) -> Option<String> {
        self.conn.protocol_error().map(|p| p.message)
    }
}
