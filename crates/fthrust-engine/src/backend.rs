//! The capture backend abstraction the engine drives, adapters over the
//! Wayland and portal crates, and a synthetic backend for tests.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fthrust_core::capture_config::CaptureConfig;
use fthrust_ipc::BackendKind;
use fthrust_wayland::backend::{self as wb, monotonic_ns};
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
}

impl From<wb::BackendError> for BackendFailure {
    fn from(e: wb::BackendError) -> Self {
        BackendFailure {
            kind: e.kind,
            reason: e.reason,
            retry_pointless: false,
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
                crate::portal_backend::open(self.paths.as_ref(), cfg, cancel)
            }
            BackendKind::Synthetic => {
                if std::env::var("FTHRUST_SYNTHETIC_BACKEND").as_deref() == Ok("1") {
                    let script = Arc::new(Mutex::new(SyntheticScript::from_env()));
                    SyntheticFactory {
                        script,
                        candidates: vec![BackendKind::Synthetic],
                    }
                    .create(kind, cfg, strict_output, cancel)
                } else {
                    Err(BackendFailure {
                        kind: FailureKind::Error,
                        reason: "the synthetic backend needs FTHRUST_SYNTHETIC_BACKEND=1".into(),
                        retry_pointless: true,
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
        cancel: Arc<AtomicBool>,
    ) -> Result<Box<dyn CaptureBackend>, BackendFailure> {
        let mut script = self.script.lock().unwrap_or_else(|p| p.into_inner());
        let (width, height) = (script.width, script.height);
        let Some(k) = script.kinds.get_mut(&kind) else {
            return Err(BackendFailure {
                kind: FailureKind::Error,
                reason: format!("{} is not available in this script", kind.name()),
                retry_pointless: false,
            });
        };
        if let Some(f) = k.fail_inits.pop_front() {
            return Err(BackendFailure {
                kind: f,
                reason: format!("scripted {f:?} on {}", kind.name()),
                retry_pointless: false,
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
            });
        } else {
            outputs.first().cloned().unwrap_or_default()
        };
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
            let s = self.script.lock().unwrap_or_else(|p| p.into_inner());
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
        let solid = self.script.lock().unwrap_or_else(|p| p.into_inner()).solid;
        let is_static = self
            .script
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .static_after
            .is_some_and(|n| self.frames >= n);
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
        let ts = monotonic_ns().max(self.last_ts + 1);
        self.last_ts = ts;
        Ok(FrameView {
            data: &self.buffer,
            stride,
            width: self.width,
            height: self.height,
            layout: PixelLayout::Bgra,
            y_invert: false,
            transform,
            timestamp_ns: ts,
            repeated: is_static,
            source: FrameSource::Shm,
        })
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
}
