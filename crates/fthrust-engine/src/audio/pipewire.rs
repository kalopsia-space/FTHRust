//! PipeWire audio capture: one stream per source (the sink monitor via
//! `stream.capture.sink`, the microphone by `target.object`), 48 kHz
//! float32 stereo, block timestamps corrected by the stream latency, a 1 s
//! reconnect loop and a bounded stop. Ports `audio_capture.cpp` (which
//! used the PulseAudio simple API) onto libpipewire.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fthrust_ipc::{AudioStatus, SourceStatus};
use fthrust_media::audio_timing::{AudioBlock, CHANNELS, SAMPLE_RATE, chunk_start_ns};
use fthrust_wayland::backend::monotonic_ns;
use pipewire as pw;
use pw::spa::param::audio::AudioFormat;

use super::{AudioConfig, AudioProvider, AudioRing, AudioSourceKind, AudioTarget};

/// Retry interval after a stream error.
pub const RECONNECT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct SourceState {
    ring: Option<AudioRing>,
    status: SourceStatus,
    blocks: u64,
}

struct Shared {
    system: Mutex<SourceState>,
    microphone: Mutex<SourceState>,
}

impl Shared {
    fn source(&self, kind: AudioSourceKind) -> &Mutex<SourceState> {
        match kind {
            AudioSourceKind::System => &self.system,
            AudioSourceKind::Microphone => &self.microphone,
        }
    }
}

enum LoopMessage {
    Quit,
}

/// The PipeWire audio provider.
pub struct PipeWireAudio {
    shared: Arc<Shared>,
    control: Mutex<
        Option<(
            pw::channel::Sender<LoopMessage>,
            std::thread::JoinHandle<()>,
        )>,
    >,
}

impl Default for PipeWireAudio {
    fn default() -> Self {
        Self::new()
    }
}

impl PipeWireAudio {
    /// Creates an idle provider.
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                system: Mutex::new(SourceState::default()),
                microphone: Mutex::new(SourceState::default()),
            }),
            control: Mutex::new(None),
        }
    }

    /// Blocks received on `kind` so far.
    pub fn blocks_received(&self, kind: AudioSourceKind) -> u64 {
        self.shared
            .source(kind)
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .blocks
    }
}

impl AudioProvider for PipeWireAudio {
    fn start(&self, cfg: &AudioConfig) -> Vec<(String, String)> {
        if cfg.system.is_none() && cfg.microphone.is_none() {
            return Vec::new();
        }
        for (kind, target) in [
            (AudioSourceKind::System, &cfg.system),
            (AudioSourceKind::Microphone, &cfg.microphone),
        ] {
            let mut s = self
                .shared
                .source(kind)
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if target.is_some() {
                s.ring = Some(AudioRing::new(cfg.keep_seconds));
                s.status = SourceStatus::Connecting;
            } else {
                s.ring = None;
                s.status = SourceStatus::Disabled;
            }
        }
        crate::devices::init_pipewire();
        let (quit_tx, quit_rx) = pw::channel::channel::<LoopMessage>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let shared = self.shared.clone();
        let cfg = cfg.clone();
        let thread = match std::thread::Builder::new()
            .name("fthrust-audio-pw".into())
            .spawn(move || run_loop(cfg, shared, quit_rx, ready_tx))
        {
            Ok(t) => t,
            Err(e) => {
                return vec![(
                    "DESKTOP_AUDIO_UNAVAILABLE".into(),
                    format!("audio thread: {e}"),
                )];
            }
        };
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                *self.control.lock().unwrap_or_else(|p| p.into_inner()) = Some((quit_tx, thread));
                Vec::new()
            }
            Ok(Err(e)) => {
                let _ = thread.join();
                self.mark_all(SourceStatus::Disabled);
                vec![("DESKTOP_AUDIO_UNAVAILABLE".into(), e)]
            }
            Err(_) => {
                let _ = quit_tx.send(LoopMessage::Quit);
                let _ = thread.join();
                self.mark_all(SourceStatus::Disabled);
                vec![(
                    "DESKTOP_AUDIO_UNAVAILABLE".into(),
                    "PipeWire did not answer".into(),
                )]
            }
        }
    }

    fn window(&self, kind: AudioSourceKind, end_ns: i64, duration_ms: u32) -> Option<Vec<f32>> {
        let s = self
            .shared
            .source(kind)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        s.ring.as_ref().map(|r| r.window(end_ns, duration_ms))
    }

    fn status(&self) -> AudioStatus {
        AudioStatus {
            system: self
                .shared
                .system
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .status
                .clone(),
            microphone: self
                .shared
                .microphone
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .status
                .clone(),
        }
    }

    fn stop(&self) {
        let control = self
            .control
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some((quit, thread)) = control {
            let _ = quit.send(LoopMessage::Quit);
            // The loop quits at its next iteration; join is bounded by that.
            let _ = thread.join();
        }
        self.mark_all(SourceStatus::Disabled);
    }
}

impl PipeWireAudio {
    fn mark_all(&self, status: SourceStatus) {
        for kind in [AudioSourceKind::System, AudioSourceKind::Microphone] {
            let mut s = self
                .shared
                .source(kind)
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if s.ring.is_some() || status == SourceStatus::Disabled {
                s.status = status.clone();
            }
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
            SAMPLE_RATE as i32
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::AudioChannels,
            Int,
            CHANNELS as i32
        ),
    );
    pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .map(|(c, _)| c.into_inner())
    .map_err(|e| format!("audio format pod: {e:?}"))
}

struct StreamSlot {
    kind: AudioSourceKind,
    stream: pw::stream::StreamRc,
    _listener: pw::stream::StreamListener<()>,
    target: AudioTarget,
    needs_reconnect: bool,
}

fn connect_stream(slot: &StreamSlot, pod_bytes: &[u8]) -> Result<(), String> {
    let pod = pw::spa::pod::Pod::from_bytes(pod_bytes).ok_or("malformed audio pod")?;
    let mut params = [pod];
    slot.stream
        .connect(
            pw::spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(|e| format!("{:?} stream connect: {e}", slot.kind))?;
    let _ = &slot.target;
    Ok(())
}

fn run_loop(
    cfg: AudioConfig,
    shared: Arc<Shared>,
    quit_rx: pw::channel::Receiver<LoopMessage>,
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
    let pod_bytes = match format_pod() {
        Ok(b) => b,
        Err(e) => return fail(e),
    };
    let mut slots: Vec<StreamSlot> = Vec::new();
    for (kind, target) in [
        (AudioSourceKind::System, cfg.system.clone()),
        (AudioSourceKind::Microphone, cfg.microphone.clone()),
    ] {
        let Some(target) = target else { continue };
        let mut props = pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::NODE_NAME => "FTHRust capture",
        };
        match &target {
            AudioTarget::DefaultSinkMonitor => {
                props.insert("stream.capture.sink", "true");
            }
            AudioTarget::SinkMonitor(name) => {
                props.insert("stream.capture.sink", "true");
                props.insert("target.object", name.as_str());
            }
            AudioTarget::Source(name) => {
                props.insert("target.object", name.as_str());
            }
        }
        let stream = match pw::stream::StreamRc::new(core.clone(), "fthrust-audio", props) {
            Ok(s) => s,
            Err(e) => return fail(format!("{kind:?} stream: {e}")),
        };
        let listener = stream
            .add_local_listener_with_user_data(())
            .state_changed({
                let shared = shared.clone();
                move |_, _, _old, new| {
                    let mut s = shared
                        .source(kind)
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    s.status = match new {
                        pw::stream::StreamState::Streaming => SourceStatus::Streaming,
                        pw::stream::StreamState::Error(e) => {
                            SourceStatus::Reconnecting { reason: e }
                        }
                        pw::stream::StreamState::Unconnected => {
                            if matches!(s.status, SourceStatus::Reconnecting { .. }) {
                                s.status.clone()
                            } else {
                                SourceStatus::Connecting
                            }
                        }
                        _ => SourceStatus::Connecting,
                    };
                }
            })
            .process({
                let shared = shared.clone();
                move |stream, _| {
                    let latency_us = stream
                        .time()
                        .ok()
                        .map(|t| {
                            let rate = t.rate();
                            if rate.denom == 0 {
                                0
                            } else {
                                ((t.delay() as i128 + t.queued() as i128)
                                    * 1_000_000
                                    * i128::from(rate.num)
                                    / i128::from(rate.denom)) as i64
                            }
                        })
                        .unwrap_or(0);
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
                        let frames = samples.len() / CHANNELS;
                        if frames == 0 {
                            continue;
                        }
                        let delivery_end = monotonic_ns();
                        let start_ns =
                            chunk_start_ns(delivery_end, latency_us, frames, SAMPLE_RATE);
                        let mut s = shared
                            .source(kind)
                            .lock()
                            .unwrap_or_else(|p| p.into_inner());
                        if let Some(ring) = s.ring.as_mut() {
                            ring.push(AudioBlock { start_ns, samples });
                        }
                        s.blocks += 1;
                    }
                }
            })
            .register();
        let listener = match listener {
            Ok(l) => l,
            Err(e) => return fail(format!("{kind:?} listener: {e}")),
        };
        slots.push(StreamSlot {
            kind,
            stream,
            _listener: listener,
            target,
            needs_reconnect: false,
        });
    }
    for slot in &slots {
        if let Err(e) = connect_stream(slot, &pod_bytes) {
            return fail(e);
        }
    }
    let _ = ready.send(Ok(()));

    // Reconnect loop: once per second, re-connect any stream in error.
    let slots = std::rc::Rc::new(std::cell::RefCell::new(slots));
    let timer = main_loop.loop_().add_timer({
        let slots = slots.clone();
        let shared = shared.clone();
        let pod_bytes = pod_bytes.clone();
        move |_| {
            for slot in slots.borrow_mut().iter_mut() {
                let errored = matches!(
                    shared
                        .source(slot.kind)
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .status,
                    SourceStatus::Reconnecting { .. }
                );
                if errored || slot.needs_reconnect {
                    let _ = slot.stream.disconnect();
                    slot.needs_reconnect = connect_stream(slot, &pod_bytes).is_err();
                }
            }
        }
    });
    let _ = timer.update_timer(Some(RECONNECT_INTERVAL), Some(RECONNECT_INTERVAL));
    let _quit = quit_rx.attach(main_loop.loop_(), {
        let main_loop = main_loop.clone();
        move |LoopMessage::Quit| main_loop.quit()
    });
    main_loop.run();
    for slot in slots.borrow().iter() {
        let _ = slot.stream.disconnect();
    }
}
