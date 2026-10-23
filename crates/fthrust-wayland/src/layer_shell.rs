//! An overlay surface through `wlr-layer-shell` for the capture card: a
//! `wl_surface` on the overlay layer anchored top-right of one output with
//! margins, no exclusive zone and no keyboard interactivity, plus the raw
//! handles a wgpu surface is created from.

use std::ptr::NonNull;
use std::time::{Duration, Instant};

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, WindowHandle,
};
use wayland_client::protocol::{
    wl_callback, wl_compositor, wl_output, wl_registry, wl_shm, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::connection::{ConnectError, CoreState, bounded_roundtrip, connect_display};
use crate::dispatch::{WaitResult, WaylandOps, wait_for_event};

/// Why the surface could not be created.
#[derive(Debug, thiserror::Error)]
pub enum LayerError {
    /// No display / connection failed.
    #[error(transparent)]
    Connect(#[from] ConnectError),
    /// The compositor has no `zwlr_layer_shell_v1`.
    #[error("the compositor does not expose zwlr_layer_shell_v1")]
    Unavailable,
    /// No `wl_compositor` or no outputs.
    #[error("{0}")]
    Missing(&'static str),
    /// The compositor never configured the surface.
    #[error("the layer surface was not configured within {0:?}")]
    ConfigureTimeout(Duration),
    /// The compositor closed the surface.
    #[error("the compositor closed the layer surface")]
    Closed,
}

#[derive(Default)]
struct LayerState {
    core: CoreState,
    configured: Option<(u32, u32, u32)>,
    closed: bool,
}

impl AsMut<CoreState> for LayerState {
    fn as_mut(&mut self) -> &mut CoreState {
        &mut self.core
    }
}

macro_rules! delegate_core {
    ($($iface:ty),* $(,)?) => {$(
        impl Dispatch<$iface, ()> for LayerState {
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
delegate_noop!(LayerState: ignore wl_compositor::WlCompositor);
delegate_noop!(LayerState: ignore wl_surface::WlSurface);
delegate_noop!(LayerState: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for LayerState {
    fn event(
        state: &mut Self,
        surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                surface.ack_configure(serial);
                state.configured = Some((serial, width, height));
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

/// Whether the current compositor offers `zwlr_layer_shell_v1`.
pub fn available() -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    let keep = || true;
    crate::connection::WaylandConnection::connect(deadline, &keep)
        .map(|c| c.core().global("zwlr_layer_shell_v1").is_some())
        .unwrap_or(false)
}

/// Raw handles for a wgpu surface.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceHandles {
    display: NonNull<std::ffi::c_void>,
    surface: NonNull<std::ffi::c_void>,
}

// Thread safety: the pointers refer to libwayland objects owned by the
// `LayerSurface`, which outlives every handle use by construction; libwayland
// proxies may be used from any thread.
#[allow(unsafe_code)]
unsafe impl Send for SurfaceHandles {}
#[allow(unsafe_code)]
unsafe impl Sync for SurfaceHandles {}

impl HasDisplayHandle for SurfaceHandles {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let raw = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(self.display));
        // SAFETY: the display pointer stays valid while the LayerSurface lives.
        #[allow(unsafe_code)]
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

impl HasWindowHandle for SurfaceHandles {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::Wayland(WaylandWindowHandle::new(self.surface));
        // SAFETY: the wl_surface proxy stays valid while the LayerSurface lives.
        #[allow(unsafe_code)]
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

/// An overlay-layer surface anchored top-right.
pub struct LayerSurface {
    ops: WaylandOps<LayerState>,
    surface: wl_surface::WlSurface,
    layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    size: (u32, u32),
    output_name: String,
}

const CONFIGURE_DEADLINE: Duration = Duration::from_secs(1);

impl LayerSurface {
    /// Creates the surface on `output` (a `wl_output` name; `None` or an
    /// unknown name picks the highest-refresh output) with `size` and
    /// `margins` `(top, right, bottom, left)`, waits for the first
    /// configure and acknowledges it.
    pub fn create(
        output: Option<&str>,
        size: (u32, u32),
        margins: (i32, i32, i32, i32),
        namespace: &str,
    ) -> Result<LayerSurface, LayerError> {
        let conn = connect_display()?;
        let queue = conn.new_event_queue::<LayerState>();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut ops = WaylandOps::new(conn, queue, LayerState::default());
        let deadline = Instant::now() + Duration::from_secs(5);
        let keep = || true;
        for _ in 0..2 {
            match bounded_roundtrip(&mut ops, deadline, &keep) {
                WaitResult::EventReceived => {}
                _ => return Err(ConnectError::Roundtrip("timeout").into()),
            }
        }
        let core = &ops.state().core;
        let Some(global) = core.global("zwlr_layer_shell_v1") else {
            return Err(LayerError::Unavailable);
        };
        let shell = registry.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, LayerState>(
            global.name,
            global.version.min(4),
            &qh,
            (),
        );
        let Some(compositor) = core.compositor.clone() else {
            return Err(LayerError::Missing("wl_compositor"));
        };
        let (wl_output, output_name) = {
            let wanted = output.unwrap_or("");
            let chosen = core.outputs.select(wanted).or_else(|| {
                core.outputs
                    .outputs()
                    .into_iter()
                    .max_by_key(|o| o.refresh_mhz)
                    .and_then(|best| core.outputs.select(&best.name))
            });
            let Some((proxy, info)) = chosen else {
                return Err(LayerError::Missing("no output is advertised"));
            };
            (proxy.clone(), info.name.clone())
        };
        let surface = compositor.create_surface(&qh, ());
        let layer_surface = shell.get_layer_surface(
            &surface,
            Some(&wl_output),
            zwlr_layer_shell_v1::Layer::Overlay,
            namespace.to_owned(),
            &qh,
            (),
        );
        layer_surface.set_size(size.0, size.1);
        layer_surface
            .set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Right);
        layer_surface.set_exclusive_zone(0);
        layer_surface.set_margin(margins.0, margins.1, margins.2, margins.3);
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        surface.commit();
        let _ = ops.connection().flush();
        let configure_deadline = Instant::now() + CONFIGURE_DEADLINE;
        loop {
            if ops.state().closed {
                return Err(LayerError::Closed);
            }
            if ops.state().configured.is_some() {
                break;
            }
            match wait_for_event(&mut ops, configure_deadline, &keep) {
                WaitResult::EventReceived => {}
                WaitResult::Timeout => {
                    return Err(LayerError::ConfigureTimeout(CONFIGURE_DEADLINE));
                }
                _ => return Err(ConnectError::Roundtrip("disconnected").into()),
            }
        }
        let (_, w, h) = ops.state().configured.unwrap_or((0, size.0, size.1));
        let size = (
            if w == 0 { size.0 } else { w },
            if h == 0 { size.1 } else { h },
        );
        Ok(LayerSurface {
            ops,
            surface,
            layer_surface,
            size,
            output_name,
        })
    }

    /// The configured size.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The output the surface was placed on.
    pub fn output_name(&self) -> &str {
        &self.output_name
    }

    /// Moves the surface by changing its margins (`right`, `top`).
    pub fn set_margins(&mut self, right: i32, top: i32) {
        self.layer_surface.set_margin(top, right, 0, 0);
        self.surface.commit();
        let _ = self.ops.connection().flush();
    }

    /// The underlying `wl_surface`.
    pub fn wl_surface(&self) -> &wl_surface::WlSurface {
        &self.surface
    }

    /// The handles a wgpu surface is created from.
    pub fn handles(&self) -> SurfaceHandles {
        let display = self
            .ops
            .connection()
            .backend()
            .display_ptr()
            .cast::<std::ffi::c_void>();
        let surface = self.surface.id().as_ptr().cast::<std::ffi::c_void>();
        SurfaceHandles {
            display: NonNull::new(display).expect("live display"),
            surface: NonNull::new(surface).expect("live surface"),
        }
    }

    /// Dispatches pending events for up to `timeout`; `false` once the
    /// compositor closed the surface or the connection died.
    pub fn pump(&mut self, timeout: Duration) -> bool {
        let keep = || true;
        match wait_for_event(&mut self.ops, Instant::now() + timeout, &keep) {
            WaitResult::Disconnected | WaitResult::Error => return false,
            _ => {}
        }
        if let Some((_, w, h)) = self.ops.state().configured
            && w != 0
            && h != 0
        {
            self.size = (w, h);
        }
        !self.ops.state().closed
    }
}

impl Drop for LayerSurface {
    fn drop(&mut self) {
        self.layer_surface.destroy();
        self.surface.destroy();
        let _ = self.ops.connection().flush();
    }
}
