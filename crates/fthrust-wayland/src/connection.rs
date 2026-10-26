//! Connecting to the compositor, the registry, and the shared core state
//! that every backend queue embeds (outputs, shm formats, sync callbacks).

use std::time::Instant;

use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1;

use crate::dispatch::{WaitResult, WaylandOps, wait_for_event};
use crate::outputs::OutputTracker;

/// Why the compositor could not be reached.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// `WAYLAND_DISPLAY` is not set.
    #[error("WAYLAND_DISPLAY is not set; this is not a Wayland session")]
    NoDisplay,
    /// The socket could not be opened.
    #[error("could not connect to the Wayland display: {0}")]
    Connect(#[from] wayland_client::ConnectError),
    /// The registry roundtrip did not complete.
    #[error("the compositor did not answer the registry roundtrip ({0})")]
    Roundtrip(&'static str),
}

/// One advertised global.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Global {
    /// Registry name.
    pub name: u32,
    /// Interface.
    pub interface: String,
    /// Advertised version.
    pub version: u32,
}

/// State shared by every queue: globals, outputs, shm formats, syncs.
#[derive(Default)]
pub struct CoreState {
    /// Every global the registry announced (and not removed).
    pub globals: Vec<Global>,
    /// Bound outputs.
    pub outputs: OutputTracker,
    /// `wl_shm` formats the compositor supports.
    pub shm_formats: Vec<u32>,
    /// The bound `wl_shm`, once announced.
    pub shm: Option<wl_shm::WlShm>,
    /// The bound `wl_compositor`, once announced.
    pub compositor: Option<wl_compositor::WlCompositor>,
    /// Count of completed `wl_display.sync` callbacks.
    pub syncs_done: u64,
}

impl AsMut<CoreState> for CoreState {
    fn as_mut(&mut self) -> &mut CoreState {
        self
    }
}

impl AsRef<CoreState> for CoreState {
    fn as_ref(&self) -> &CoreState {
        self
    }
}

impl CoreState {
    /// Finds a global by interface.
    pub fn global(&self, interface: &str) -> Option<&Global> {
        self.globals.iter().find(|g| g.interface == interface)
    }
}

impl<S> Dispatch<wl_registry::WlRegistry, (), S> for CoreState
where
    S: Dispatch<wl_registry::WlRegistry, ()>
        + Dispatch<wl_output::WlOutput, ()>
        + Dispatch<wl_shm::WlShm, ()>
        + Dispatch<wl_compositor::WlCompositor, ()>
        + AsMut<CoreState>
        + 'static,
{
    fn event(
        state: &mut S,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<S>,
    ) {
        let core = state.as_mut();
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                match interface.as_str() {
                    "wl_output" => {
                        let output = registry.bind::<wl_output::WlOutput, _, S>(
                            name,
                            version.min(4),
                            qh,
                            (),
                        );
                        core.outputs.add(name, output);
                    }
                    "wl_shm" if core.shm.is_none() => {
                        core.shm = Some(registry.bind::<wl_shm::WlShm, _, S>(name, 1, qh, ()));
                    }
                    "wl_compositor" if core.compositor.is_none() => {
                        core.compositor = Some(registry.bind::<wl_compositor::WlCompositor, _, S>(
                            name,
                            version.min(4),
                            qh,
                            (),
                        ));
                    }
                    _ => {}
                }
                core.globals.push(Global {
                    name,
                    interface,
                    version,
                });
            }
            wl_registry::Event::GlobalRemove { name } => {
                core.globals.retain(|g| g.name != name);
                core.outputs.remove(name);
            }
            _ => {}
        }
    }
}

impl<S> Dispatch<wl_output::WlOutput, (), S> for CoreState
where
    S: Dispatch<wl_output::WlOutput, ()> + AsMut<CoreState>,
{
    fn event(
        state: &mut S,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<S>,
    ) {
        let core = state.as_mut();
        let Some(info) = core.outputs.info_mut(output) else {
            return;
        };
        match event {
            wl_output::Event::Geometry {
                transform: WEnum::Value(t),
                ..
            } => {
                info.transform =
                    crate::frame::OutputTransform::from_wl(t as u32).unwrap_or_default();
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                refresh,
            } => {
                let current =
                    matches!(flags, WEnum::Value(f) if f.contains(wl_output::Mode::Current));
                if current || info.width == 0 {
                    info.width = width.max(0) as u32;
                    info.height = height.max(0) as u32;
                    info.refresh_mhz = refresh.max(0) as u32;
                }
            }
            wl_output::Event::Scale { factor } => info.scale = factor,
            wl_output::Event::Name { name } => info.name = name,
            wl_output::Event::Description { description } => info.description = description,
            wl_output::Event::Done => info.done = true,
            _ => {}
        }
    }
}

impl<S> Dispatch<wl_shm::WlShm, (), S> for CoreState
where
    S: Dispatch<wl_shm::WlShm, ()> + AsMut<CoreState>,
{
    fn event(
        state: &mut S,
        _: &wl_shm::WlShm,
        event: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<S>,
    ) {
        if let wl_shm::Event::Format { format } = event {
            let code = match format {
                WEnum::Value(f) => f as u32,
                WEnum::Unknown(v) => v,
            };
            state.as_mut().shm_formats.push(code);
        }
    }
}

impl<S> Dispatch<wl_callback::WlCallback, (), S> for CoreState
where
    S: Dispatch<wl_callback::WlCallback, ()> + AsMut<CoreState>,
{
    fn event(
        state: &mut S,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<S>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.as_mut().syncs_done += 1;
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for CoreStateOnly {
    fn event(
        s: &mut Self,
        r: &wl_registry::WlRegistry,
        e: wl_registry::Event,
        d: &(),
        c: &Connection,
        q: &QueueHandle<Self>,
    ) {
        <CoreState as Dispatch<wl_registry::WlRegistry, (), Self>>::event(s, r, e, d, c, q)
    }
}
impl Dispatch<wl_output::WlOutput, ()> for CoreStateOnly {
    fn event(
        s: &mut Self,
        r: &wl_output::WlOutput,
        e: wl_output::Event,
        d: &(),
        c: &Connection,
        q: &QueueHandle<Self>,
    ) {
        <CoreState as Dispatch<wl_output::WlOutput, (), Self>>::event(s, r, e, d, c, q)
    }
}
impl Dispatch<wl_shm::WlShm, ()> for CoreStateOnly {
    fn event(
        s: &mut Self,
        r: &wl_shm::WlShm,
        e: wl_shm::Event,
        d: &(),
        c: &Connection,
        q: &QueueHandle<Self>,
    ) {
        <CoreState as Dispatch<wl_shm::WlShm, (), Self>>::event(s, r, e, d, c, q)
    }
}
impl Dispatch<wl_callback::WlCallback, ()> for CoreStateOnly {
    fn event(
        s: &mut Self,
        r: &wl_callback::WlCallback,
        e: wl_callback::Event,
        d: &(),
        c: &Connection,
        q: &QueueHandle<Self>,
    ) {
        <CoreState as Dispatch<wl_callback::WlCallback, (), Self>>::event(s, r, e, d, c, q)
    }
}
delegate_noop!(CoreStateOnly: ignore wl_compositor::WlCompositor);
delegate_noop!(CoreStateOnly: ignore wl_shm_pool::WlShmPool);
delegate_noop!(CoreStateOnly: ignore wl_buffer::WlBuffer);

/// A queue state that is nothing but the core state (output listing).
#[derive(Default)]
pub struct CoreStateOnly(pub CoreState);

impl AsMut<CoreState> for CoreStateOnly {
    fn as_mut(&mut self) -> &mut CoreState {
        &mut self.0
    }
}

impl AsRef<CoreState> for CoreStateOnly {
    fn as_ref(&self) -> &CoreState {
        &self.0
    }
}

/// Lowest `zwp_linux_dmabuf_v1` version bound (v4 drops the format and
/// modifier event flood; `create_immed` exists since v2).
pub const LINUX_DMABUF_MIN_VERSION: u32 = 4;

/// Binds `zwp_linux_dmabuf_v1` at version 4 when `core` saw it advertised
/// at version 4 or later.
pub fn bind_linux_dmabuf<S>(
    core: &CoreState,
    registry: &wl_registry::WlRegistry,
    qh: &QueueHandle<S>,
) -> Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>
where
    S: Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> + 'static,
{
    let global = core
        .global("zwp_linux_dmabuf_v1")
        .filter(|g| g.version >= LINUX_DMABUF_MIN_VERSION)?;
    Some(
        registry.bind::<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, _, S>(
            global.name,
            LINUX_DMABUF_MIN_VERSION,
            qh,
            (),
        ),
    )
}

/// Connects to `WAYLAND_DISPLAY` (errors without one).
pub fn connect_display() -> Result<Connection, ConnectError> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none_or(|v| v.is_empty()) {
        return Err(ConnectError::NoDisplay);
    }
    Ok(Connection::connect_to_env()?)
}

/// A deadline-aware `wl_display.roundtrip`: issues a sync and dispatches
/// until its callback fires.
pub fn bounded_roundtrip<S>(
    ops: &mut WaylandOps<S>,
    deadline: Instant,
    keep_running: &dyn Fn() -> bool,
) -> WaitResult
where
    S: Dispatch<wl_callback::WlCallback, ()> + AsMut<CoreState> + 'static,
{
    let before = ops.state_mut().as_mut().syncs_done;
    let qh = ops.queue().handle();
    let _callback = ops.connection().display().sync(&qh, ());
    loop {
        if ops.state_mut().as_mut().syncs_done > before {
            return WaitResult::EventReceived;
        }
        match wait_for_event(ops, deadline, keep_running) {
            WaitResult::EventReceived => {}
            other => return other,
        }
    }
}

/// A connection with the registry bound, outputs tracked and `wl_shm`
/// formats known: the starting point for backends and output listing.
pub struct WaylandConnection {
    ops: WaylandOps<CoreStateOnly>,
    registry: wl_registry::WlRegistry,
}

impl WaylandConnection {
    /// Connects and completes two bounded roundtrips (registry, then
    /// output details) within `deadline`.
    pub fn connect(
        deadline: Instant,
        keep_running: &dyn Fn() -> bool,
    ) -> Result<Self, ConnectError> {
        let conn = connect_display()?;
        let queue = conn.new_event_queue::<CoreStateOnly>();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut ops = WaylandOps::new(conn, queue, CoreStateOnly::default());
        for step in ["registry", "outputs"] {
            match bounded_roundtrip(&mut ops, deadline, keep_running) {
                WaitResult::EventReceived => {}
                WaitResult::Timeout => return Err(ConnectError::Roundtrip("timeout")),
                WaitResult::Cancelled => return Err(ConnectError::Roundtrip("cancelled")),
                WaitResult::Disconnected => return Err(ConnectError::Roundtrip("disconnected")),
                WaitResult::Error => return Err(ConnectError::Roundtrip("error")),
            }
            tracing::trace!(step, "roundtrip done");
        }
        Ok(Self { ops, registry })
    }

    /// The tracked outputs.
    pub fn outputs(&self) -> &OutputTracker {
        &self.ops.state().0.outputs
    }

    /// The core state.
    pub fn core(&self) -> &CoreState {
        &self.ops.state().0
    }

    /// The registry proxy (for binding protocol globals).
    pub fn registry(&self) -> &wl_registry::WlRegistry {
        &self.registry
    }

    /// Takes the connection apart for a backend that needs its own queue
    /// state: the `Connection`, the globals and outputs seen so far.
    pub fn into_parts(self) -> (Connection, CoreState) {
        let WaylandConnection { ops, .. } = self;
        let conn = ops.connection().clone();
        let state = ops.into_state();
        (conn, state.0)
    }

    /// The object id of the registry (for tests/logging).
    pub fn registry_id(&self) -> String {
        self.registry.id().to_string()
    }
}
