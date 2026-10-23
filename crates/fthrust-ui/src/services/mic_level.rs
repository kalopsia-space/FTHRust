//! The microphone level (`MicRecorder.latest_level`): a small PipeWire
//! capture stream on the selected microphone feeding the Audio page meter
//! and Gary mode. The engine records the microphone for clips; this
//! stream only measures it.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::spa::param::audio::AudioFormat;

/// A level older than this reads as silence (a stalled device).
pub const STALE: Duration = Duration::from_millis(350);
/// Peak hold decay per block.
const PEAK_DECAY: f32 = 0.02;

/// Level and peak of the newest block.
#[derive(Clone, Copy, Debug, Default)]
pub struct Meter {
    level: f32,
    peak: f32,
    at: Option<Instant>,
}

impl Meter {
    /// One block of interleaved samples: the level is `clamp(rms × 4)` of
    /// channel 0 after `gain`.
    pub fn feed(&mut self, samples: &[f32], channels: usize, gain: f32, now: Instant) {
        let channels = channels.max(1);
        let frames = samples.len() / channels;
        if frames == 0 {
            return;
        }
        let sum: f32 = (0..frames)
            .map(|i| {
                let v = samples[i * channels] * gain;
                v * v
            })
            .sum();
        let rms = (sum / frames as f32).sqrt();
        self.level = (rms * 4.0).clamp(0.0, 1.0);
        self.peak = self.level.max(self.peak - PEAK_DECAY);
        self.at = Some(now);
    }

    /// `(level, peak)`, zero when stale.
    pub fn read(&self, now: Instant) -> (f32, f32) {
        match self.at {
            Some(at) if now.saturating_duration_since(at) <= STALE => (self.level, self.peak),
            _ => (0.0, 0.0),
        }
    }
}

/// Whether the stream must restart: only for another device (the gain is
/// applied live).
pub fn needs_restart(current: Option<&Option<String>>, target: &Option<String>) -> bool {
    current != Some(target)
}

/// The capture stream's meter, shared with readers.
pub type SharedMeter = Arc<Mutex<Meter>>;

/// A running level stream.
pub struct MicLevel {
    meter: SharedMeter,
    quit: Option<pw::channel::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    gain: Arc<std::sync::atomic::AtomicU32>,
    /// The device it measures.
    pub target: Option<String>,
}

impl MicLevel {
    /// Starts measuring `target` (a PipeWire node name; `None` is the
    /// default source) with `gain`.
    pub fn start(target: Option<String>, gain: f32) -> Result<MicLevel, String> {
        let meter: SharedMeter = Arc::default();
        let (quit_tx, quit_rx) = pw::channel::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let m = meter.clone();
        let t = target.clone();
        let gain = Arc::new(std::sync::atomic::AtomicU32::new(gain.to_bits()));
        let g = gain.clone();
        let thread = std::thread::Builder::new()
            .name("fthr-mic-level".into())
            .spawn(move || run(t, g, m, quit_rx, ready_tx))
            .map_err(|e| e.to_string())?;
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(MicLevel {
                meter,
                quit: Some(quit_tx),
                thread: Some(thread),
                gain,
                target,
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("PipeWire did not answer".into()),
        }
    }

    /// Changes the gain without restarting the stream.
    pub fn set_gain(&self, gain: f32) {
        self.gain
            .store(gain.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    /// The shared meter.
    pub fn meter(&self) -> SharedMeter {
        self.meter.clone()
    }

    /// `(level, peak)` now.
    pub fn read(&self) -> (f32, f32) {
        self.meter
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .read(Instant::now())
    }
}

impl Drop for MicLevel {
    fn drop(&mut self) {
        if let Some(q) = self.quit.take() {
            let _ = q.send(());
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
            AudioFormat::F32LE
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::AudioRate,
            Int,
            48_000
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::AudioChannels,
            Int,
            2
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
    target: Option<String>,
    gain: Arc<std::sync::atomic::AtomicU32>,
    meter: SharedMeter,
    quit: pw::channel::Receiver<()>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let fail = |e: String| {
        let _ = ready.send(Err(e));
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
    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::NODE_NAME => "FTHRust mic level",
    };
    if let Some(t) = &target {
        props.insert("target.object", t.as_str());
    }
    let stream = match pw::stream::StreamRc::new(core.clone(), "fthrust-mic-level", props) {
        Ok(s) => s,
        Err(e) => return fail(format!("mic level stream: {e}")),
    };
    let listener = stream
        .add_local_listener_with_user_data(())
        .process(move |stream, _| {
            while let Some(mut buffer) = stream.dequeue_buffer() {
                let datas = buffer.datas_mut();
                let Some(data) = datas.first_mut() else {
                    continue;
                };
                let chunk = data.chunk();
                let (offset, size) = (chunk.offset() as usize, chunk.size() as usize);
                let Some(bytes) = data.data() else { continue };
                let end = offset.saturating_add(size).min(bytes.len());
                if offset >= end {
                    continue;
                }
                let samples: Vec<f32> = bytes[offset..end]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect();
                meter.lock().unwrap_or_else(|e| e.into_inner()).feed(
                    &samples,
                    2,
                    f32::from_bits(gain.load(std::sync::atomic::Ordering::Relaxed)),
                    Instant::now(),
                );
            }
        })
        .register();
    let _listener = match listener {
        Ok(l) => l,
        Err(e) => return fail(format!("mic level listener: {e}")),
    };
    let pod = match format_pod() {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let Some(pod) = pw::spa::pod::Pod::from_bytes(&pod) else {
        return fail("malformed audio pod".into());
    };
    let mut params = [pod];
    if let Err(e) = stream.connect(
        pw::spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    ) {
        return fail(format!("mic level connect: {e}"));
    }
    let _quit = quit.attach(main_loop.loop_(), {
        let main_loop = main_loop.clone();
        move |_| main_loop.quit()
    });
    let _ = ready.send(Ok(()));
    main_loop.run();
}
