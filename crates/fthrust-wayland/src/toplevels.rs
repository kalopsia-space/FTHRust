//! The compositor's window list through `wlr-foreign-toplevel-management`
//! (title, app id, fullscreen, activated, minimized). Game detection and
//! focus pause read snapshots; a channel message follows every `done`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wayland_client::protocol::{wl_callback, wl_compositor, wl_output, wl_registry, wl_shm};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1, zwlr_foreign_toplevel_manager_v1,
};

use crate::connection::{ConnectError, CoreState, bounded_roundtrip, connect_display};
use crate::dispatch::{WaitResult, WaylandOps, wait_for_event};

/// One toplevel as last published by `done`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Toplevel {
    /// The handle's protocol id (stable while the window lives).
    pub id: u32,
    /// Window title.
    pub title: String,
    /// Application id (`steam_app_…`, `org.mozilla.firefox`).
    pub app_id: String,
    /// `state` contains `fullscreen`.
    pub fullscreen: bool,
    /// `state` contains `activated` (focused).
    pub activated: bool,
    /// `state` contains `minimized`.
    pub minimized: bool,
}

/// A handle event, decoupled from the protocol objects for tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToplevelEvent {
    /// `title`.
    Title {
        /// Handle id.
        id: u32,
        /// Title.
        title: String,
    },
    /// `app_id`.
    AppId {
        /// Handle id.
        id: u32,
        /// App id.
        app_id: String,
    },
    /// `state` (an array of native-endian `u32`s).
    State {
        /// Handle id.
        id: u32,
        /// Raw array.
        state: Vec<u8>,
    },
    /// `done`: publish the pending changes.
    Done {
        /// Handle id.
        id: u32,
    },
    /// `closed`.
    Closed {
        /// Handle id.
        id: u32,
    },
}

const STATE_MINIMIZED: u32 = 1;
const STATE_ACTIVATED: u32 = 2;
const STATE_FULLSCREEN: u32 = 3;

/// Applies handle events; publishes a toplevel only after `done`.
#[derive(Clone, Debug, Default)]
pub struct ToplevelTracker {
    pending: HashMap<u32, Toplevel>,
    published: HashMap<u32, Toplevel>,
}

impl ToplevelTracker {
    /// Empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one event.
    pub fn apply(&mut self, ev: ToplevelEvent) {
        match ev {
            ToplevelEvent::Title { id, title } => self.pending_mut(id).title = title,
            ToplevelEvent::AppId { id, app_id } => self.pending_mut(id).app_id = app_id,
            ToplevelEvent::State { id, state } => {
                if state.len() % 4 != 0 {
                    // A malformed array is ignored rather than misread.
                    return;
                }
                let values: Vec<u32> = state
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| u32::from_ne_bytes(*c))
                    .collect();
                let p = self.pending_mut(id);
                p.fullscreen = values.contains(&STATE_FULLSCREEN);
                p.activated = values.contains(&STATE_ACTIVATED);
                p.minimized = values.contains(&STATE_MINIMIZED);
            }
            ToplevelEvent::Done { id } => {
                if let Some(p) = self.pending.get(&id) {
                    self.published.insert(id, p.clone());
                }
            }
            ToplevelEvent::Closed { id } => {
                self.pending.remove(&id);
                self.published.remove(&id);
            }
        }
    }

    fn pending_mut(&mut self, id: u32) -> &mut Toplevel {
        self.pending.entry(id).or_insert_with(|| {
            let mut t = self.published.get(&id).cloned().unwrap_or_default();
            t.id = id;
            t
        })
    }

    /// Every published toplevel, by id.
    pub fn snapshot(&self) -> Vec<Toplevel> {
        let mut v: Vec<Toplevel> = self.published.values().cloned().collect();
        v.sort_by_key(|t| t.id);
        v
    }

    /// The activated toplevel, if any.
    pub fn active(&self) -> Option<&Toplevel> {
        let mut v: Vec<&Toplevel> = self.published.values().filter(|t| t.activated).collect();
        v.sort_by_key(|t| t.id);
        v.into_iter().next()
    }
}

struct WatcherState {
    core: CoreState,
    tracker: Arc<Mutex<ToplevelTracker>>,
    notify: Sender<()>,
}

impl AsMut<CoreState> for WatcherState {
    fn as_mut(&mut self) -> &mut CoreState {
        &mut self.core
    }
}

macro_rules! delegate_core {
    ($($iface:ty),* $(,)?) => {$(
        impl Dispatch<$iface, ()> for WatcherState {
            fn event(s: &mut Self, p: &$iface, e: <$iface as Proxy>::Event, d: &(), c: &Connection, q: &QueueHandle<Self>) {
                <CoreState as Dispatch<$iface, (), Self>>::event(s, p, e, d, c, q)
            }
        }
    )*};
}
delegate_core!(
    wl_registry::WlRegistry,
    wl_output::WlOutput,
    wl_shm::WlShm,
    wl_callback::WlCallback
);
delegate_noop!(WatcherState: ignore wl_compositor::WlCompositor);

impl Dispatch<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, ()> for WatcherState {
    fn event(
        _: &mut Self,
        _: &zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        _: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // `toplevel` handles are created by wayland-client with our queue;
        // `finished` ends the listing.
    }
}

impl Dispatch<zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ()> for WatcherState {
    fn event(
        state: &mut Self,
        handle: &zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;
        let id = handle.id().protocol_id();
        let ev = match event {
            Event::Title { title } => ToplevelEvent::Title { id, title },
            Event::AppId { app_id } => ToplevelEvent::AppId { id, app_id },
            Event::State { state } => ToplevelEvent::State { id, state },
            Event::Done => ToplevelEvent::Done { id },
            Event::Closed => {
                handle.destroy();
                ToplevelEvent::Closed { id }
            }
            _ => return,
        };
        let publish = matches!(
            ev,
            ToplevelEvent::Done { .. } | ToplevelEvent::Closed { .. }
        );
        if let Ok(mut t) = state.tracker.lock() {
            t.apply(ev);
        }
        if publish {
            let _ = state.notify.send(());
        }
    }
}

/// Why the watcher could not start.
#[derive(Debug, thiserror::Error)]
pub enum WatcherError {
    /// No Wayland display or the connection failed.
    #[error(transparent)]
    Connect(#[from] ConnectError),
    /// The compositor has no `zwlr_foreign_toplevel_manager_v1`.
    #[error("the compositor does not expose zwlr_foreign_toplevel_manager_v1")]
    Unavailable,
}

/// A background connection that keeps a [`ToplevelTracker`] current.
pub struct ToplevelWatcher {
    tracker: Arc<Mutex<ToplevelTracker>>,
    changes: Receiver<()>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ToplevelWatcher {
    /// Connects, binds the manager (version ≤ 3) and starts dispatching on
    /// a thread named `fthrust-toplevels`.
    pub fn connect() -> Result<ToplevelWatcher, WatcherError> {
        let conn = connect_display()?;
        let queue = conn.new_event_queue::<WatcherState>();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let tracker = Arc::new(Mutex::new(ToplevelTracker::new()));
        let (tx, rx) = channel();
        let mut ops = WaylandOps::new(
            conn,
            queue,
            WatcherState {
                core: CoreState::default(),
                tracker: tracker.clone(),
                notify: tx,
            },
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let keep = || true;
        match bounded_roundtrip(&mut ops, deadline, &keep) {
            WaitResult::EventReceived => {}
            _ => return Err(ConnectError::Roundtrip("timeout").into()),
        }
        let Some(global) = ops.state().core.global("zwlr_foreign_toplevel_manager_v1") else {
            return Err(WatcherError::Unavailable);
        };
        let _manager = registry
            .bind::<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, _, WatcherState>(
                global.name,
                global.version.min(3),
                &qh,
                (),
            );
        // One more roundtrip so the initial listing is published before we return.
        let _ = bounded_roundtrip(&mut ops, deadline, &keep);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = stop.clone();
        let thread = std::thread::Builder::new()
            .name("fthrust-toplevels".into())
            .spawn(move || {
                let keep = || !stop_t.load(Ordering::Relaxed);
                while keep() {
                    match wait_for_event(
                        &mut ops,
                        Instant::now() + Duration::from_millis(250),
                        &keep,
                    ) {
                        WaitResult::Disconnected | WaitResult::Error => break,
                        _ => {}
                    }
                }
            })
            .expect("spawn toplevel watcher");
        Ok(ToplevelWatcher {
            tracker,
            changes: rx,
            stop,
            thread: Some(thread),
        })
    }

    /// Whether the protocol is present (`connect` succeeded).
    pub fn available(&self) -> bool {
        self.thread.is_some()
    }

    /// The current window list.
    pub fn snapshot(&self) -> Vec<Toplevel> {
        self.tracker
            .lock()
            .map(|t| t.snapshot())
            .unwrap_or_default()
    }

    /// The activated window.
    pub fn active(&self) -> Option<Toplevel> {
        self.tracker.lock().ok().and_then(|t| t.active().cloned())
    }

    /// One message per published change.
    pub fn changes(&self) -> &Receiver<()> {
        &self.changes
    }

    /// Stops the thread.
    pub fn close(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ToplevelWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
