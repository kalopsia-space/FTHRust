//! Where the mixed PCM goes: a bounded queue the audio worker fills and an
//! output drains. The PipeWire output is the master clock; the null output
//! drains at a test-controlled clock.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use fthrust_media::playback::{CANONICAL_CHANNELS, CANONICAL_SAMPLE_RATE, MAX_QUEUED_FRAMES};

/// Bounded interleaved-stereo PCM (`MAX_QUEUED_FRAMES` frames).
pub struct PcmQueue {
    samples: Mutex<VecDeque<f32>>,
    capacity_frames: usize,
    /// Frames the output has taken since the last `clear`.
    played_frames: AtomicUsize,
}

impl Default for PcmQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl PcmQueue {
    /// An empty queue.
    pub fn new() -> Self {
        PcmQueue {
            samples: Mutex::new(VecDeque::new()),
            capacity_frames: MAX_QUEUED_FRAMES,
            played_frames: AtomicUsize::new(0),
        }
    }

    /// Frames waiting.
    pub fn frames(&self) -> usize {
        self.samples
            .lock()
            .map(|s| s.len() / CANONICAL_CHANNELS)
            .unwrap_or(0)
    }

    /// Room for one more block of `frames`.
    pub fn has_room(&self, frames: usize) -> bool {
        self.frames() + frames <= self.capacity_frames
    }

    /// Appends a block; `false` (and nothing appended) when it does not fit.
    pub fn push(&self, block: &[f32]) -> bool {
        let Ok(mut s) = self.samples.lock() else {
            return false;
        };
        if s.len() / CANONICAL_CHANNELS + block.len() / CANONICAL_CHANNELS > self.capacity_frames {
            return false;
        }
        s.extend(block.iter().copied());
        true
    }

    /// Takes `frames` frames, zero-filled when short; returns how many were real.
    pub fn read(&self, frames: usize, out: &mut [f32]) -> usize {
        let Ok(mut s) = self.samples.lock() else {
            out.fill(0.0);
            return 0;
        };
        let want = frames * CANONICAL_CHANNELS;
        let have = s.len().min(want);
        for (i, v) in s.drain(..have).enumerate() {
            out[i] = v;
        }
        out[have..want].fill(0.0);
        let real = have / CANONICAL_CHANNELS;
        self.played_frames.fetch_add(real, Ordering::Relaxed);
        real
    }

    /// Discards everything.
    pub fn clear(&self) {
        if let Ok(mut s) = self.samples.lock() {
            s.clear();
        }
    }

    /// Frames taken by the output since the last clear (for tests).
    pub fn played_frames(&self) -> usize {
        self.played_frames.load(Ordering::Relaxed)
    }
}

/// An audio output drains a [`PcmQueue`].
pub trait AudioOutput: Send {
    /// Starts draining `queue`.
    fn start(&mut self, queue: Arc<PcmQueue>) -> Result<(), String>;
    /// Stops consuming (the sync rule's hold).
    fn suspend(&mut self);
    /// Resumes consuming.
    fn resume(&mut self);
    /// Frames queued plus frames the device holds, i.e. not yet heard.
    fn buffered_frames(&self) -> usize;
    /// Stops for good.
    fn stop(&mut self);
    /// A failure since the last call (the device vanished, the stream
    /// errored); the service reports it once and carries on without audio.
    fn take_failure(&mut self) -> Option<String> {
        None
    }
    /// The null output, when this is one (the service polls its drain).
    fn as_null_mut(&mut self) -> Option<&mut NullOutput> {
        None
    }
}

/// Test hooks on a [`NullOutput`]: `fail` makes the next poll report a
/// device failure; `suspended` mirrors the hold state.
#[derive(Clone, Default)]
pub struct NullSwitches {
    /// Set to make the output fail mid-play.
    pub fail: Arc<AtomicBool>,
    /// Whether the output is currently suspended.
    pub suspended: Arc<AtomicBool>,
}

/// Drains the queue at real time from a clock the test controls (ms).
pub struct NullOutput {
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    last_ms: u64,
    queue: Option<Arc<PcmQueue>>,
    suspended: bool,
    fail_start: bool,
    switches: NullSwitches,
    failed: bool,
    scratch: Vec<f32>,
}

impl NullOutput {
    /// A null output driven by `clock`.
    pub fn new(clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        NullOutput {
            last_ms: clock(),
            clock,
            queue: None,
            suspended: false,
            fail_start: false,
            switches: NullSwitches::default(),
            failed: false,
            scratch: Vec::new(),
        }
    }

    /// Makes `start` fail (the "device gone" case).
    pub fn failing(mut self) -> Self {
        self.fail_start = true;
        self
    }

    /// Shares the test switches.
    pub fn with_switches(mut self, switches: NullSwitches) -> Self {
        self.switches = switches;
        self
    }

    fn drain(&mut self) {
        let now = (self.clock)();
        let elapsed = now.saturating_sub(self.last_ms);
        self.last_ms = now;
        if self.suspended || self.failed || self.switches.fail.load(Ordering::Relaxed) {
            return;
        }
        let frames = (elapsed * u64::from(CANONICAL_SAMPLE_RATE) / 1000) as usize;
        if frames == 0 {
            return;
        }
        if let Some(q) = &self.queue {
            self.scratch.resize(frames * CANONICAL_CHANNELS, 0.0);
            q.read(frames, &mut self.scratch);
        }
    }
}

impl AudioOutput for NullOutput {
    fn start(&mut self, queue: Arc<PcmQueue>) -> Result<(), String> {
        if self.fail_start {
            return Err("audio device unavailable".into());
        }
        self.queue = Some(queue);
        self.last_ms = (self.clock)();
        Ok(())
    }

    fn suspend(&mut self) {
        self.drain();
        self.suspended = true;
        self.switches.suspended.store(true, Ordering::Relaxed);
    }

    fn resume(&mut self) {
        self.last_ms = (self.clock)();
        self.suspended = false;
        self.switches.suspended.store(false, Ordering::Relaxed);
    }

    fn take_failure(&mut self) -> Option<String> {
        if !self.failed && self.switches.fail.load(Ordering::Relaxed) {
            self.failed = true;
            return Some("audio device lost".into());
        }
        None
    }

    fn buffered_frames(&self) -> usize {
        // Interior drain through a const method is not possible; callers use
        // `poll` first. Report what is queued.
        self.queue.as_ref().map(|q| q.frames()).unwrap_or(0)
    }

    fn stop(&mut self) {
        self.queue = None;
    }

    fn as_null_mut(&mut self) -> Option<&mut NullOutput> {
        Some(self)
    }
}

impl NullOutput {
    /// Advances the drain to the clock (the service calls this each tick).
    pub fn poll(&mut self) {
        self.drain();
    }
}

/// The PipeWire playback stream: pulls from the queue in the process
/// callback and tracks the last quantum as the device-side buffer.
pub struct PipeWireOutput {
    inner: Option<pipewire_out::Stream>,
    queue: Option<Arc<PcmQueue>>,
}

impl Default for PipeWireOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl PipeWireOutput {
    /// Not yet started.
    pub fn new() -> Self {
        PipeWireOutput {
            inner: None,
            queue: None,
        }
    }
}

impl AudioOutput for PipeWireOutput {
    fn start(&mut self, queue: Arc<PcmQueue>) -> Result<(), String> {
        let stream = pipewire_out::Stream::start(queue.clone())?;
        self.inner = Some(stream);
        self.queue = Some(queue);
        Ok(())
    }

    fn suspend(&mut self) {
        if let Some(s) = &self.inner {
            s.set_active(false);
        }
    }

    fn resume(&mut self) {
        if let Some(s) = &self.inner {
            s.set_active(true);
        }
    }

    fn buffered_frames(&self) -> usize {
        let queued = self.queue.as_ref().map(|q| q.frames()).unwrap_or(0);
        let device = self.inner.as_ref().map(|s| s.last_quantum()).unwrap_or(0);
        queued + device
    }

    fn take_failure(&mut self) -> Option<String> {
        self.inner.as_ref().and_then(|s| s.take_failure())
    }

    fn stop(&mut self) {
        self.inner = None;
        self.queue = None;
    }
}

mod pipewire_out {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use pipewire as pw;

    use super::{AtomicBool, PcmQueue};
    use fthrust_media::playback::{CANONICAL_CHANNELS, CANONICAL_SAMPLE_RATE};

    enum Msg {
        Active(bool),
        Quit,
    }

    pub struct Stream {
        tx: Option<pw::channel::Sender<Msg>>,
        thread: Option<std::thread::JoinHandle<()>>,
        last_quantum: Arc<AtomicUsize>,
        failure: Arc<std::sync::Mutex<Option<String>>>,
    }

    impl Stream {
        pub fn start(queue: Arc<PcmQueue>) -> Result<Self, String> {
            let (tx, rx) = pw::channel::channel::<Msg>();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let last_quantum = Arc::new(AtomicUsize::new(0));
            let failure = Arc::new(std::sync::Mutex::new(None));
            let thread = {
                let lq = last_quantum.clone();
                let failure = failure.clone();
                std::thread::Builder::new()
                    .name("fthrust-playback-out".into())
                    .spawn(move || run(queue, lq, failure, rx, ready_tx))
                    .map_err(|e| e.to_string())?
            };
            match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(Ok(())) => Ok(Stream {
                    tx: Some(tx),
                    thread: Some(thread),
                    last_quantum,
                    failure,
                }),
                Ok(Err(e)) => Err(e),
                Err(_) => Err("PipeWire did not answer".into()),
            }
        }

        pub fn set_active(&self, active: bool) {
            if let Some(tx) = &self.tx {
                let _ = tx.send(Msg::Active(active));
            }
        }

        pub fn last_quantum(&self) -> usize {
            self.last_quantum.load(Ordering::Relaxed)
        }

        pub fn take_failure(&self) -> Option<String> {
            self.failure.lock().ok().and_then(|mut f| f.take())
        }
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            if let Some(tx) = self.tx.take() {
                let _ = tx.send(Msg::Quit);
            }
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn format_pod() -> Result<Vec<u8>, String> {
        let obj = pw::spa::pod::object!(
            pw::spa::utils::SpaTypes::ObjectParamFormat,
            pw::spa::param::ParamType::EnumFormat,
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::MediaType,
                Id,
                pw::spa::param::format::MediaType::Audio
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::MediaSubtype,
                Id,
                pw::spa::param::format::MediaSubtype::Raw
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::AudioFormat,
                Id,
                pw::spa::param::audio::AudioFormat::F32LE
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::AudioRate,
                Int,
                CANONICAL_SAMPLE_RATE as i32
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::AudioChannels,
                Int,
                CANONICAL_CHANNELS as i32
            ),
        );
        pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(obj),
        )
        .map(|(c, _)| c.into_inner())
        .map_err(|e| format!("audio format pod: {e:?}"))
    }

    fn run(
        queue: Arc<PcmQueue>,
        last_quantum: Arc<AtomicUsize>,
        failure: Arc<std::sync::Mutex<Option<String>>>,
        rx: pw::channel::Receiver<Msg>,
        ready: std::sync::mpsc::Sender<Result<(), String>>,
    ) {
        let fail = |msg: String| {
            let _ = ready.send(Err(msg));
        };
        let main_loop = match pw::main_loop::MainLoopRc::new(None) {
            Ok(l) => l,
            Err(e) => return fail(format!("main loop: {e}")),
        };
        let context = match pw::context::ContextRc::new(&main_loop, None) {
            Ok(c) => c,
            Err(e) => return fail(format!("context: {e}")),
        };
        let core = match context.connect_rc(None) {
            Ok(c) => c,
            Err(e) => return fail(format!("cannot connect to PipeWire: {e}")),
        };
        let props = pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "Movie",
            *pw::keys::NODE_NAME => "FTHRust playback",
            *pw::keys::APP_NAME => "FTHR Clips",
        };
        let stream = match pw::stream::StreamRc::new(core.clone(), "fthrust-playback", props) {
            Ok(s) => s,
            Err(e) => return fail(format!("stream: {e}")),
        };
        let active = Arc::new(AtomicBool::new(true));
        let listener = stream
            .add_local_listener_with_user_data(())
            .state_changed({
                let failure = failure.clone();
                move |_stream, _, _old, new| {
                    let msg = match new {
                        pw::stream::StreamState::Error(e) => {
                            Some(format!("PipeWire stream error: {e}"))
                        }
                        pw::stream::StreamState::Unconnected => {
                            Some("PipeWire stream disconnected".to_owned())
                        }
                        _ => None,
                    };
                    if let Some(m) = msg
                        && let Ok(mut f) = failure.lock()
                        && f.is_none()
                    {
                        *f = Some(m);
                    }
                }
            })
            .process({
                let queue = queue.clone();
                let last_quantum = last_quantum.clone();
                let active = active.clone();
                let mut scratch: Vec<f32> = Vec::new();
                move |stream, _| {
                    while let Some(mut buffer) = stream.dequeue_buffer() {
                        let requested = buffer.requested() as usize;
                        let datas = buffer.datas_mut();
                        let Some(data) = datas.first_mut() else {
                            continue;
                        };
                        let Some(bytes) = data.data() else {
                            continue;
                        };
                        let max_frames = bytes.len() / (4 * CANONICAL_CHANNELS);
                        let frames = if requested > 0 {
                            requested.min(max_frames)
                        } else {
                            max_frames.min(1024)
                        };
                        last_quantum.store(frames, Ordering::Relaxed);
                        scratch.resize(frames * CANONICAL_CHANNELS, 0.0);
                        if active.load(Ordering::Relaxed) {
                            queue.read(frames, &mut scratch);
                        } else {
                            scratch.fill(0.0);
                        }
                        for (i, s) in scratch.iter().enumerate() {
                            bytes[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
                        }
                        let chunk = data.chunk_mut();
                        *chunk.offset_mut() = 0;
                        *chunk.stride_mut() = (4 * CANONICAL_CHANNELS) as i32;
                        *chunk.size_mut() = (frames * CANONICAL_CHANNELS * 4) as u32;
                    }
                }
            })
            .register();
        let _listener = match listener {
            Ok(l) => l,
            Err(e) => return fail(format!("listener: {e}")),
        };
        let pod_bytes = match format_pod() {
            Ok(b) => b,
            Err(e) => return fail(e),
        };
        let Some(pod) = pw::spa::pod::Pod::from_bytes(&pod_bytes) else {
            return fail("malformed audio pod".into());
        };
        let mut params = [pod];
        if let Err(e) = stream.connect(
            pw::spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        ) {
            return fail(format!("stream connect: {e}"));
        }
        let _ = ready.send(Ok(()));
        let _attached = rx.attach(main_loop.loop_(), {
            let main_loop = main_loop.clone();
            let active = active.clone();
            move |msg| match msg {
                Msg::Active(a) => active.store(a, Ordering::Relaxed),
                Msg::Quit => main_loop.quit(),
            }
        });
        main_loop.run();
        let _ = stream.disconnect();
    }
}
