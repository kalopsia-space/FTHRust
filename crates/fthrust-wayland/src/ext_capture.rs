//! The `ext_image_copy_capture_manager_v1` backend (ports
//! `backend_ext.cpp`), with cursors painted and the output transform
//! honoured (both missing in the reference).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1, ext_output_image_capture_source_manager_v1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1, ext_image_copy_capture_manager_v1,
    ext_image_copy_capture_session_v1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1,
};

use crate::backend::{
    BackendError, CaptureFailure, FailureKind, monotonic_ns, open_failure, wait_failure,
};
use crate::connection::{CoreState, bind_linux_dmabuf, bounded_roundtrip, connect_display};
use crate::dispatch::{WaitResult, WaylandOps, wait_for_event};
use crate::dmabuf::{
    ConstraintsAccumulator, DRM_FORMAT_ARGB8888, DmabufConstraints, DmabufImage, ImageRing, offers,
};
use crate::frame::{FrameSource, FrameView, OutputTransform, PixelLayout, preferred_wl_shm};
use crate::shm::{BufferGeometry, MemfdPool, SlotState};

/// Time allowed for the whole open sequence.
pub const OPEN_DEADLINE: Duration = Duration::from_secs(5);
/// Hard limit for one frame when no earlier frame can be repeated.
pub const FRAME_DEADLINE: Duration = Duration::from_secs(2);
/// Time allowed for new constraints after a `buffer_constraints` failure.
pub const CONSTRAINTS_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Default)]
struct SessionInfo {
    width: u32,
    height: u32,
    shm_formats: Vec<u32>,
    pending_formats: Vec<u32>,
    pending_size: Option<(u32, u32)>,
    dmabuf: ConstraintsAccumulator,
    done_serial: u64,
    stopped: bool,
}

#[derive(Default)]
struct PendingFrame {
    ready: bool,
    failed: Option<ext_image_copy_capture_frame_v1::FailureReason>,
    transform: Option<OutputTransform>,
}

#[derive(Default)]
struct ExtState {
    core: CoreState,
    session: SessionInfo,
    pending: PendingFrame,
    /// A `zwp_linux_buffer_params_v1.failed` arrived since the last reset.
    dmabuf_import_failed: bool,
}

impl AsMut<CoreState> for ExtState {
    fn as_mut(&mut self) -> &mut CoreState {
        &mut self.core
    }
}

macro_rules! delegate_core {
    ($($iface:ty),* $(,)?) => {$(
        impl Dispatch<$iface, ()> for ExtState {
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
delegate_noop!(ExtState: ignore wl_compositor::WlCompositor);
delegate_noop!(ExtState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(ExtState: ignore wl_buffer::WlBuffer);
delegate_noop!(ExtState: ignore ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1);
delegate_noop!(ExtState: ignore ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1);
delegate_noop!(ExtState: ignore ext_image_capture_source_v1::ExtImageCaptureSourceV1);
delegate_noop!(ExtState: ignore zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1);

impl Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ()> for ExtState {
    fn event(
        state: &mut Self,
        _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_linux_buffer_params_v1::Event::Failed = event {
            state.dmabuf_import_failed = true;
        }
    }

    wayland_client::event_created_child!(ExtState, zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (wl_buffer::WlBuffer, ()),
    ]);
}

impl Dispatch<ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1, ()> for ExtState {
    fn event(
        state: &mut Self,
        _: &ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event;
        let s = &mut state.session;
        match event {
            Event::BufferSize { width, height } => s.pending_size = Some((width, height)),
            Event::ShmFormat { format } => s.pending_formats.push(match format {
                WEnum::Value(f) => f as u32,
                WEnum::Unknown(v) => v,
            }),
            Event::DmabufDevice { device } => s.dmabuf.device(&device),
            Event::DmabufFormat { format, modifiers } => s.dmabuf.format(format, &modifiers),
            Event::Done => {
                if let Some((w, h)) = s.pending_size.take() {
                    s.width = w;
                    s.height = h;
                }
                if !s.pending_formats.is_empty() {
                    s.shm_formats = std::mem::take(&mut s.pending_formats);
                }
                s.dmabuf.done();
                s.done_serial += 1;
            }
            Event::Stopped => s.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1, ()> for ExtState {
    fn event(
        state: &mut Self,
        _: &ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event;
        let p = &mut state.pending;
        match event {
            Event::Transform {
                transform: WEnum::Value(t),
            } => p.transform = OutputTransform::from_wl(t as u32),
            Event::Ready => p.ready = true,
            Event::Failed { reason } => {
                p.failed = Some(match reason {
                    WEnum::Value(r) => r,
                    WEnum::Unknown(_) => ext_image_copy_capture_frame_v1::FailureReason::Unknown,
                })
            }
            _ => {}
        }
    }
}

/// An ext-image-copy-capture session on one output.
pub struct ExtImageCopy {
    ops: WaylandOps<ExtState>,
    session: ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
    _source: ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    output_id: wayland_client::backend::ObjectId,
    selected_output: String,
    pool: MemfdPool,
    cancel: Arc<AtomicBool>,
    frame_period: Duration,
    pending_frame: Option<ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1>,
    slots: SlotState,
    pending_slot: usize,
    last_geometry: Option<BufferGeometry>,
    last_transform: OutputTransform,
    last_delivery: Option<Instant>,
    last_timestamp_ns: i64,
    native: (u32, u32),
    linux_dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    dmabuf: Option<DmabufMode>,
}

/// Dmabuf mode: one `wl_buffer` per caller image, used round-robin.
struct DmabufMode {
    buffers: Vec<wl_buffer::WlBuffer>,
    width: u32,
    height: u32,
    stride: u32,
    fourcc: u32,
    ring: ImageRing,
}

impl DmabufMode {
    fn destroy(self) {
        for b in self.buffers {
            b.destroy();
        }
    }
}

impl ExtImageCopy {
    /// Connects, selects `output` (empty = first; `strict` refuses a
    /// fallback), creates a session with cursors painted and probes a frame.
    pub fn open(
        output: &str,
        strict: bool,
        fps: u32,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, BackendError> {
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
        let queue = conn.new_event_queue::<ExtState>();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut ops = WaylandOps::new(conn, queue, ExtState::default());
        for step in ["registry roundtrip", "output roundtrip"] {
            match bounded_roundtrip(&mut ops, deadline, &keep) {
                WaitResult::EventReceived => {}
                other => return Err(open_failure(step, wait_failure(other))),
            }
        }
        let core = &ops.state().core;
        let (Some(copy_global), Some(source_global)) = (
            core.global("ext_image_copy_capture_manager_v1"),
            core.global("ext_output_image_capture_source_manager_v1"),
        ) else {
            return Err(BackendError::new(
                FailureKind::Error,
                "the compositor does not provide ext-image-copy-capture",
            ));
        };
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
        let output_transform = info.transform;
        let copy_manager = registry
            .bind::<ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1, _, ExtState>(
                copy_global.name,
                1,
                &qh,
                (),
            );
        let source_manager = registry
            .bind::<ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1, _, ExtState>(
                source_global.name,
                1,
                &qh,
                (),
            );
        let source = source_manager.create_source(&output_proxy, &qh, ());
        let session = copy_manager.create_session(
            &source,
            ext_image_copy_capture_manager_v1::Options::PaintCursors,
            &qh,
            (),
        );
        source_manager.destroy();
        copy_manager.destroy();
        let linux_dmabuf = bind_linux_dmabuf(&ops.state().core, &registry, &qh);
        let pool = MemfdPool::new(&shm, &qh, 4096)
            .map_err(|e| BackendError::new(FailureKind::Error, e.to_string()))?;
        let mut this = Self {
            ops,
            session,
            _source: source,
            output_id: output_proxy.id(),
            selected_output,
            pool,
            cancel,
            frame_period: Duration::from_nanos(1_000_000_000 / u64::from(fps.max(1))),
            pending_frame: None,
            slots: SlotState::default(),
            pending_slot: 0,
            last_geometry: None,
            last_transform: output_transform,
            last_delivery: None,
            last_timestamp_ns: 0,
            native: (0, 0),
            linux_dmabuf,
            dmabuf: None,
        };
        this.wait_constraints(deadline, 0)
            .map_err(|e| open_failure("buffer constraints", e))?;
        let view = this
            .capture_with_deadline(deadline, false)
            .map_err(|e| open_failure("probe frame", e))?;
        let native = (view.width, view.height);
        this.native = native;
        tracing::info!(output = %this.selected_output, width = native.0, height = native.1, "ext-image-copy-capture ready");
        Ok(this)
    }

    /// Source size learned from the session's buffer constraints, upright
    /// (a 90°/270° transform swaps the buffer's axes).
    pub fn native_size(&self) -> (u32, u32) {
        let (w, h) = self.native;
        if self.last_transform.swaps_axes() {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// The `wl_output.name` being captured.
    pub fn selected_output(&self) -> &str {
        &self.selected_output
    }

    /// Whether the session paints cursors (always).
    pub fn cursor_requested(&self) -> bool {
        true
    }

    /// Whether a completed frame exists to repeat on a static screen.
    pub fn last_frame_available(&self) -> bool {
        match &self.dmabuf {
            Some(mode) => mode.ring.view().is_some(),
            None => self.last_geometry.is_some(),
        }
    }

    /// The session's current dmabuf constraints, when it offers dmabuf and
    /// the compositor has `zwp_linux_dmabuf_v1` v4 or later.
    pub fn dmabuf_constraints(&self) -> Option<DmabufConstraints> {
        self.linux_dmabuf.as_ref()?;
        self.ops.state().session.dmabuf.constraints()
    }

    /// Switches to dmabuf buffers: one `wl_buffer` per image (at least
    /// two, all the session's buffer size, an offered format), used
    /// round-robin so the most recently delivered image is never
    /// re-attached. Views then carry `FrameSource::Dmabuf(index)`.
    pub fn use_dmabuf(&mut self, images: Vec<DmabufImage>) -> Result<(), CaptureFailure> {
        let Some(linux_dmabuf) = self.linux_dmabuf.clone() else {
            return Err(CaptureFailure::Failed(
                "the compositor does not provide zwp_linux_dmabuf_v1 v4".into(),
            ));
        };
        let constraints = self.dmabuf_constraints().ok_or_else(|| {
            CaptureFailure::Failed("the capture session offers no dmabuf buffers".into())
        })?;
        let (width, height) = {
            let s = &self.ops.state().session;
            (s.width, s.height)
        };
        let first = images
            .first()
            .ok_or_else(|| CaptureFailure::Failed("no dmabuf images".into()))?;
        let (fourcc, stride) = (first.fourcc, first.planes.first().map_or(0, |p| p.stride));
        if images.len() < 2 {
            return Err(CaptureFailure::Failed(
                "dmabuf capture needs at least two images".into(),
            ));
        }
        for (i, image) in images.iter().enumerate() {
            if (image.width, image.height) != (width, height)
                || image.fourcc != fourcc
                || image.planes.is_empty()
                || image.planes.len() > 4
                || i32::try_from(image.width).is_err()
                || i32::try_from(image.height).is_err()
            {
                return Err(CaptureFailure::Failed(format!(
                    "dmabuf image {i} ({}x{}, {} planes, fourcc {:#x}) does not fit the {width}x{height} session",
                    image.width,
                    image.height,
                    image.planes.len(),
                    image.fourcc
                )));
            }
            if !offers(&constraints, image.fourcc, image.modifier) {
                return Err(CaptureFailure::Failed(format!(
                    "the session does not offer fourcc {:#x} with modifier {:#x}",
                    image.fourcc, image.modifier
                )));
            }
        }
        self.leave_mode();
        let qh = self.ops.queue().handle();
        self.ops.state_mut().dmabuf_import_failed = false;
        let buffers: Vec<wl_buffer::WlBuffer> = images
            .iter()
            .map(|image| {
                let params = linux_dmabuf.create_params(&qh, ());
                for (idx, plane) in image.planes.iter().enumerate() {
                    params.add(
                        std::os::fd::AsFd::as_fd(&plane.fd),
                        idx as u32,
                        plane.offset,
                        plane.stride,
                        (image.modifier >> 32) as u32,
                        image.modifier as u32,
                    );
                }
                let buffer = params.create_immed(
                    image.width as i32,
                    image.height as i32,
                    image.fourcc,
                    zwp_linux_buffer_params_v1::Flags::empty(),
                    &qh,
                    (),
                );
                params.destroy();
                buffer
            })
            .collect();
        let mode = DmabufMode {
            ring: ImageRing::new(buffers.len()),
            buffers,
            width,
            height,
            stride,
            fourcc,
        };
        // The compositor may refuse an import with `failed`.
        let cancel = self.cancel.clone();
        let keep = move || !cancel.load(Ordering::Relaxed);
        match bounded_roundtrip(&mut self.ops, Instant::now() + CONSTRAINTS_DEADLINE, &keep) {
            WaitResult::EventReceived => {}
            other => {
                mode.destroy();
                return Err(wait_failure(other));
            }
        }
        if self.ops.state().dmabuf_import_failed {
            mode.destroy();
            return Err(CaptureFailure::Failed(
                "the compositor could not import the dmabuf images".into(),
            ));
        }
        tracing::info!(
            images = mode.buffers.len(),
            width,
            height,
            fourcc = format_args!("{fourcc:#x}"),
            "ext-image-copy-capture using dmabuf buffers"
        );
        self.dmabuf = Some(mode);
        Ok(())
    }

    /// Switches back to the shm pool (a no-op in shm mode).
    pub fn use_shm(&mut self) {
        if self.dmabuf.is_some() {
            self.leave_mode();
        }
    }

    /// Drops the in-flight frame, the current mode's delivered frame and
    /// the dmabuf buffers, if any.
    fn leave_mode(&mut self) {
        if let Some(frame) = self.pending_frame.take() {
            frame.destroy();
        }
        if let Some(mode) = self.dmabuf.take() {
            mode.destroy();
        }
        self.slots.reset();
        self.last_geometry = None;
    }

    /// Captures one frame. On a static screen the previous frame is
    /// repeated once per frame period so the stream keeps its rate.
    pub fn capture(&mut self) -> Result<FrameView<'_>, CaptureFailure> {
        let deadline = match self.last_delivery {
            Some(last) if self.last_frame_available() => {
                (last + self.frame_period).max(Instant::now() + Duration::from_millis(1))
            }
            _ => Instant::now() + FRAME_DEADLINE,
        };
        self.capture_with_deadline(deadline, true)
    }

    fn wait_until(
        &mut self,
        deadline: Instant,
        done: impl Fn(&ExtState) -> bool,
    ) -> Result<(), CaptureFailure> {
        let cancel = self.cancel.clone();
        let keep = move || !cancel.load(Ordering::Relaxed);
        loop {
            if done(self.ops.state()) {
                return Ok(());
            }
            match wait_for_event(&mut self.ops, deadline, &keep) {
                WaitResult::EventReceived => {}
                other => return Err(wait_failure(other)),
            }
        }
    }

    /// Waits for a constraints batch newer than `after_serial`.
    fn wait_constraints(
        &mut self,
        deadline: Instant,
        after_serial: u64,
    ) -> Result<(), CaptureFailure> {
        self.wait_until(deadline, |s| {
            s.session.stopped || s.session.done_serial > after_serial
        })?;
        if self.ops.state().session.stopped {
            return Err(CaptureFailure::Failed(
                "the capture session was stopped by the compositor".into(),
            ));
        }
        Ok(())
    }

    fn geometry(&self) -> Result<BufferGeometry, CaptureFailure> {
        let s = &self.ops.state().session;
        let format = preferred_wl_shm(&s.shm_formats).ok_or_else(|| {
            CaptureFailure::Failed("the compositor offers no supported wl_shm format".into())
        })?;
        if s.width == 0 || s.height == 0 {
            return Err(CaptureFailure::Failed(
                "the compositor announced an empty buffer size".into(),
            ));
        }
        Ok(BufferGeometry {
            width: s.width,
            height: s.height,
            stride: s.width * 4,
            format,
        })
    }

    fn request_frame(&mut self) -> Result<(), CaptureFailure> {
        if self.dmabuf.is_some() {
            return self.request_dmabuf_frame();
        }
        let geometry = self.geometry()?;
        let qh = self.ops.queue().handle();
        if self.pool.geometry().is_some_and(|g| g != geometry) {
            // The pool is rebuilt below: the delivered frame is gone.
            self.slots.reset();
            self.last_geometry = None;
        }
        let slot = self.slots.next_request();
        let buffer = self
            .pool
            .ensure(&qh, geometry, slot)
            .map_err(|e| CaptureFailure::Failed(e.to_string()))?;
        self.pending_slot = slot;
        self.ops.state_mut().pending = PendingFrame::default();
        let frame = self.session.create_frame(&qh, ());
        frame.attach_buffer(buffer);
        frame.damage_buffer(0, 0, geometry.width as i32, geometry.height as i32);
        frame.capture();
        self.pending_frame = Some(frame);
        Ok(())
    }

    fn request_dmabuf_frame(&mut self) -> Result<(), CaptureFailure> {
        let (width, height) = {
            let s = &self.ops.state().session;
            (s.width, s.height)
        };
        let qh = self.ops.queue().handle();
        let mode = self.dmabuf.as_ref().expect("dmabuf mode");
        if (mode.width, mode.height) != (width, height) {
            return Err(CaptureFailure::DmabufInvalidated);
        }
        let slot = mode.ring.next_request();
        self.pending_slot = slot;
        self.ops.state_mut().pending = PendingFrame::default();
        let frame = self.session.create_frame(&qh, ());
        frame.attach_buffer(&self.dmabuf.as_ref().expect("dmabuf mode").buffers[slot]);
        frame.damage_buffer(0, 0, width as i32, height as i32);
        frame.capture();
        self.pending_frame = Some(frame);
        Ok(())
    }

    fn capture_with_deadline(
        &mut self,
        deadline: Instant,
        allow_repeat: bool,
    ) -> Result<FrameView<'_>, CaptureFailure> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(CaptureFailure::Cancelled);
        }
        if self.ops.state().session.stopped {
            return Err(CaptureFailure::Failed(
                "the capture session was stopped by the compositor".into(),
            ));
        }
        if self.pending_frame.is_none() {
            self.request_frame()?;
        }
        match self.wait_until(deadline, |s| {
            s.pending.ready || s.pending.failed.is_some() || s.session.stopped
        }) {
            Ok(()) => {}
            Err(CaptureFailure::Timeout) if allow_repeat && self.last_frame_available() => {
                // The compositor holds the request until the screen changes:
                // repeat the previous frame with a fresh timestamp.
                return Ok(self.last_view(true));
            }
            Err(e) => return Err(e),
        }
        let frame = self.pending_frame.take().expect("requested above");
        frame.destroy();
        let state = self.ops.state();
        if state.session.stopped {
            return Err(CaptureFailure::Failed(
                "the capture session was stopped by the compositor".into(),
            ));
        }
        if let Some(reason) = state.pending.failed {
            use ext_image_copy_capture_frame_v1::FailureReason;
            return match reason {
                FailureReason::BufferConstraints if self.dmabuf.is_some() => {
                    // Wait briefly for the new batch so the caller sees
                    // fresh constraints; a missing batch is not an error.
                    let serial = state.session.done_serial;
                    match self.wait_constraints(Instant::now() + CONSTRAINTS_DEADLINE, serial) {
                        Ok(()) | Err(CaptureFailure::Timeout) => {}
                        Err(e) => return Err(e),
                    }
                    Err(CaptureFailure::DmabufInvalidated)
                }
                FailureReason::BufferConstraints => {
                    let serial = state.session.done_serial;
                    self.wait_constraints(Instant::now() + CONSTRAINTS_DEADLINE, serial)?;
                    self.request_frame()?;
                    self.wait_until(Instant::now() + FRAME_DEADLINE, |s| {
                        s.pending.ready || s.pending.failed.is_some()
                    })?;
                    let frame = self.pending_frame.take().expect("requested above");
                    frame.destroy();
                    if self.ops.state().pending.failed.is_some() {
                        return Err(CaptureFailure::Failed(
                            "the compositor rejected the frame twice".into(),
                        ));
                    }
                    self.store_frame();
                    Ok(self.last_view(false))
                }
                FailureReason::Stopped => Err(CaptureFailure::Failed(
                    "the capture session was stopped by the compositor".into(),
                )),
                _ => Err(CaptureFailure::Failed(
                    "the compositor failed the frame".into(),
                )),
            };
        }
        self.store_frame();
        Ok(self.last_view(false))
    }

    fn store_frame(&mut self) {
        if let Some(mode) = &mut self.dmabuf {
            mode.ring.delivered(self.pending_slot);
        } else {
            let geometry = self.pool.geometry();
            self.slots.delivered(self.pending_slot);
            self.last_geometry = geometry;
        }
        if let Some(t) = self.ops.state().pending.transform {
            self.last_transform = t;
        } else if let Some((_, info)) = self
            .ops
            .state()
            .core
            .outputs
            .select(&self.selected_output)
            .filter(|(o, _)| o.id() == self.output_id)
        {
            self.last_transform = info.transform;
        }
        self.last_delivery = Some(Instant::now());
    }

    fn last_view(&mut self, repeated: bool) -> FrameView<'_> {
        if repeated {
            self.last_delivery = Some(Instant::now());
        }
        let timestamp_ns = monotonic_ns().max(self.last_timestamp_ns + 1);
        self.last_timestamp_ns = timestamp_ns;
        if let Some(mode) = &self.dmabuf {
            return FrameView {
                data: &[],
                stride: mode.stride as usize,
                width: mode.width,
                height: mode.height,
                layout: if mode.fourcc == DRM_FORMAT_ARGB8888 {
                    PixelLayout::Bgra
                } else {
                    PixelLayout::Bgrx
                },
                y_invert: false,
                transform: self.last_transform,
                timestamp_ns,
                repeated,
                source: FrameSource::Dmabuf(mode.ring.view().expect("caller checked")),
            };
        }
        let geometry = self.last_geometry.expect("caller checked");
        FrameView {
            data: self.pool.data(self.slots.view_slot().unwrap_or(0)),
            stride: geometry.stride as usize,
            width: geometry.width,
            height: geometry.height,
            layout: PixelLayout::from_wl_shm(geometry.format).unwrap_or(PixelLayout::Bgrx),
            y_invert: false,
            transform: self.last_transform,
            timestamp_ns,
            repeated,
            source: FrameSource::Shm,
        }
    }
}

impl Drop for ExtImageCopy {
    fn drop(&mut self) {
        if let Some(frame) = self.pending_frame.take() {
            frame.destroy();
        }
        if let Some(mode) = self.dmabuf.take() {
            mode.destroy();
        }
        if let Some(d) = self.linux_dmabuf.take() {
            d.destroy();
        }
        self.session.destroy();
    }
}
