//! The `zwlr_screencopy_manager_v1` backend (ports `backend_wlr.cpp`),
//! with the cursor composited into every frame (the reference left it out).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};

use crate::backend::{
    BackendError, CaptureFailure, FailureKind, monotonic_ns, open_failure, wait_failure,
};
use crate::connection::{CoreState, bounded_roundtrip, connect_display};
use crate::dispatch::{WaitResult, WaylandOps, wait_for_event};
use crate::frame::{FrameView, OutputTransform, PixelLayout};
use crate::shm::{BufferGeometry, MemfdPool};

/// Time allowed for the whole open sequence.
pub const OPEN_DEADLINE: Duration = Duration::from_secs(5);
/// Time allowed for one frame.
pub const FRAME_DEADLINE: Duration = Duration::from_secs(2);

/// What the pending frame reported so far.
#[derive(Default)]
struct PendingFrame {
    geometry: Option<BufferGeometry>,
    y_invert: bool,
    buffer_done: bool,
    ready: bool,
    failed: bool,
}

#[derive(Default)]
struct WlrState {
    core: CoreState,
    pending: PendingFrame,
}

impl AsMut<CoreState> for WlrState {
    fn as_mut(&mut self) -> &mut CoreState {
        &mut self.core
    }
}

macro_rules! delegate_core {
    ($($iface:ty),* $(,)?) => {$(
        impl Dispatch<$iface, ()> for WlrState {
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
delegate_noop!(WlrState: ignore wl_compositor::WlCompositor);
delegate_noop!(WlrState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(WlrState: ignore wl_buffer::WlBuffer);
delegate_noop!(WlrState: ignore zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1);

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for WlrState {
    fn event(
        state: &mut Self,
        _: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event;
        let p = &mut state.pending;
        match event {
            Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let format = match format {
                    WEnum::Value(f) => f as u32,
                    WEnum::Unknown(v) => v,
                };
                p.geometry = Some(BufferGeometry {
                    width,
                    height,
                    stride,
                    format,
                });
            }
            Event::Flags { flags } => {
                p.y_invert = matches!(flags, WEnum::Value(f) if f.contains(zwlr_screencopy_frame_v1::Flags::YInvert));
            }
            Event::BufferDone => p.buffer_done = true,
            Event::Ready { .. } => p.ready = true,
            Event::Failed => p.failed = true,
            _ => {}
        }
    }
}

/// A wlr-screencopy capture of one output.
pub struct WlrScreencopy {
    ops: WaylandOps<WlrState>,
    manager: zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    manager_version: u32,
    output: wl_output::WlOutput,
    output_id: wayland_client::backend::ObjectId,
    selected_output: String,
    pool: MemfdPool,
    cancel: Arc<AtomicBool>,
    native: (u32, u32),
    last_timestamp_ns: i64,
}

impl WlrScreencopy {
    /// Connects, selects `output` (empty = first; `strict` refuses a
    /// fallback) and probes one frame to learn the geometry.
    pub fn open(output: &str, strict: bool, cancel: Arc<AtomicBool>) -> Result<Self, BackendError> {
        let deadline = Instant::now() + OPEN_DEADLINE;
        let keep = {
            let cancel = cancel.clone();
            move || !cancel.load(Ordering::Relaxed)
        };
        if !keep() {
            return Err(BackendError::new(
                FailureKind::Cancelled,
                "capture cancelled",
            ));
        }
        let conn = connect_display()
            .map_err(|e| BackendError::new(FailureKind::CompositorUnreachable, e.to_string()))?;
        let queue = conn.new_event_queue::<WlrState>();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut ops = WaylandOps::new(conn, queue, WlrState::default());
        for step in ["registry roundtrip", "output roundtrip"] {
            match bounded_roundtrip(&mut ops, deadline, &keep) {
                WaitResult::EventReceived => {}
                other => return Err(open_failure(step, wait_failure(other))),
            }
        }
        let core = &ops.state().core;
        let Some(global) = core.global("zwlr_screencopy_manager_v1") else {
            return Err(BackendError::new(
                FailureKind::Error,
                "the compositor does not provide zwlr_screencopy_manager_v1",
            ));
        };
        let manager_version = global.version.min(3);
        let manager = registry
            .bind::<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, _, WlrState>(
                global.name,
                manager_version,
                &qh,
                (),
            );
        let Some(shm) = core.shm.clone() else {
            return Err(BackendError::new(
                FailureKind::Error,
                "the compositor does not provide wl_shm",
            ));
        };
        let (output_proxy, info) = match core.outputs.select(output) {
            Some(found) => found,
            None if !output.is_empty() && strict => {
                return Err(BackendError::new(
                    FailureKind::OutputAbsent,
                    format!("output {output} is not advertised right now"),
                ));
            }
            None => match core.outputs.select("") {
                Some(first) => {
                    tracing::warn!(wanted = output, using = %first.1.name, "output not found; capturing the first output");
                    first
                }
                None => {
                    return Err(BackendError::new(
                        FailureKind::OutputAbsent,
                        "no output is advertised",
                    ));
                }
            },
        };
        let output_proxy = output_proxy.clone();
        let selected_output = info.name.clone();
        let pool = MemfdPool::new(&shm, &qh, 4096)
            .map_err(|e| BackendError::new(FailureKind::Error, e.to_string()))?;
        let mut this = Self {
            output_id: output_proxy.id(),
            output: output_proxy,
            ops,
            manager,
            manager_version,
            selected_output,
            pool,
            cancel,
            native: (0, 0),
            last_timestamp_ns: 0,
        };
        // Probe one frame to learn size, stride and format.
        let view = this
            .capture_with_deadline(deadline)
            .map_err(|e| open_failure("probe frame", e))?;
        let native = (view.width, view.height);
        this.native = native;
        tracing::info!(
            output = %this.selected_output,
            width = native.0,
            height = native.1,
            version = manager_version,
            "wlr-screencopy ready"
        );
        Ok(this)
    }

    /// Source size learned from the probe frame.
    pub fn native_size(&self) -> (u32, u32) {
        self.native
    }

    /// The `wl_output.name` being captured.
    pub fn selected_output(&self) -> &str {
        &self.selected_output
    }

    /// Whether frames are requested with the cursor overlaid (always).
    pub fn cursor_requested(&self) -> bool {
        true
    }

    /// Captures one frame within [`FRAME_DEADLINE`].
    pub fn capture(&mut self) -> Result<FrameView<'_>, CaptureFailure> {
        self.capture_with_deadline(Instant::now() + FRAME_DEADLINE)
    }

    fn wait_until(
        &mut self,
        deadline: Instant,
        done: impl Fn(&PendingFrame) -> bool,
    ) -> Result<(), CaptureFailure> {
        let cancel = self.cancel.clone();
        let keep = move || !cancel.load(Ordering::Relaxed);
        loop {
            if done(&self.ops.state().pending) {
                return Ok(());
            }
            match wait_for_event(&mut self.ops, deadline, &keep) {
                WaitResult::EventReceived => {}
                other => return Err(wait_failure(other)),
            }
        }
    }

    fn capture_with_deadline(
        &mut self,
        deadline: Instant,
    ) -> Result<FrameView<'_>, CaptureFailure> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(CaptureFailure::Cancelled);
        }
        let qh = self.ops.queue().handle();
        self.ops.state_mut().pending = PendingFrame::default();
        let frame = self.manager.capture_output(1, &self.output, &qh, ());
        let result = self.run_frame(&frame, deadline);
        frame.destroy();
        result?;
        let geometry = self
            .pool
            .geometry()
            .ok_or_else(|| CaptureFailure::Failed("no buffer".into()))?;
        let layout = PixelLayout::from_wl_shm(geometry.format).ok_or_else(|| {
            CaptureFailure::Failed(format!("unsupported wl_shm format {:#x}", geometry.format))
        })?;
        let transform = self.output_transform().unwrap_or(OutputTransform::Normal);
        let y_invert = self.ops.state().pending.y_invert;
        let timestamp_ns = monotonic_ns().max(self.last_timestamp_ns + 1);
        self.last_timestamp_ns = timestamp_ns;
        Ok(FrameView {
            data: self.pool.data(),
            stride: geometry.stride as usize,
            width: geometry.width,
            height: geometry.height,
            layout,
            y_invert,
            transform,
            timestamp_ns,
        })
    }

    fn output_transform(&self) -> Option<OutputTransform> {
        let core = &self.ops.state().core;
        core.outputs
            .select(&self.selected_output)
            .filter(|(o, _)| o.id() == self.output_id)
            .map(|(_, info)| info.transform)
    }

    fn run_frame(
        &mut self,
        frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        deadline: Instant,
    ) -> Result<(), CaptureFailure> {
        // v3 announces every buffer type then `buffer_done`; older versions
        // only send `buffer`, after which the copy may start.
        let needs_done = self.manager_version >= 3;
        self.wait_until(deadline, |p| {
            p.failed
                || if needs_done {
                    p.buffer_done
                } else {
                    p.geometry.is_some()
                }
        })?;
        if self.ops.state().pending.failed {
            return Err(CaptureFailure::Failed(
                "the compositor failed the frame request".into(),
            ));
        }
        let geometry = self.ops.state().pending.geometry.ok_or_else(|| {
            CaptureFailure::Failed("the compositor offered no wl_shm buffer".into())
        })?;
        let qh = self.ops.queue().handle();
        let buffer = self
            .pool
            .ensure(&qh, geometry)
            .map_err(|e| CaptureFailure::Failed(e.to_string()))?;
        frame.copy(buffer);
        self.wait_until(deadline, |p| p.ready || p.failed)?;
        if self.ops.state().pending.failed {
            return Err(CaptureFailure::Failed(
                "the compositor failed the frame copy".into(),
            ));
        }
        Ok(())
    }
}
