//! The capture backend abstraction the engine drives, adapters over the
//! Wayland and portal crates, and a synthetic backend for tests.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fthrust_core::capture_config::CaptureConfig;
use fthrust_ipc::BackendKind;
use fthrust_wayland::backend::{self as wb, FirstDmabufFrame, monotonic_ns};
use fthrust_wayland::dmabuf::{DmabufConstraints, DmabufImage};
use fthrust_wayland::frame::{FrameSource, FrameView, OutputTransform, PixelLayout};

pub use fthrust_wayland::backend::{CaptureFailure as CaptureError, FailureKind};

/// An open failure with words for the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendFailure {
    /// Classification.
    pub kind: FailureKind,
    /// Human-readable reason.
    pub reason: String,
    /// Only user action can resolve it (a dismissed portal dialog).
    pub retry_pointless: bool,
    /// With `retry_pointless`: the portal could only go on by showing its
    /// dialog, which this open may not ([`OpenReason::Recovery`]). Capture
    /// stops with `CAPTURE_NOT_RESTORED` instead of `CAPTURE_DECLINED`.
    pub needs_dialog: bool,
}

impl From<wb::BackendError> for BackendFailure {
    fn from(e: wb::BackendError) -> Self {
        BackendFailure {
            kind: e.kind,
            reason: e.reason,
            retry_pointless: false,
            needs_dialog: false,
        }
    }
}

/// A source of frames.
pub trait CaptureBackend: Send {
    /// Which backend.
    fn kind(&self) -> BackendKind;
    /// The next frame, or why the generation must end.
    fn capture(&mut self) -> Result<FrameView<'_>, CaptureError>;
    /// Source size.
    fn native_size(&self) -> (u32, u32);
    /// The output being captured (`wl_output.name`), if known.
    fn selected_output(&self) -> String;
    /// Whether a completed frame exists to repeat.
    fn last_frame_available(&self) -> bool {
        false
    }
    /// The compositor's current buffer size, where the protocol says it.
    fn buffer_size(&self) -> Option<(u32, u32)> {
        None
    }
    /// What the compositor accepts for dmabuf capture buffers, if anything.
    fn dmabuf_constraints(&self) -> Option<DmabufConstraints> {
        None
    }
    /// Captures into `images` from now on; on error the backend stays on shm.
    fn use_dmabuf(&mut self, images: Vec<DmabufImage>) -> Result<(), CaptureError> {
        drop(images);
        Err(CaptureError::Failed(
            "this backend has no dmabuf capture".into(),
        ))
    }
    /// Captures through shared memory from now on.
    fn use_shm(&mut self) {}
    /// Whether frames go into the images from `use_dmabuf`. A backend may
    /// go back to shm on its own (the compositor filled none of them).
    fn uses_dmabuf(&self) -> bool {
        false
    }
}

/// Why a backend is being opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenReason {
    /// The capture the user started (the app starting, a settings change,
    /// RESTART ENGINE): the portal may show its dialog.
    Start,
    /// An automatic recovery: a re-open after a failed generation, or the
    /// first open of an engine the app restarted on its own
    /// (`EngineConfig::automatic_start`). The portal may only restore its
    /// saved session, never show its dialog.
    Recovery,
}

/// Creates backends.
pub trait BackendFactory: Send + Sync {
    /// First-time selection order.
    fn candidates(&self) -> Vec<BackendKind>;
    /// Opens one backend kind with no fallback.
    fn create(
        &self,
        kind: BackendKind,
        cfg: &CaptureConfig,
        strict_output: bool,
        reason: OpenReason,
        cancel: Arc<AtomicBool>,
    ) -> Result<Box<dyn CaptureBackend>, BackendFailure>;
}

/// The production factory over the Wayland and portal crates.
pub struct RealFactory {
    has_wayland: bool,
    paths: Option<fthrust_core::FthrPaths>,
}

impl RealFactory {
    /// Detects Wayland from `WAYLAND_DISPLAY`.
    pub fn new(paths: Option<fthrust_core::FthrPaths>) -> Self {
        let has_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
        Self { has_wayland, paths }
    }

    /// Forces the Wayland answer (tests).
    pub fn assume_wayland(has_wayland: bool) -> Self {
        Self {
            has_wayland,
            paths: None,
        }
    }

    /// Uses `paths` for the portal's restore token (tests).
    pub fn with_paths(self, paths: fthrust_core::FthrPaths) -> Self {
        Self {
            paths: Some(paths),
            ..self
        }
    }
}

impl BackendFactory for RealFactory {
    fn candidates(&self) -> Vec<BackendKind> {
        crate::policy::backend_candidates(self.has_wayland)
    }

    fn create(
        &self,
        kind: BackendKind,
        cfg: &CaptureConfig,
        strict_output: bool,
        reason: OpenReason,
        cancel: Arc<AtomicBool>,
    ) -> Result<Box<dyn CaptureBackend>, BackendFailure> {
        match kind {
            BackendKind::WlrScreencopy => {
                let b = fthrust_wayland::screencopy::WlrScreencopy::open(
                    &cfg.monitor,
                    strict_output,
                    cancel,
                )?;
                Ok(Box::new(WlrAdapter(b)))
            }
            BackendKind::ExtImageCopy => {
                let b = fthrust_wayland::ext_capture::ExtImageCopy::open(
                    &cfg.monitor,
                    strict_output,
                    cfg.fps,
                    cancel,
                )?;
                Ok(Box::new(ExtAdapter(b)))
            }
            BackendKind::PortalScreenCast => {
                crate::portal_backend::open(self.paths.as_ref(), cfg, reason, cancel)
            }
            BackendKind::Synthetic => {
                if std::env::var("FTHRUST_SYNTHETIC_BACKEND").as_deref() == Ok("1") {
                    let script = Arc::new(Mutex::new(SyntheticScript::from_env()));
                    SyntheticFactory {
                        script,
                        candidates: vec![BackendKind::Synthetic],
                    }
                    .create(kind, cfg, strict_output, reason, cancel)
                } else {
                    Err(BackendFailure {
                        kind: FailureKind::Error,
                        reason: "the synthetic backend needs FTHRUST_SYNTHETIC_BACKEND=1".into(),
                        retry_pointless: true,
                        needs_dialog: false,
                    })
                }
            }
        }
    }
}

struct WlrAdapter(fthrust_wayland::screencopy::WlrScreencopy);

impl CaptureBackend for WlrAdapter {
    fn kind(&self) -> BackendKind {
        BackendKind::WlrScreencopy
    }
    fn capture(&mut self) -> Result<FrameView<'_>, CaptureError> {
        self.0.capture()
    }
    fn native_size(&self) -> (u32, u32) {
        self.0.native_size()
    }
    fn selected_output(&self) -> String {
        self.0.selected_output().to_string()
    }
}

struct ExtAdapter(fthrust_wayland::ext_capture::ExtImageCopy);

impl CaptureBackend for ExtAdapter {
    fn kind(&self) -> BackendKind {
        BackendKind::ExtImageCopy
    }
    fn capture(&mut self) -> Result<FrameView<'_>, CaptureError> {
        self.0.capture()
    }
    fn native_size(&self) -> (u32, u32) {
        self.0.native_size()
    }
    fn selected_output(&self) -> String {
        self.0.selected_output().to_string()
    }
    fn last_frame_available(&self) -> bool {
        self.0.last_frame_available()
    }
    fn buffer_size(&self) -> Option<(u32, u32)> {
        self.0.buffer_size()
    }
    fn dmabuf_constraints(&self) -> Option<DmabufConstraints> {
        self.0.dmabuf_constraints()
    }
    fn use_dmabuf(&mut self, images: Vec<DmabufImage>) -> Result<(), CaptureError> {
        self.0.use_dmabuf(images)
    }
    fn use_shm(&mut self) {
        self.0.use_shm();
    }
    fn uses_dmabuf(&self) -> bool {
        self.0.uses_dmabuf()
    }
}

/// The grey level of fresh frame `index` when `SyntheticScript::solid` is set.
pub fn solid_grey(index: u64) -> u8 {
    (30 + index * 53 % 190) as u8
}

/// Per-kind behaviour of the synthetic factory.
#[derive(Clone, Debug, Default)]
pub struct KindScript {
    /// Failures to report on successive `create` calls before succeeding.
    pub fail_inits: VecDeque<FailureKind>,
    /// After this many frames in a generation, return this error.
    pub capture_error_after: Option<(u64, CaptureError)>,
    /// Outputs the backend "sees" (empty = one unnamed output).
    pub outputs: Vec<String>,
    /// Outputs for successive opens, consumed one entry per `create`; when
    /// exhausted, `outputs` applies. Lets a test change what the second open
    /// sees without racing the engine.
    pub outputs_sequence: VecDeque<Vec<String>>,
}

/// What the synthetic backend does.
#[derive(Clone, Debug)]
pub struct SyntheticScript {
    /// Frame width.
    pub width: u32,
    /// Frame height.
    pub height: u32,
    /// Time between frames.
    pub frame_interval: Duration,
    /// Per-kind scripts; kinds absent here fail to open.
    pub kinds: HashMap<BackendKind, KindScript>,
    /// Deliver frames whose stride is too small (encode failures).
    pub short_stride: bool,
    /// Deliver black frames.
    pub black: bool,
    /// Output transform the frames arrive with (the buffer is in output
    /// orientation; `native_size` reports the upright size).
    pub transform: OutputTransform,
    /// After this many frames the screen goes static: the last frame is
    /// delivered again, marked `repeated`.
    pub static_after: Option<u64>,
    /// Fill each fresh frame with one grey level that differs per frame,
    /// so a test can tell which frame a decoded picture came from.
    pub solid: bool,
    /// Stamp each fresh frame this long before it is returned, as PipeWire
    /// stamps portal frames with the time they were produced.
    pub timestamp_lag: Duration,
    /// After `.0` frames, report [`CaptureError::Unchanged`] `.1` times, a
    /// frame interval apart, then deliver again: a static screen right
    /// after a mode switch, with no frame of the new size to repeat.
    pub unchanged: Option<(u64, u64)>,
    /// Offer dmabuf capture with these constraints. In dmabuf mode frames
    /// arrive as `FrameSource::Dmabuf` views with no pixel data.
    pub dmabuf: Option<DmabufConstraints>,
    /// In dmabuf mode, report `DmabufInvalidated` once after this many frames.
    pub dmabuf_invalidate_after: Option<u64>,
    /// In dmabuf mode, complete no capture into the images: the shm frame
    /// repeats until
    /// [`DMABUF_FIRST_FRAME_DEADLINE`](fthrust_wayland::backend::DMABUF_FIRST_FRAME_DEADLINE)
    /// runs out, then the backend goes back to shm on its own, as
    /// ext-image-copy does.
    ///
    /// Otherwise dmabuf mode behaves like ext-image-copy after its switch
    /// to a fresh session: the first dmabuf frame comes even when the
    /// screen is static (`static_after`), and later ones repeat it.
    pub dmabuf_never_filled: bool,
    /// `use_dmabuf` calls that succeeded (tests read it).
    pub dmabuf_uses: u64,
    /// `capture` calls so far, each one a frame the compositor would copy
    /// (tests read it).
    pub captures: u64,
    /// Backends opened so far (for the portal: sessions started; tests
    /// read it).
    pub opens: u64,
    /// Every `create` call with the reason it gave, opened or not (tests
    /// read it).
    pub open_reasons: Vec<(BackendKind, OpenReason)>,
}

impl SyntheticScript {
    /// A moving gradient at 60 fps.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            frame_interval: Duration::from_millis(16),
            kinds: HashMap::new(),
            short_stride: false,
            black: false,
            transform: OutputTransform::Normal,
            static_after: None,
            solid: false,
            timestamp_lag: Duration::ZERO,
            unchanged: None,
            dmabuf: None,
            dmabuf_invalidate_after: None,
            dmabuf_never_filled: false,
            dmabuf_uses: 0,
            captures: 0,
            opens: 0,
            open_reasons: Vec::new(),
        }
    }

    /// For `FTHRUST_SYNTHETIC_BACKEND=1`: `FTHRUST_SYNTHETIC_SIZE=WxH`
    /// (default 640x360) on the `Synthetic` kind.
    pub fn from_env() -> Self {
        let (w, h) = std::env::var("FTHRUST_SYNTHETIC_SIZE")
            .ok()
            .and_then(|s| {
                let (w, h) = s.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            })
            .unwrap_or((640, 360));
        let mut s = Self::new(w, h);
        s.kinds
            .insert(BackendKind::Synthetic, KindScript::default());
        s
    }
}

/// Creates synthetic backends per a shared, mutable script.
pub struct SyntheticFactory {
    /// The script (tests mutate it mid-run).
    pub script: Arc<Mutex<SyntheticScript>>,
    /// First-time selection order.
    pub candidates: Vec<BackendKind>,
}

impl BackendFactory for SyntheticFactory {
    fn candidates(&self) -> Vec<BackendKind> {
        self.candidates.clone()
    }

    fn create(
        &self,
        kind: BackendKind,
        cfg: &CaptureConfig,
        strict_output: bool,
        reason: OpenReason,
        cancel: Arc<AtomicBool>,
    ) -> Result<Box<dyn CaptureBackend>, BackendFailure> {
        let mut script = self.script.lock().unwrap_or_else(|p| p.into_inner());
        script.open_reasons.push((kind, reason));
        let (width, height) = (script.width, script.height);
        let Some(k) = script.kinds.get_mut(&kind) else {
            return Err(BackendFailure {
                kind: FailureKind::Error,
                reason: format!("{} is not available in this script", kind.name()),
                retry_pointless: false,
                needs_dialog: false,
            });
        };
        if let Some(f) = k.fail_inits.pop_front() {
            return Err(BackendFailure {
                kind: f,
                reason: format!("scripted {f:?} on {}", kind.name()),
                retry_pointless: false,
                needs_dialog: false,
            });
        }
        let outputs = k
            .outputs_sequence
            .pop_front()
            .unwrap_or_else(|| k.outputs.clone());
        let selected = if cfg.monitor.is_empty() {
            outputs.first().cloned().unwrap_or_default()
        } else if outputs.contains(&cfg.monitor) {
            cfg.monitor.clone()
        } else if strict_output {
            return Err(BackendFailure {
                kind: FailureKind::OutputAbsent,
                reason: format!("output {} is not advertised right now", cfg.monitor),
                retry_pointless: false,
                needs_dialog: false,
            });
        } else {
            outputs.first().cloned().unwrap_or_default()
        };
        script.opens += 1;
        Ok(Box::new(SyntheticBackend {
            kind,
            script: self.script.clone(),
            cancel,
            width,
            height,
            selected,
            frames: 0,
            buffer: Vec::new(),
            last_ts: 0,
            next_at: Instant::now(),
            dmabuf_images: 0,
            dmabuf_next: 0,
            dmabuf_last: None,
            dmabuf_invalidated: false,
            first_dmabuf: FirstDmabufFrame::default(),
            unchanged_reported: 0,
        }))
    }
}

struct SyntheticBackend {
    kind: BackendKind,
    script: Arc<Mutex<SyntheticScript>>,
    cancel: Arc<AtomicBool>,
    width: u32,
    height: u32,
    selected: String,
    frames: u64,
    buffer: Vec<u8>,
    last_ts: i64,
    next_at: Instant,
    /// Images handed over by `use_dmabuf`; 0 in shm mode.
    dmabuf_images: usize,
    dmabuf_next: usize,
    /// The image of the last dmabuf frame of this dmabuf mode.
    dmabuf_last: Option<usize>,
    dmabuf_invalidated: bool,
    first_dmabuf: FirstDmabufFrame,
    unchanged_reported: u64,
}

impl CaptureBackend for SyntheticBackend {
    fn kind(&self) -> BackendKind {
        self.kind
    }

    fn capture(&mut self) -> Result<FrameView<'_>, CaptureError> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(CaptureError::Cancelled);
        }
        let (interval, short_stride, black, error, transform) = {
            let mut s = self.script.lock().unwrap_or_else(|p| p.into_inner());
            s.captures += 1;
            let k = s.kinds.get(&self.kind);
            (
                s.frame_interval,
                s.short_stride,
                s.black,
                k.and_then(|k| k.capture_error_after.clone()),
                s.transform,
            )
        };
        if let Some((after, err)) = error
            && self.frames >= after
        {
            return Err(err);
        }
        let now = Instant::now();
        if now < self.next_at {
            std::thread::sleep(self.next_at - now);
        }
        self.next_at = Instant::now() + interval;
        let (solid, is_static, lag, unchanged) = {
            let s = self.script.lock().unwrap_or_else(|p| p.into_inner());
            (
                s.solid,
                s.static_after.is_some_and(|n| self.frames >= n),
                s.timestamp_lag,
                s.unchanged,
            )
        };
        if let Some((after, count)) = unchanged
            && self.frames >= after
            && self.unchanged_reported < count
        {
            self.unchanged_reported += 1;
            return Err(CaptureError::Unchanged);
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let stride = if short_stride { w * 4 - 4 } else { w * 4 };
        self.buffer.resize(stride * h, 0);
        if !short_stride && !is_static {
            let shift = (self.frames % 256) as u8;
            let grey = solid_grey(self.frames);
            for y in 0..h {
                for x in 0..w {
                    let i = y * stride + x * 4;
                    if solid {
                        self.buffer[i..i + 4].copy_from_slice(&[grey, grey, grey, 255]);
                    } else if black {
                        self.buffer[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
                    } else {
                        self.buffer[i] = (x as u8).wrapping_add(shift);
                        self.buffer[i + 1] = (y as u8).wrapping_add(shift);
                        self.buffer[i + 2] = ((x ^ y) as u8).wrapping_add(shift);
                        self.buffer[i + 3] = 255;
                    }
                }
            }
        }
        self.frames += 1;
        let lag = if is_static { 0 } else { lag.as_nanos() as i64 };
        let ts = (monotonic_ns() - lag).max(self.last_ts + 1);
        self.last_ts = ts;
        let mut unfilled_repeat = false;
        if self.dmabuf_images > 0 {
            let (invalidate_after, never_filled) = {
                let s = self.script.lock().unwrap_or_else(|p| p.into_inner());
                (s.dmabuf_invalidate_after, s.dmabuf_never_filled)
            };
            if !self.dmabuf_invalidated && invalidate_after.is_some_and(|n| self.frames > n) {
                self.dmabuf_invalidated = true;
                self.dmabuf_images = 0;
                return Err(CaptureError::DmabufInvalidated);
            }
            if never_filled {
                if self.first_dmabuf.overdue(Instant::now()) {
                    // As ext-image-copy does: no frame came into the images,
                    // so capture goes back to shm.
                    self.dmabuf_images = 0;
                    self.first_dmabuf.clear();
                } else {
                    // The dmabuf request is held: the shm frame repeats.
                    unfilled_repeat = true;
                }
            }
        }
        if self.dmabuf_images > 0 && !unfilled_repeat {
            // The switch to dmabuf started a fresh session, whose first
            // capture comes even on a static screen; later ones repeat it.
            let (index, repeated) = match self.dmabuf_last {
                Some(last) if is_static => (last, true),
                _ => {
                    let index = self.dmabuf_next;
                    self.dmabuf_next = (index + 1) % self.dmabuf_images;
                    (index, false)
                }
            };
            self.dmabuf_last = Some(index);
            self.first_dmabuf.clear();
            return Ok(FrameView {
                data: &[],
                stride: self.width as usize * 4,
                width: self.width,
                height: self.height,
                layout: PixelLayout::Bgrx,
                y_invert: false,
                transform,
                timestamp_ns: ts,
                repeated,
                source: FrameSource::Dmabuf(index),
            });
        }
        Ok(FrameView {
            data: &self.buffer,
            stride,
            width: self.width,
            height: self.height,
            layout: PixelLayout::Bgra,
            y_invert: false,
            transform,
            timestamp_ns: ts,
            repeated: is_static || unfilled_repeat,
            source: FrameSource::Shm,
        })
    }

    fn uses_dmabuf(&self) -> bool {
        self.dmabuf_images > 0
    }

    fn native_size(&self) -> (u32, u32) {
        let t = self
            .script
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .transform;
        if t.swaps_axes() {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    fn selected_output(&self) -> String {
        self.selected.clone()
    }

    fn buffer_size(&self) -> Option<(u32, u32)> {
        Some((self.width, self.height))
    }

    fn dmabuf_constraints(&self) -> Option<DmabufConstraints> {
        self.script
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .dmabuf
            .clone()
    }

    fn use_dmabuf(&mut self, images: Vec<DmabufImage>) -> Result<(), CaptureError> {
        let mut s = self.script.lock().unwrap_or_else(|p| p.into_inner());
        let fits = images
            .iter()
            .all(|i| (i.width, i.height) == (self.width, self.height));
        if s.dmabuf.is_none() || images.is_empty() || !fits {
            self.dmabuf_images = 0;
            return Err(CaptureError::Failed(
                "synthetic dmabuf images do not fit".into(),
            ));
        }
        s.dmabuf_uses += 1;
        self.dmabuf_images = images.len();
        self.dmabuf_next = 0;
        self.dmabuf_last = None;
        self.first_dmabuf.arm(Instant::now());
        Ok(())
    }

    fn use_shm(&mut self) {
        self.dmabuf_images = 0;
        self.dmabuf_last = None;
        self.first_dmabuf.clear();
    }
}
