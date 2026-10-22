//! The capture engine: backend generations with recovery, encoding into
//! the replay ring, health and status. Ports `capture_engine.cpp`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use fthrust_core::capture_config::Scaling;
use fthrust_core::capture_health::flags;
use fthrust_ipc::{
    AudioStatus, BackendKind, EngineConfig, RecordingState, SourceStatus, StatusSnapshot,
};
use fthrust_media::encoder::{Encoder, EncoderConfig};
use fthrust_media::ring::{EncodedPacket, EncodedRing};
use fthrust_wayland::frame::{BgraFrame, normalize};

use crate::audio::{AudioConfig, AudioProvider, AudioTarget};
use crate::backend::{BackendFactory, BackendFailure, CaptureError, FailureKind};
use crate::policy::{
    CaptureRecoveryPolicy, EncodeFailurePolicy, next_encode_deadline, recovery_order,
    wait_for_recovery_backoff,
};
use crate::watchdog::ContentWatchdog;

/// Capture could not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupError {
    /// Machine-readable code (`WAYLAND_UNAVAILABLE`, ...).
    pub code: &'static str,
    /// Human-readable detail.
    pub detail: String,
}

/// A startup condition that does not stop capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupNotice {
    /// Machine-readable code (`DESKTOP_AUDIO_UNAVAILABLE`, ...).
    pub code: String,
    /// Human-readable detail.
    pub detail: String,
}

/// Something the daemon should push to the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineNotice {
    /// Any status field changed.
    StatusChanged,
    /// A generation ended and recovery is under way.
    Recovering {
        /// Why.
        reason: String,
    },
    /// Capture failed for good.
    Failed {
        /// Why.
        reason: String,
    },
    /// A generation started.
    GenerationStarted {
        /// Its number.
        capture_generation: u32,
        /// Its backend.
        backend: BackendKind,
    },
}

/// What the save and record workers need from the open encoder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderSnapshot {
    /// Encoder generation stamped on its packets.
    pub generation: u64,
    /// Codec of the packets.
    pub codec_id: ffmpeg_next::codec::Id,
    /// Global header.
    pub extradata: Vec<u8>,
    /// Output width.
    pub width: u32,
    /// Output height.
    pub height: u32,
    /// Frame rate.
    pub fps: u32,
    /// Bitrate.
    pub bitrate_kbps: u32,
}

/// Injected collaborators.
pub struct EngineDeps {
    /// Backend factory.
    pub backends: Arc<dyn BackendFactory>,
    /// Audio capture.
    pub audio: Arc<dyn AudioProvider>,
    /// Waits out a recovery backoff while `running` holds; returns whether
    /// it completed (production: [`wait_for_recovery_backoff`] with a real
    /// sleep; tests record the duration and return at once).
    pub backoff: BackoffHook,
}

/// See [`EngineDeps::backoff`].
pub type BackoffHook = Arc<dyn Fn(Duration, &dyn Fn() -> bool) -> bool + Send + Sync>;

/// The production backoff: 50 ms slices of real sleep.
pub fn real_backoff() -> BackoffHook {
    Arc::new(|d, running| wait_for_recovery_backoff(d, running, std::thread::sleep))
}

#[derive(Default)]
struct Status {
    frames_captured: u64,
    encoder_open: bool,
    active_codec: String,
    health_flags: u32,
    capture_generation: u32,
    encoder_generation: u32,
    content_sample_sequence: u32,
    content_suspicious_streak: u32,
    content_luma_mean: f32,
    content_luma_variance: f32,
    capture_failure_detail: String,
    recovery_reason: String,
    backend: Option<BackendKind>,
    selected_output: String,
    native_width: u32,
    native_height: u32,
}

struct Inner {
    running: AtomicBool,
    paused: AtomicBool,
    cancel: Arc<AtomicBool>,
    status: Mutex<Status>,
    ring: Mutex<Arc<EncodedRing>>,
    encoder_snapshot: Mutex<Option<EncoderSnapshot>>,
    notices: Sender<EngineNotice>,
    backends: Arc<dyn BackendFactory>,
    backoff: BackoffHook,
    packet_tap: Mutex<Option<Sender<EncodedPacket>>>,
    first_outcome: Mutex<Option<std::sync::mpsc::Sender<FirstOutcome>>>,
    config: Mutex<EngineConfig>,
    audio: Arc<dyn AudioProvider>,
    keyframe_request: AtomicBool,
}

impl Inner {
    fn notify(&self, n: EngineNotice) {
        let _ = self.notices.try_send(n);
    }

    fn update(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap_or_else(|p| p.into_inner()));
        self.notify(EngineNotice::StatusChanged);
    }
}

/// Result of the first generation attempt, reported to `start`.
enum FirstOutcome {
    Started,
    Failed(StartupError),
}

/// The engine.
pub struct CaptureEngine {
    config: EngineConfig,
    inner: Arc<Inner>,
    audio: Arc<dyn AudioProvider>,
    notices_rx: Receiver<EngineNotice>,
    thread: Option<std::thread::JoinHandle<()>>,
    started: bool,
}

/// Longest `start` waits for the first generation outcome.
pub const START_DEADLINE: Duration = Duration::from_secs(15);

impl CaptureEngine {
    /// Builds an engine; nothing runs until `start`.
    pub fn new(config: EngineConfig, deps: EngineDeps) -> Self {
        let (config, _) = config.normalized();
        let (tx, rx) = crossbeam_channel::bounded(256);
        let ring = make_ring(&config);
        let inner = Arc::new(Inner {
            running: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancel: Arc::new(AtomicBool::new(false)),
            status: Mutex::new(Status::default()),
            ring: Mutex::new(ring),
            encoder_snapshot: Mutex::new(None),
            notices: tx,
            backends: deps.backends,
            backoff: deps.backoff,
            packet_tap: Mutex::new(None),
            first_outcome: Mutex::new(None),
            config: Mutex::new(config.clone()),
            audio: deps.audio.clone(),
            keyframe_request: AtomicBool::new(false),
        });
        Self {
            config,
            inner,
            audio: deps.audio,
            notices_rx: rx,
            thread: None,
            started: false,
        }
    }

    /// The normalised configuration.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Starts audio and the capture thread, waiting (bounded) for the
    /// first generation to open an encoder.
    pub fn start(&mut self) -> Result<Vec<StartupNotice>, StartupError> {
        if self.inner.backends.candidates().is_empty()
            && self.config.backend_override != Some(BackendKind::Synthetic)
        {
            let err = StartupError {
                code: "WAYLAND_UNAVAILABLE",
                detail: "WAYLAND_DISPLAY is not set; FTHRust captures Wayland sessions only".into(),
            };
            self.inner.update(|s| {
                s.health_flags = flags::BACKEND_FAILED;
                s.capture_failure_detail = err.detail.clone();
            });
            eprintln!("FTHR_STARTUP_ERROR: {}: {}", err.code, err.detail);
            return Err(err);
        }
        let mut notices = Vec::new();
        if self.config.capture.audio_enabled
            || !self.config.capture.microphone_endpoint_id.is_empty()
        {
            let cfg = AudioConfig {
                system: self.config.capture.audio_enabled.then(|| {
                    if self.config.capture.playback_endpoint_id.is_empty() {
                        AudioTarget::DefaultSinkMonitor
                    } else {
                        AudioTarget::SinkMonitor(self.config.capture.playback_endpoint_id.clone())
                    }
                }),
                microphone: (!self.config.capture.microphone_endpoint_id.is_empty()).then(|| {
                    AudioTarget::Source(self.config.capture.microphone_endpoint_id.clone())
                }),
                keep_seconds: self.config.capture.buffer_seconds,
            };
            for (code, detail) in self.audio.start(&cfg) {
                eprintln!("FTHR_STARTUP_WARNING: {code}: {detail}");
                notices.push(StartupNotice { code, detail });
            }
        }
        self.spawn_capture_thread();
        self.started = true;
        match self.first_outcome() {
            FirstOutcome::Started => Ok(notices),
            FirstOutcome::Failed(e) => {
                eprintln!("FTHR_STARTUP_ERROR: {}: {}", e.code, e.detail);
                Err(e)
            }
        }
    }

    fn first_outcome(&mut self) -> FirstOutcome {
        let (tx, rx) = std::sync::mpsc::channel();
        *self
            .inner
            .first_outcome
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(tx);
        match rx.recv_timeout(START_DEADLINE) {
            Ok(o) => o,
            Err(_) => FirstOutcome::Started,
        }
    }

    fn spawn_capture_thread(&mut self) {
        self.inner.running.store(true, Ordering::SeqCst);
        self.inner.cancel.store(false, Ordering::SeqCst);
        let inner = self.inner.clone();
        let config = self.config.clone();
        self.thread = Some(
            std::thread::Builder::new()
                .name("fthrust-capture".into())
                .spawn(move || run_capture(inner, config))
                .expect("spawn capture thread"),
        );
    }

    /// Current status (audio and save fields are filled by the daemon).
    pub fn status(&self) -> StatusSnapshot {
        let s = self.inner.status.lock().unwrap_or_else(|p| p.into_inner());
        StatusSnapshot {
            frames_captured: s.frames_captured,
            bytes_written: 0,
            encoder_open: s.encoder_open,
            active_codec: s.active_codec.clone(),
            health_flags: s.health_flags,
            capture_generation: s.capture_generation,
            encoder_generation: s.encoder_generation,
            content_sample_sequence: s.content_sample_sequence,
            content_suspicious_streak: s.content_suspicious_streak,
            content_luma_mean: s.content_luma_mean,
            content_luma_variance: s.content_luma_variance,
            capture_failure_detail: s.capture_failure_detail.clone(),
            recovery_reason: s.recovery_reason.clone(),
            paused: self.inner.paused.load(Ordering::Relaxed),
            recording: RecordingState::Idle,
            backend: s.backend,
            selected_output: s.selected_output.clone(),
            native_width: s.native_width,
            native_height: s.native_height,
            audio: if self.started {
                self.audio.status()
            } else {
                AudioStatus {
                    system: SourceStatus::Disabled,
                    microphone: SourceStatus::Disabled,
                }
            },
            last_save: None,
        }
    }

    /// Notices, in order (one receiver; clone for more).
    pub fn events(&self) -> Receiver<EngineNotice> {
        self.notices_rx.clone()
    }

    /// Focus pause: stop encoding, keep the ring.
    pub fn set_paused(&self, paused: bool) {
        self.inner.paused.store(paused, Ordering::SeqCst);
        self.inner.notify(EngineNotice::StatusChanged);
    }

    /// Whether the capture thread is alive.
    pub fn is_capturing(&self) -> bool {
        self.inner.running.load(Ordering::Relaxed)
    }

    /// The encoded ring.
    pub fn ring(&self) -> Arc<EncodedRing> {
        self.inner
            .ring
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The open encoder's parameters, if one is open.
    pub fn encoder_snapshot(&self) -> Option<EncoderSnapshot> {
        self.inner
            .encoder_snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The audio provider.
    pub fn audio(&self) -> Arc<dyn AudioProvider> {
        self.audio.clone()
    }

    /// Installs a sink that receives every encoded packet (recording).
    pub fn set_packet_tap(&self, tap: Option<Sender<EncodedPacket>>) {
        *self
            .inner
            .packet_tap
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = tap;
    }

    /// A cloneable handle for the save and record workers.
    pub fn handle(&self) -> EngineHandle {
        EngineHandle {
            inner: self.inner.clone(),
        }
    }

    /// Restarts capture with a new configuration (audio keeps running).
    pub fn reconfigure(&mut self, config: EngineConfig) -> Result<Vec<String>, StartupError> {
        let (config, warnings) = config.normalized();
        self.stop_capture();
        self.config = config;
        *self.inner.config.lock().unwrap_or_else(|p| p.into_inner()) = self.config.clone();
        *self.inner.ring.lock().unwrap_or_else(|p| p.into_inner()) = make_ring(&self.config);
        *self
            .inner
            .encoder_snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        self.inner.update(|s| {
            *s = Status {
                capture_generation: s.capture_generation,
                ..Status::default()
            }
        });
        self.spawn_capture_thread();
        match self.first_outcome() {
            FirstOutcome::Started => Ok(warnings),
            FirstOutcome::Failed(e) => Err(e),
        }
    }

    fn stop_capture(&mut self) {
        self.inner.running.store(false, Ordering::SeqCst);
        self.inner.cancel.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    /// Stops capture and audio.
    pub fn shutdown(&mut self) {
        self.stop_capture();
        if self.started {
            self.audio.stop();
            self.started = false;
        }
    }
}

/// Read access to the engine's state from worker threads.
#[derive(Clone)]
pub struct EngineHandle {
    inner: Arc<Inner>,
}

impl EngineHandle {
    /// Current capture status (no save fields).
    pub fn status(&self) -> StatusSnapshot {
        let s = self.inner.status.lock().unwrap_or_else(|p| p.into_inner());
        StatusSnapshot {
            frames_captured: s.frames_captured,
            bytes_written: 0,
            encoder_open: s.encoder_open,
            active_codec: s.active_codec.clone(),
            health_flags: s.health_flags,
            capture_generation: s.capture_generation,
            encoder_generation: s.encoder_generation,
            content_sample_sequence: s.content_sample_sequence,
            content_suspicious_streak: s.content_suspicious_streak,
            content_luma_mean: s.content_luma_mean,
            content_luma_variance: s.content_luma_variance,
            capture_failure_detail: s.capture_failure_detail.clone(),
            recovery_reason: s.recovery_reason.clone(),
            paused: self.inner.paused.load(Ordering::Relaxed),
            recording: RecordingState::Idle,
            backend: s.backend,
            selected_output: s.selected_output.clone(),
            native_width: s.native_width,
            native_height: s.native_height,
            audio: self.inner.audio.status(),
            last_save: None,
        }
    }

    /// Whether the capture thread is alive.
    pub fn is_capturing(&self) -> bool {
        self.inner.running.load(Ordering::Relaxed)
    }

    /// The encoded ring.
    pub fn ring(&self) -> Arc<EncodedRing> {
        self.inner
            .ring
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The open encoder's parameters.
    pub fn encoder_snapshot(&self) -> Option<EncoderSnapshot> {
        self.inner
            .encoder_snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The audio provider.
    pub fn audio(&self) -> Arc<dyn AudioProvider> {
        self.inner.audio.clone()
    }

    /// The normalised configuration.
    pub fn config(&self) -> EngineConfig {
        self.inner
            .config
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Installs or removes the packet tap.
    pub fn set_packet_tap(&self, tap: Option<Sender<EncodedPacket>>) {
        if tap.is_some() {
            self.inner.keyframe_request.store(true, Ordering::SeqCst);
        }
        *self
            .inner
            .packet_tap
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = tap;
    }
}

impl Drop for CaptureEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn report_first(inner: &Inner, outcome: FirstOutcome) {
    if let Some(tx) = inner
        .first_outcome
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
    {
        let _ = tx.send(outcome);
    }
}

fn make_ring(config: &EngineConfig) -> Arc<EncodedRing> {
    let c = &config.capture;
    Arc::new(EncodedRing::new(
        (u64::from(c.buffer_seconds) + 5) * 1000,
        c.fps,
        u64::from(config.max_buffer_mb) * 1024 * 1024,
    ))
}

/// Encode size: target or native, `Fit` shrinks to fit keeping aspect,
/// both dimensions masked even (`capture_engine.cpp:206-223`).
pub fn encode_size(native: (u32, u32), target: (u32, u32), scaling: Scaling) -> (u32, u32) {
    let (nw, nh) = (native.0.max(2), native.1.max(2));
    let (mut w, mut h) = if target.0 == 0 || target.1 == 0 {
        (nw, nh)
    } else if scaling == Scaling::Fit {
        let scale = (f64::from(target.0) / f64::from(nw)).min(f64::from(target.1) / f64::from(nh));
        if scale < 1.0 {
            (
                (f64::from(nw) * scale) as u32,
                (f64::from(nh) * scale) as u32,
            )
        } else {
            (nw, nh)
        }
    } else {
        target
    };
    w &= !1;
    h &= !1;
    (w.max(2), h.max(2))
}

enum GenerationEnd {
    Stop,
    BackendFailed { transient: bool, reason: String },
    EncoderFailed(String),
}

fn run_capture(inner: Arc<Inner>, config: EngineConfig) {
    let cfg = &config.capture;
    let fps = cfg.fps;
    let candidates: Vec<BackendKind> = match config.backend_override {
        Some(k) => vec![k],
        None => inner.backends.candidates(),
    };
    let mut order = candidates.clone();
    let mut recovery = CaptureRecoveryPolicy::new(u64::from(fps) * 10, 3);
    let mut encoder: Option<Encoder> = None;
    let mut encoder_geometry: (u32, u32) = (0, 0);
    let mut strict_output = false;
    let mut ever_produced = false;
    let mut first_reported = false;
    let mut bgra = BgraFrame::default();

    'generations: while inner.running.load(Ordering::Relaxed) {
        // Backend selection along `order`.
        let mut backend = None;
        let mut failures: Vec<BackendFailure> = Vec::new();
        for kind in &order {
            if !inner.running.load(Ordering::Relaxed) {
                break 'generations;
            }
            match inner
                .backends
                .create(*kind, cfg, strict_output, inner.cancel.clone())
            {
                Ok(b) => {
                    backend = Some(b);
                    break;
                }
                Err(f) if f.kind == FailureKind::Cancelled => break 'generations,
                Err(f) if f.retry_pointless => {
                    let reason = f.reason.clone();
                    terminal_failure(&inner, &reason);
                    if !first_reported {
                        first_reported = true;
                        report_first(
                            &inner,
                            FirstOutcome::Failed(StartupError {
                                code: "CAPTURE_DECLINED",
                                detail: reason,
                            }),
                        );
                    }
                    let _ = first_reported;
                    break 'generations;
                }
                Err(f) => failures.push(f),
            }
        }
        let Some(mut backend) = backend else {
            // A transient condition on any candidate (the compositor is not
            // answering, the selected output is absent) is waited out
            // without consuming the budget; only genuine errors everywhere
            // count toward it.
            let transient = failures.iter().any(|f| {
                matches!(
                    f.kind,
                    FailureKind::CompositorUnreachable | FailureKind::OutputAbsent
                )
            });
            let reason = failures
                .iter()
                .map(|f| f.reason.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            if !handle_failure(
                &inner,
                &mut recovery,
                &mut order,
                &candidates,
                0,
                transient,
                &reason,
                &mut first_reported,
                &mut ever_produced,
            ) {
                break 'generations;
            }
            continue;
        };

        // Generation start.
        let kind = backend.kind();
        let native = backend.native_size();
        let selected = backend.selected_output();
        let (ew, eh) = encode_size(native, (cfg.width, cfg.height), cfg.scaling);
        if encoder
            .as_ref()
            .is_none_or(|_| encoder_geometry != (ew, eh))
        {
            if encoder.take().is_some() {
                inner.ring.lock().unwrap_or_else(|p| p.into_inner()).clear();
            }
            *inner
                .encoder_snapshot
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = None;
            let enc_cfg = EncoderConfig {
                src_width: native.0,
                src_height: native.1,
                enc_width: ew,
                enc_height: eh,
                fps,
                bitrate_kbps: cfg.bitrate_kbps,
                codec: cfg.codec,
                encoder: cfg.encoder,
                preset: cfg.preset,
            };
            match Encoder::open(&enc_cfg) {
                Ok(mut enc) => {
                    let generation = {
                        let mut s = inner.status.lock().unwrap_or_else(|p| p.into_inner());
                        s.encoder_generation += 1;
                        u64::from(s.encoder_generation)
                    };
                    enc.set_generation(generation);
                    *inner
                        .encoder_snapshot
                        .lock()
                        .unwrap_or_else(|p| p.into_inner()) = Some(EncoderSnapshot {
                        generation,
                        codec_id: enc.codec_id(),
                        extradata: enc.extradata(),
                        width: ew,
                        height: eh,
                        fps,
                        bitrate_kbps: cfg.bitrate_kbps,
                    });
                    encoder_geometry = (ew, eh);
                    encoder = Some(enc);
                }
                Err(e) => {
                    let reason = format!("no usable encoder: {e}");
                    terminal_failure(&inner, &reason);
                    if !first_reported {
                        first_reported = true;
                        report_first(
                            &inner,
                            FirstOutcome::Failed(StartupError {
                                code: "ENCODER_UNAVAILABLE",
                                detail: reason,
                            }),
                        );
                    }
                    let _ = first_reported;
                    break 'generations;
                }
            }
        }
        let enc = encoder.as_mut().expect("opened above");
        let active_codec = enc.active_codec().to_string();
        let generation_number = {
            let mut s = inner.status.lock().unwrap_or_else(|p| p.into_inner());
            s.capture_generation += 1;
            s.backend = Some(kind);
            s.selected_output = selected.clone();
            s.native_width = native.0;
            s.native_height = native.1;
            s.encoder_open = true;
            s.active_codec = active_codec;
            s.health_flags = flags::ACTIVE | (s.health_flags & flags::CONTENT_SUSPECT);
            s.recovery_reason.clear();
            s.capture_failure_detail.clear();
            s.capture_generation
        };
        inner.notify(EngineNotice::GenerationStarted {
            capture_generation: generation_number,
            backend: kind,
        });
        inner.notify(EngineNotice::StatusChanged);
        if !first_reported {
            first_reported = true;
            report_first(&inner, FirstOutcome::Started);
        }

        // Frame loop.
        let frame_ns = 1_000_000_000i64 / i64::from(fps.max(1));
        let mut next_encode_ns = 0i64;
        let mut produced: u64 = 0;
        let mut encode_failures = EncodeFailurePolicy::new(fps);
        let mut watchdog = ContentWatchdog::new(fps);
        let mut packets = Vec::new();
        let end = loop {
            if !inner.running.load(Ordering::Relaxed) {
                break GenerationEnd::Stop;
            }
            if inner.paused.load(Ordering::Relaxed) {
                {
                    let mut s = inner.status.lock().unwrap_or_else(|p| p.into_inner());
                    if s.health_flags & flags::PAUSED == 0 {
                        s.health_flags |= flags::PAUSED;
                        drop(s);
                        inner.notify(EngineNotice::StatusChanged);
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
                continue;
            } else {
                let mut s = inner.status.lock().unwrap_or_else(|p| p.into_inner());
                if s.health_flags & flags::PAUSED != 0 {
                    s.health_flags &= !flags::PAUSED;
                    drop(s);
                    inner.notify(EngineNotice::StatusChanged);
                }
            }
            let view = match backend.capture() {
                Ok(v) => v,
                Err(CaptureError::Cancelled) => break GenerationEnd::Stop,
                Err(CaptureError::Disconnected) => {
                    break GenerationEnd::BackendFailed {
                        transient: true,
                        reason: "the Wayland connection was lost".into(),
                    };
                }
                Err(e) => {
                    break GenerationEnd::BackendFailed {
                        transient: false,
                        reason: e.to_string(),
                    };
                }
            };
            let ts = view.timestamp_ns;
            if ts < next_encode_ns {
                continue;
            }
            next_encode_ns = next_encode_deadline(next_encode_ns, frame_ns, ts) + frame_ns;
            let encoded = match normalize(&view, &mut bgra) {
                Ok(()) => {
                    if let Some(m) = watchdog.observe(&bgra) {
                        inner.update(|s| {
                            s.content_sample_sequence = m.sequence;
                            s.content_suspicious_streak = m.streak;
                            s.content_luma_mean = m.luma_mean;
                            s.content_luma_variance = m.luma_variance;
                            if m.suspect {
                                s.health_flags |= flags::CONTENT_SUSPECT;
                            } else {
                                s.health_flags &= !flags::CONTENT_SUSPECT;
                            }
                        });
                    }
                    packets.clear();
                    if inner.keyframe_request.swap(false, Ordering::SeqCst) {
                        enc.request_keyframe();
                    }
                    enc.encode(
                        &bgra.data,
                        bgra.stride,
                        bgra.width,
                        bgra.height,
                        ts,
                        &mut packets,
                    )
                    .map_err(|e| e.to_string())
                }
                Err(e) => Err(e.to_string()),
            };
            match encoded {
                Ok(()) => {
                    encode_failures.on_encode_result(true);
                    produced += 1;
                    ever_produced = true;
                    let ring = inner.ring.lock().unwrap_or_else(|p| p.into_inner()).clone();
                    let tap = inner
                        .packet_tap
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .clone();
                    for p in packets.drain(..) {
                        if let Some(tap) = &tap {
                            let _ = tap.try_send(p.clone());
                        }
                        ring.push(p);
                    }
                    let mut s = inner.status.lock().unwrap_or_else(|p| p.into_inner());
                    s.frames_captured += 1;
                    if s.frames_captured.is_multiple_of(u64::from(fps.max(1)))
                        || s.frames_captured == 1
                    {
                        drop(s);
                        inner.notify(EngineNotice::StatusChanged);
                    }
                }
                Err(e) => {
                    if encode_failures.on_encode_result(false) {
                        break GenerationEnd::EncoderFailed(e);
                    }
                }
            }
        };

        // Generation end.
        drop(backend);
        match end {
            GenerationEnd::Stop => break 'generations,
            GenerationEnd::EncoderFailed(reason) => {
                tracing::warn!("encoder failed ({reason}); reopening");
                encoder = None;
                *inner
                    .encoder_snapshot
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = None;
                inner.ring.lock().unwrap_or_else(|p| p.into_inner()).clear();
                inner.update(|s| {
                    s.encoder_open = false;
                    s.active_codec.clear();
                });
                if produced > 0 {
                    strict_output = !cfg.monitor.is_empty() && selected == cfg.monitor;
                    order = recovery_order(kind, &candidates);
                }
                if !handle_failure(
                    &inner,
                    &mut recovery,
                    &mut order,
                    &candidates,
                    produced,
                    false,
                    &format!("encoder failed: {reason}"),
                    &mut first_reported,
                    &mut ever_produced,
                ) {
                    break 'generations;
                }
            }
            GenerationEnd::BackendFailed { transient, reason } => {
                if produced > 0 {
                    strict_output = !cfg.monitor.is_empty() && selected == cfg.monitor;
                    order = recovery_order(kind, &candidates);
                }
                if !handle_failure(
                    &inner,
                    &mut recovery,
                    &mut order,
                    &candidates,
                    produced,
                    transient,
                    &reason,
                    &mut first_reported,
                    &mut ever_produced,
                ) {
                    break 'generations;
                }
            }
        }
    }
    inner.running.store(false, Ordering::SeqCst);
    let mut s = inner.status.lock().unwrap_or_else(|p| p.into_inner());
    s.encoder_open = false;
    if s.health_flags & flags::BACKEND_FAILED == 0 {
        s.health_flags = 0;
    }
    drop(s);
    inner.notify(EngineNotice::StatusChanged);
}

fn terminal_failure(inner: &Inner, reason: &str) {
    inner.update(|s| {
        s.health_flags = flags::BACKEND_FAILED;
        s.capture_failure_detail = reason.to_string();
        s.recovery_reason.clear();
        s.encoder_open = false;
    });
    inner.notify(EngineNotice::Failed {
        reason: reason.to_string(),
    });
}

/// Applies the recovery policy after a failed generation; returns whether
/// to keep going. The budget belongs to the first backend in `order`; when
/// it is spent the next candidate gets a fresh budget (the fall-through
/// fix), and only an empty queue is terminal.
#[allow(clippy::too_many_arguments)]
fn handle_failure(
    inner: &Inner,
    recovery: &mut CaptureRecoveryPolicy,
    order: &mut Vec<BackendKind>,
    candidates: &[BackendKind],
    produced: u64,
    transient: bool,
    reason: &str,
    first_reported: &mut bool,
    ever_produced: &mut bool,
) -> bool {
    let decision = recovery.on_generation_failed(produced, transient);
    if decision.exhausted {
        if order.len() > 1 {
            let spent = order.remove(0);
            tracing::warn!(
                backend = spent.name(),
                "retry budget spent; falling through to the next backend"
            );
            *recovery = CaptureRecoveryPolicy::new(recovery_reset_frames(inner), 3);
            let next = recovery.on_generation_failed(0, false);
            return wait_recovering(
                inner,
                next.backoff,
                &format!("{reason} (trying {})", order[0].name()),
            );
        }
        let detail = if *ever_produced || candidates.is_empty() {
            reason.to_string()
        } else {
            format!("No Wayland capture path is available ({reason})")
        };
        terminal_failure(inner, &detail);
        if !*first_reported {
            *first_reported = true;
            report_first(
                inner,
                FirstOutcome::Failed(StartupError {
                    code: "CAPTURE_BACKEND_UNAVAILABLE",
                    detail,
                }),
            );
        }
        return false;
    }
    wait_recovering(inner, decision.backoff, reason)
}

fn recovery_reset_frames(inner: &Inner) -> u64 {
    let _ = inner;
    600
}

fn wait_recovering(inner: &Inner, backoff: Duration, reason: &str) -> bool {
    inner.update(|s| {
        s.health_flags = flags::RECOVERING | (s.health_flags & flags::CONTENT_SUSPECT);
        s.recovery_reason = reason.to_string();
        s.encoder_open = s.encoder_open && true;
    });
    inner.notify(EngineNotice::Recovering {
        reason: reason.to_string(),
    });
    let running = || inner.running.load(Ordering::Relaxed);
    (inner.backoff)(backoff, &running)
}
