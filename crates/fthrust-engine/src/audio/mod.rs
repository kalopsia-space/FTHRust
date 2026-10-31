//! Audio capture behind a trait so the engine runs without an audio
//! server. The PipeWire implementation lives in [`pipewire`]; the
//! synthetic one feeds a tone for tests.

pub mod pipewire;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fthrust_ipc::{AudioStatus, SourceStatus};
use fthrust_media::audio_timing::{
    AudioBlock, CHANNELS, SAMPLE_RATE, assemble_window_from, ring_capacity_samples,
};
use fthrust_wayland::backend::monotonic_ns;

/// Which capture source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioSourceKind {
    /// The desktop (sink monitor).
    System,
    /// The microphone.
    Microphone,
}

/// What to capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioTarget {
    /// Follow the default sink's monitor.
    DefaultSinkMonitor,
    /// A named sink's monitor.
    SinkMonitor(String),
    /// A named capture source (microphone).
    Source(String),
}

/// Audio configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioConfig {
    /// The desktop source, if enabled.
    pub system: Option<AudioTarget>,
    /// The microphone, if enabled.
    pub microphone: Option<AudioTarget>,
    /// How many seconds to keep (the replay length).
    pub keep_seconds: u32,
}

/// Nanoseconds covered by `block`.
fn block_ns(block: &AudioBlock) -> i64 {
    (block.samples.len() / CHANNELS) as i64 * 1_000_000_000 / i64::from(SAMPLE_RATE)
}

/// A bounded store of timestamped float32 blocks.
pub struct AudioRing {
    blocks: VecDeque<Arc<AudioBlock>>,
    samples: usize,
    capacity_samples: usize,
}

/// The blocks of one window, shared with the ring they came from.
pub struct AudioWindow {
    blocks: Vec<Arc<AudioBlock>>,
    end_ns: i64,
    duration_ms: u32,
}

impl AudioWindow {
    /// The window's interleaved PCM (silence-padded; empty when no block
    /// overlaps it).
    pub fn pcm(&self) -> Vec<f32> {
        assemble_window_from(
            &self.blocks,
            CHANNELS,
            SAMPLE_RATE,
            self.end_ns,
            self.duration_ms,
        )
    }
}

impl AudioRing {
    /// Capacity for `keep_seconds` (the reference's `(keep + 10) s`).
    pub fn new(keep_seconds: u32) -> Self {
        Self {
            blocks: VecDeque::new(),
            samples: 0,
            capacity_samples: ring_capacity_samples(keep_seconds, SAMPLE_RATE, CHANNELS),
        }
    }

    /// Appends a block, pruning the oldest ones past capacity.
    pub fn push(&mut self, block: AudioBlock) {
        self.samples += block.samples.len();
        self.blocks.push_back(Arc::new(block));
        while self.samples > self.capacity_samples {
            match self.blocks.pop_front() {
                Some(old) => self.samples -= old.samples.len(),
                None => break,
            }
        }
    }

    /// The blocks that can overlap the `duration_ms` window ending at
    /// `end_ns`, shared rather than copied: cheap enough to take under the
    /// lock the capture callback needs, with [`AudioWindow::pcm`] doing the
    /// copying once that lock is released.
    pub fn window_blocks(&self, end_ns: i64, duration_ms: u32) -> AudioWindow {
        let start_ns = end_ns - i64::from(duration_ms) * 1_000_000 - 100_000_000;
        let blocks = self
            .blocks
            .iter()
            .filter(|b| b.start_ns + block_ns(b) >= start_ns && b.start_ns <= end_ns)
            .cloned()
            .collect();
        AudioWindow {
            blocks,
            end_ns,
            duration_ms,
        }
    }

    /// The `duration_ms` window ending at `end_ns` (silence-padded).
    pub fn window(&self, end_ns: i64, duration_ms: u32) -> Vec<f32> {
        self.window_blocks(end_ns, duration_ms).pcm()
    }

    /// Capture time of the end of the newest block, `None` when empty.
    pub fn captured_until(&self) -> Option<i64> {
        self.blocks.back().map(|b| b.start_ns + block_ns(b))
    }

    /// Blocks held.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Drops everything.
    pub fn clear(&mut self) {
        self.blocks.clear();
        self.samples = 0;
    }
}

/// An audio capture implementation.
pub trait AudioProvider: Send + Sync {
    /// Starts capturing; returns `(code, detail)` startup warnings.
    fn start(&self, cfg: &AudioConfig) -> Vec<(String, String)>;
    /// The `duration_ms` window of `kind` ending at `end_ns`, or `None`
    /// when that source is not capturing.
    fn window(&self, kind: AudioSourceKind, end_ns: i64, duration_ms: u32) -> Option<Vec<f32>>;
    /// Source states.
    fn status(&self) -> AudioStatus;
    /// Stops capturing (bounded).
    fn stop(&self);
    /// Capture time up to which `kind` has delivered audio (the end of its
    /// newest block), or `None` when it is not capturing or cannot say.
    /// Saves and recordings wait on it, bounded, so they do not end on
    /// silence the source simply has not delivered yet.
    fn captured_until(&self, kind: AudioSourceKind) -> Option<i64> {
        let _ = kind;
        None
    }
}

/// A 440 Hz tone at amplitude 0.3 on both sources, produced in 10 ms
/// blocks by a thread.
pub struct SyntheticAudio {
    inner: Arc<Mutex<SyntheticInner>>,
}

struct SyntheticInner {
    system: Option<AudioRing>,
    microphone: Option<AudioRing>,
    running: bool,
    thread: Option<std::thread::JoinHandle<()>>,
    amplitude: f32,
}

impl Default for SyntheticAudio {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntheticAudio {
    /// Creates a silent provider; `start` begins the tone.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SyntheticInner {
                system: None,
                microphone: None,
                running: false,
                thread: None,
                amplitude: 0.3,
            })),
        }
    }

    /// A provider that reports the desktop source as unavailable.
    pub fn unavailable() -> Self {
        let this = Self::new();
        this.inner.lock().unwrap().amplitude = f32::NAN;
        this
    }
}

impl AudioProvider for SyntheticAudio {
    fn start(&self, cfg: &AudioConfig) -> Vec<(String, String)> {
        let mut inner = self.inner.lock().unwrap();
        if inner.amplitude.is_nan() {
            return vec![(
                "DESKTOP_AUDIO_UNAVAILABLE".into(),
                "synthetic audio provider configured as unavailable".into(),
            )];
        }
        inner.system = cfg
            .system
            .as_ref()
            .map(|_| AudioRing::new(cfg.keep_seconds));
        inner.microphone = cfg
            .microphone
            .as_ref()
            .map(|_| AudioRing::new(cfg.keep_seconds));
        inner.running = true;
        let shared = self.inner.clone();
        let amplitude = inner.amplitude;
        inner.thread = Some(std::thread::spawn(move || {
            let mut phase = 0u64;
            let frames = 480usize;
            loop {
                let start_ns = monotonic_ns();
                let samples: Vec<f32> = (0..frames)
                    .flat_map(|i| {
                        let t = (phase + i as u64) as f32 / SAMPLE_RATE as f32;
                        let v = (t * 440.0 * std::f32::consts::TAU).sin() * amplitude;
                        [v, v]
                    })
                    .collect();
                phase += frames as u64;
                {
                    let mut inner = shared.lock().unwrap();
                    if !inner.running {
                        break;
                    }
                    let SyntheticInner {
                        system, microphone, ..
                    } = &mut *inner;
                    for ring in [system, microphone].into_iter().flatten() {
                        ring.push(AudioBlock {
                            start_ns,
                            samples: samples.clone(),
                        });
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }));
        Vec::new()
    }

    fn window(&self, kind: AudioSourceKind, end_ns: i64, duration_ms: u32) -> Option<Vec<f32>> {
        let blocks = {
            let inner = self.inner.lock().unwrap();
            let ring = match kind {
                AudioSourceKind::System => inner.system.as_ref(),
                AudioSourceKind::Microphone => inner.microphone.as_ref(),
            }?;
            ring.window_blocks(end_ns, duration_ms)
        };
        Some(blocks.pcm())
    }

    fn captured_until(&self, kind: AudioSourceKind) -> Option<i64> {
        let inner = self.inner.lock().unwrap();
        match kind {
            AudioSourceKind::System => inner.system.as_ref(),
            AudioSourceKind::Microphone => inner.microphone.as_ref(),
        }?
        .captured_until()
    }

    fn status(&self) -> AudioStatus {
        let inner = self.inner.lock().unwrap();
        let st = |r: &Option<AudioRing>| {
            if r.is_some() {
                SourceStatus::Streaming
            } else {
                SourceStatus::Disabled
            }
        };
        AudioStatus {
            system: st(&inner.system),
            microphone: st(&inner.microphone),
        }
    }

    fn stop(&self) {
        let thread = {
            let mut inner = self.inner.lock().unwrap();
            inner.running = false;
            inner.system = None;
            inner.microphone = None;
            inner.thread.take()
        };
        if let Some(t) = thread {
            let _ = t.join();
        }
    }
}
