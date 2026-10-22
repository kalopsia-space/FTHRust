//! The PipeWire video stream behind a ScreenCast session (ports
//! `backend_portal.cpp` and `pipewire_frame.cpp`). The pure helpers live
//! here; the stream itself is in [`PipeWireVideoStream`].

use fthrust_wayland::frame::{BgraFrame, PixelLayout};

/// What the negotiated format looks like in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameLayout {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes per row as delivered.
    pub stride: usize,
    /// Pixel byte order.
    pub layout: PixelLayout,
}

/// Why a frame could not be copied.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CopyError {
    /// The buffer is shorter than its geometry.
    #[error("PipeWire buffer holds {0} bytes but its layout needs {1}")]
    ShortBuffer(usize, usize),
}

/// Copies `height` rows out of a PipeWire buffer into a packed BGRA frame,
/// swapping R and B for RGB-order layouts and dropping row padding.
pub fn copy_rows(
    src: &[u8],
    layout: &FrameLayout,
    timestamp_ns: i64,
    out: &mut BgraFrame,
) -> Result<(), CopyError> {
    let (w, h) = (layout.width as usize, layout.height as usize);
    let row = w * 4;
    let needed = if h == 0 {
        0
    } else {
        layout.stride * (h - 1) + row
    };
    if src.len() < needed || layout.stride < row {
        return Err(CopyError::ShortBuffer(
            src.len(),
            needed.max(layout.stride.max(row)),
        ));
    }
    out.data.clear();
    out.data.reserve(row * h);
    let swap = matches!(layout.layout, PixelLayout::Rgba | PixelLayout::Rgbx);
    let opaque = matches!(layout.layout, PixelLayout::Rgbx | PixelLayout::Bgrx);
    for y in 0..h {
        let line = &src[y * layout.stride..y * layout.stride + row];
        if !swap && !opaque {
            out.data.extend_from_slice(line);
            continue;
        }
        for px in line.as_chunks::<4>().0 {
            let (b, g, r) = if swap {
                (px[2], px[1], px[0])
            } else {
                (px[0], px[1], px[2])
            };
            out.data
                .extend_from_slice(&[b, g, r, if opaque { 255 } else { px[3] }]);
        }
    }
    out.width = layout.width;
    out.height = layout.height;
    out.stride = row;
    out.timestamp_ns = timestamp_ns;
    Ok(())
}

/// A header pts is trusted only when it is within one second of now.
pub fn choose_pts(header_pts_ns: Option<i64>, now_ns: i64) -> i64 {
    match header_pts_ns {
        Some(pts) if (now_ns - pts).abs() <= 1_000_000_000 => pts,
        _ => now_ns,
    }
}

/// Timestamps are forced strictly increasing.
pub fn monotonic(last_ns: i64, candidate_ns: i64) -> i64 {
    candidate_ns.max(last_ns + 1)
}

/// The timestamp of the frame being returned: a fresh frame keeps its pts
/// (clamped monotonic); a repeat of the last frame is stamped `now` so time
/// keeps advancing on a static screen.
pub fn next_timestamp(last_ns: i64, fresh_pts_ns: Option<i64>, now_ns: i64) -> i64 {
    monotonic(last_ns, fresh_pts_ns.unwrap_or(now_ns))
}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use fthrust_wayland::backend::{CaptureFailure, monotonic_ns};
use fthrust_wayland::frame::{FrameView, OutputTransform};
use pipewire as pw;
use pw::spa::param::video::VideoFormat;

/// How long the compositor may take to settle a format.
pub const FORMAT_DEADLINE: Duration = Duration::from_secs(10);
/// How long the first frame may take.
pub const FIRST_FRAME_DEADLINE: Duration = Duration::from_secs(5);

/// Why the stream could not be connected.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// libpipewire failed.
    #[error("pipewire: {0}")]
    PipeWire(String),
    /// No format within [`FORMAT_DEADLINE`].
    #[error("the screen stream did not settle a format in time")]
    FormatTimeout,
    /// The cancel flag was set.
    #[error("capture cancelled")]
    Cancelled,
}

#[derive(Default)]
struct Shared {
    layout: Option<FrameLayout>,
    pending: Option<BgraFrame>,
    spare: Option<BgraFrame>,
    error: Option<String>,
    resolution_changed: bool,
    dmabuf_only: bool,
    frames: u64,
}

struct Slot {
    shared: Mutex<Shared>,
    cv: Condvar,
}

fn pixel_layout(format: VideoFormat) -> Option<PixelLayout> {
    Some(match format {
        VideoFormat::BGRx => PixelLayout::Bgrx,
        VideoFormat::BGRA => PixelLayout::Bgra,
        VideoFormat::RGBx => PixelLayout::Rgbx,
        VideoFormat::RGBA => PixelLayout::Rgba,
        _ => return None,
    })
}

/// Describes a negotiated raw video format, when it is one we copy.
pub fn describe_spa_format(
    format: VideoFormat,
    width: u32,
    height: u32,
    stride: usize,
) -> Option<FrameLayout> {
    let layout = pixel_layout(format)?;
    if width == 0 || height == 0 {
        return None;
    }
    Some(FrameLayout {
        width,
        height,
        stride: if stride == 0 {
            width as usize * 4
        } else {
            stride
        },
        layout,
    })
}

enum LoopMessage {
    Quit,
}

/// The PipeWire video stream of a ScreenCast session, running on its own
/// loop thread; frames are copied into a slot the capture side drains.
pub struct PipeWireVideoStream {
    slot: Arc<Slot>,
    quit: Option<pw::channel::Sender<LoopMessage>>,
    thread: Option<std::thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    closed: Option<tokio::sync::watch::Receiver<bool>>,
    frame_period: Duration,
    last: Option<BgraFrame>,
    last_delivery: Option<Instant>,
    last_timestamp_ns: i64,
    native: (u32, u32),
}

impl PipeWireVideoStream {
    /// Connects to `node_id` over `fd` and waits for the format.
    pub fn connect(
        fd: std::os::fd::OwnedFd,
        node_id: u32,
        fps: u32,
        cancel: Arc<AtomicBool>,
        closed: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<Self, StreamError> {
        crate::init_pipewire();
        let slot = Arc::new(Slot {
            shared: Mutex::new(Shared::default()),
            cv: Condvar::new(),
        });
        let (quit_tx, quit_rx) = pw::channel::channel::<LoopMessage>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let slot_for_thread = slot.clone();
        let thread = std::thread::Builder::new()
            .name("fthrust-portal-pw".into())
            .spawn(move || run_loop(fd, node_id, fps, slot_for_thread, quit_rx, ready_tx))
            .map_err(|e| StreamError::PipeWire(e.to_string()))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(StreamError::PipeWire(e));
            }
            Err(_) => {
                let _ = quit_tx.send(LoopMessage::Quit);
                let _ = thread.join();
                return Err(StreamError::PipeWire(
                    "the PipeWire loop did not start".into(),
                ));
            }
        }
        let mut this = Self {
            slot,
            quit: Some(quit_tx),
            thread: Some(thread),
            cancel,
            closed,
            frame_period: Duration::from_nanos(1_000_000_000 / u64::from(fps.max(1))),
            last: None,
            last_delivery: None,
            last_timestamp_ns: 0,
            native: (0, 0),
        };
        // Wait for the format.
        let deadline = Instant::now() + FORMAT_DEADLINE;
        loop {
            {
                let shared = this.slot.shared.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(e) = &shared.error {
                    return Err(StreamError::PipeWire(e.clone()));
                }
                if let Some(layout) = shared.layout {
                    this.native = (layout.width, layout.height);
                    break;
                }
            }
            if this.cancel.load(Ordering::Relaxed) {
                return Err(StreamError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(StreamError::FormatTimeout);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(this)
    }

    /// Negotiated size.
    pub fn native_size(&self) -> (u32, u32) {
        self.native
    }

    /// Captures the newest frame: waits up to one frame period after the
    /// last delivery (5 s for the first frame), repeating the last frame on
    /// a static screen.
    pub fn capture(&mut self) -> Result<FrameView<'_>, CaptureFailure> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(CaptureFailure::Cancelled);
        }
        if let Some(closed) = &self.closed
            && *closed.borrow()
        {
            return Err(CaptureFailure::Failed(
                "the portal session was closed".into(),
            ));
        }
        let deadline = match (self.last_delivery, &self.last) {
            (Some(last), Some(_)) => {
                (last + self.frame_period).max(Instant::now() + Duration::from_millis(1))
            }
            _ => Instant::now() + FIRST_FRAME_DEADLINE,
        };
        let taken = {
            let mut shared = self.slot.shared.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if let Some(e) = &shared.error {
                    return Err(CaptureFailure::Failed(e.clone()));
                }
                if shared.dmabuf_only {
                    return Err(CaptureFailure::Failed(
                        "the compositor offers this screen only as DMA-BUF, which this build does not import".into(),
                    ));
                }
                if shared.resolution_changed {
                    return Err(CaptureFailure::Failed("stream resolution changed".into()));
                }
                if let Some(frame) = shared.pending.take() {
                    if let Some(old) = self.last.take() {
                        shared.spare = Some(old);
                    }
                    break Some(frame);
                }
                let now = Instant::now();
                if now >= deadline {
                    break None;
                }
                if self.cancel.load(Ordering::Relaxed) {
                    return Err(CaptureFailure::Cancelled);
                }
                let wait = (deadline - now).min(Duration::from_millis(50));
                let (guard, _) = self
                    .slot
                    .cv
                    .wait_timeout(shared, wait)
                    .unwrap_or_else(|p| p.into_inner());
                shared = guard;
            }
        };
        match &taken {
            Some(_) => {
                self.last = taken.clone();
                self.last_delivery = Some(Instant::now());
            }
            None if self.last.is_some() => {
                self.last_delivery = Some(Instant::now());
            }
            None => return Err(CaptureFailure::Timeout),
        }
        let fresh = taken.is_some();
        let last = self.last.as_mut().expect("set above");
        let ts = next_timestamp(
            self.last_timestamp_ns,
            fresh.then_some(last.timestamp_ns),
            monotonic_ns(),
        );
        self.last_timestamp_ns = ts;
        last.timestamp_ns = ts;
        Ok(FrameView {
            data: &last.data,
            stride: last.stride,
            width: last.width,
            height: last.height,
            layout: PixelLayout::Bgra,
            y_invert: false,
            transform: OutputTransform::Normal,
            timestamp_ns: ts,
        })
    }

    /// Frames delivered by PipeWire so far.
    pub fn frames_received(&self) -> u64 {
        self.slot
            .shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .frames
    }
}

impl Drop for PipeWireVideoStream {
    fn drop(&mut self) {
        if let Some(quit) = self.quit.take() {
            let _ = quit.send(LoopMessage::Quit);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_loop(
    fd: std::os::fd::OwnedFd,
    node_id: u32,
    fps: u32,
    slot: Arc<Slot>,
    quit_rx: pw::channel::Receiver<LoopMessage>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let fail = |ready: &std::sync::mpsc::Sender<Result<(), String>>, msg: String| {
        let _ = ready.send(Err(msg));
    };
    let main_loop = match pw::main_loop::MainLoopRc::new(None) {
        Ok(l) => l,
        Err(e) => return fail(&ready, format!("main loop: {e}")),
    };
    let context = match pw::context::ContextRc::new(&main_loop, None) {
        Ok(c) => c,
        Err(e) => return fail(&ready, format!("context: {e}")),
    };
    let core = match context.connect_fd_rc(fd, None) {
        Ok(c) => c,
        Err(e) => return fail(&ready, format!("connect_fd: {e}")),
    };
    let stream = match pw::stream::StreamRc::new(
        core,
        "fthrust-screencast",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    ) {
        Ok(s) => s,
        Err(e) => return fail(&ready, format!("stream: {e}")),
    };
    let _quit = quit_rx.attach(main_loop.loop_(), {
        let main_loop = main_loop.clone();
        move |LoopMessage::Quit| main_loop.quit()
    });
    let slot_for_listener = slot.clone();
    let listener = stream
        .add_local_listener_with_user_data(pw::spa::param::video::VideoInfoRaw::default())
        .state_changed({
            let slot = slot.clone();
            move |_, _, _old, new| {
                if let pw::stream::StreamState::Error(e) = new {
                    let mut shared = slot.shared.lock().unwrap_or_else(|p| p.into_inner());
                    shared.error = Some(format!("PipeWire stream error: {e}"));
                    slot.cv.notify_all();
                }
            }
        })
        .param_changed({
            let slot = slot_for_listener;
            move |_, info, id, param| {
                let Some(param) = param else { return };
                if id != pw::spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Ok((media_type, media_subtype)) =
                    pw::spa::param::format_utils::parse_format(param)
                else {
                    return;
                };
                if media_type != pw::spa::param::format::MediaType::Video
                    || media_subtype != pw::spa::param::format::MediaSubtype::Raw
                {
                    return;
                }
                if info.parse(param).is_err() {
                    return;
                }
                let mut shared = slot.shared.lock().unwrap_or_else(|p| p.into_inner());
                match describe_spa_format(info.format(), info.size().width, info.size().height, 0) {
                    Some(layout) => {
                        if let Some(previous) = shared.layout
                            && (previous.width, previous.height) != (layout.width, layout.height)
                        {
                            shared.resolution_changed = true;
                        }
                        shared.layout = Some(layout);
                    }
                    None => {
                        shared.error =
                            Some(format!("unsupported stream format {:?}", info.format()))
                    }
                }
                slot.cv.notify_all();
            }
        })
        .process({
            let slot = slot.clone();
            move |stream, _| {
                // Keep only the newest queued buffer.
                let mut newest = None;
                while let Some(buffer) = stream.dequeue_buffer() {
                    newest = Some(buffer);
                }
                let Some(mut buffer) = newest else { return };
                let header_pts = buffer
                    .find_meta::<pw::spa::buffer::meta::MetaHeader>()
                    .map(|h| h.pts());
                let datas = buffer.datas_mut();
                let Some(data) = datas.first_mut() else {
                    return;
                };
                let mut shared = slot.shared.lock().unwrap_or_else(|p| p.into_inner());
                let Some(mut layout) = shared.layout else {
                    return;
                };
                if data.type_() == pw::spa::buffer::DataType::DmaBuf {
                    shared.dmabuf_only = true;
                    slot.cv.notify_all();
                    return;
                }
                let chunk = data.chunk();
                let (size, offset, stride) = (
                    chunk.size() as usize,
                    chunk.offset() as usize,
                    chunk.stride(),
                );
                if stride > 0 {
                    layout.stride = stride as usize;
                }
                if size == 0 {
                    return;
                }
                let Some(bytes) = data.data() else { return };
                let end = offset.saturating_add(size).min(bytes.len());
                if offset >= end {
                    return;
                }
                let src = &bytes[offset..end];
                let now = monotonic_ns();
                let pts = choose_pts(header_pts.filter(|p| *p > 0), now);
                let mut frame = shared
                    .spare
                    .take()
                    .or_else(|| shared.pending.take())
                    .unwrap_or_default();
                if copy_rows(src, &layout, pts, &mut frame).is_ok() {
                    shared.pending = Some(frame);
                    shared.frames += 1;
                    slot.cv.notify_all();
                } else {
                    shared.spare = Some(frame);
                }
            }
        })
        .register();
    let _listener = match listener {
        Ok(l) => l,
        Err(e) => return fail(&ready, format!("listener: {e}")),
    };

    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            pw::spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            pw::spa::utils::Rectangle {
                width: 16384,
                height: 16384
            }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: fps, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction {
                num: 1000,
                denom: 1
            }
        ),
    );
    let values: Vec<u8> = match pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    ) {
        Ok((cursor, _)) => cursor.into_inner(),
        Err(e) => return fail(&ready, format!("format pod: {e:?}")),
    };
    let Some(pod) = pw::spa::pod::Pod::from_bytes(&values) else {
        return fail(&ready, "format pod is malformed".into());
    };
    let mut params = [pod];
    if let Err(e) = stream.connect(
        pw::spa::utils::Direction::Input,
        Some(node_id),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    ) {
        return fail(&ready, format!("stream connect: {e}"));
    }
    let _ = ready.send(Ok(()));
    main_loop.run();
    let _ = stream.disconnect();
    let mut shared = slot.shared.lock().unwrap_or_else(|p| p.into_inner());
    if shared.error.is_none() {
        shared.error = Some("the PipeWire loop stopped".into());
    }
    slot.cv.notify_all();
}
