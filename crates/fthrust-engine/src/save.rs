//! The save and record workers. Saving runs on its own thread so commands
//! and status keep flowing while a long mux runs (the reference blocked
//! its command loop). Ports `CaptureEngine::SaveClip` (`capture_engine.cpp:435-533`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use fthrust_core::capture_health::flags;
use fthrust_ipc::{Event, RecordingState, SavedClip, SourceStatus, StatusSnapshot};
use fthrust_media::clip_writer::{ClipAudio, ClipVideo, write_clip};
use fthrust_media::recorder::{Recorder, RecorderAudio, RecorderConfig};
use fthrust_media::ring::EncodedPacket;
use fthrust_wayland::backend::monotonic_ns;

use crate::audio::{AudioProvider, AudioSourceKind};
use crate::engine::EngineHandle;

/// Title of the desktop track.
pub const SYSTEM_TRACK: &str = "Default Mix";
/// Title of the microphone track.
pub const MICROPHONE_TRACK: &str = "Microphone";
/// Longest a save waits for its audio sources to deliver the clip's end.
pub const SAVE_AUDIO_WAIT: Duration = Duration::from_millis(300);

/// A save request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveRequest {
    /// Correlates the reply events.
    pub request_id: u64,
    /// Destination (must not exist).
    pub path: PathBuf,
    /// Seconds to save; 0 = the configured buffer length.
    pub seconds: u32,
}

/// The reference's admission rule, with its exact messages.
pub fn admission(
    status: &StatusSnapshot,
    snapshot_present: bool,
    ring_has_packets: bool,
) -> Result<(), &'static str> {
    if status.paused
        || status.health_flags & (flags::BACKEND_FAILED | flags::RECOVERING | flags::PAUSED) != 0
        || status.health_flags & flags::ACTIVE == 0
    {
        return Err("Capture is not receiving new frames; restart capture before saving");
    }
    if !snapshot_present {
        return Err("Capture restarted; the replay buffer is still rebuilding");
    }
    if !ring_has_packets {
        return Err("Nothing to save: the replay buffer contains no video packets");
    }
    Ok(())
}

/// Runs saves one at a time on a dedicated thread.
pub struct SaveWorker {
    tx: Sender<SaveRequest>,
    busy: Arc<AtomicBool>,
    events: Sender<Event>,
}

impl SaveWorker {
    /// Spawns the worker thread.
    pub fn spawn(engine: EngineHandle, events: Sender<Event>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<SaveRequest>();
        let busy = Arc::new(AtomicBool::new(false));
        let worker_busy = busy.clone();
        let worker_events = events.clone();
        std::thread::Builder::new()
            .name("fthrust-save".into())
            .spawn(move || {
                for req in rx {
                    let result = run_save_guarded(&engine, &req, &worker_events);
                    let _ = worker_events.send(Event::SaveResult {
                        request_id: req.request_id,
                        result,
                    });
                    worker_busy.store(false, Ordering::SeqCst);
                    crate::alloc::trim();
                }
            })
            .expect("spawn save thread");
        Self { tx, busy, events }
    }

    /// Queues a save. Every call is answered by exactly one `SaveResult`:
    /// at once when another save is running (refused), otherwise when this
    /// one ends.
    pub fn submit(&self, req: SaveRequest) {
        if self.busy.swap(true, Ordering::SeqCst) {
            let _ = self.events.send(Event::SaveResult {
                request_id: req.request_id,
                result: Err("A clip save is already in progress".into()),
            });
            return;
        }
        if let Err(crossbeam_channel::SendError(req)) = self.tx.send(req) {
            self.busy.store(false, Ordering::SeqCst);
            let _ = self.events.send(Event::SaveResult {
                request_id: req.request_id,
                result: Err("The clip could not be saved: the save worker is not running".into()),
            });
        }
    }

    /// Whether a save is running.
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
    }
}

/// [`run_save`] with a panic turned into a failed save, so the worker keeps
/// serving requests and its busy flag is cleared. A `.partial` the
/// panicking save left behind is removed.
fn run_save_guarded(
    engine: &EngineHandle,
    req: &SaveRequest,
    events: &Sender<Event>,
) -> Result<SavedClip, String> {
    let partial = fthrust_media::transactional_save::temporary_path_for(&req.path);
    let partial_existed = partial.exists();
    let run = std::panic::AssertUnwindSafe(|| run_save(engine, req, events));
    match std::panic::catch_unwind(run) {
        Ok(result) => result,
        Err(panic) => {
            let why = panic_message(panic.as_ref());
            tracing::error!(path = %req.path.display(), "clip save panicked: {why}");
            if !partial_existed {
                let _ = std::fs::remove_file(&partial);
            }
            Err(format!("The clip could not be saved: {why}"))
        }
    }
}

/// The text of a panic payload.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "an internal error".to_string())
}

/// Waits until every source in `kinds` that reports its progress has
/// delivered audio up to `end_ns`, or `timeout` passes. Audio reaches the
/// ring a capture latency after it plays, so without this a window ending
/// now ends on silence.
pub fn wait_for_audio(
    audio: &dyn AudioProvider,
    kinds: &[AudioSourceKind],
    end_ns: i64,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    while kinds
        .iter()
        .any(|k| audio.captured_until(*k).is_some_and(|t| t < end_ns))
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn run_save(
    engine: &EngineHandle,
    req: &SaveRequest,
    events: &Sender<Event>,
) -> Result<SavedClip, String> {
    let status = engine.status();
    let config = engine.config();
    let ring = engine.ring();
    let snapshot_meta = engine.encoder_snapshot();
    let stats = ring.stats();
    admission(&status, snapshot_meta.is_some(), stats.packets > 0).map_err(str::to_string)?;
    let enc = snapshot_meta.expect("admitted");
    let fps = config.capture.fps.max(1);
    let seconds = if req.seconds == 0 {
        config.capture.buffer_seconds
    } else {
        req.seconds
    };
    let save_end_ns = monotonic_ns();
    let publish_wait = Duration::from_millis(u64::from((3000 / fps).max(100)));
    ring.wait_until_published(save_end_ns, publish_wait);
    let snapshot = ring.snapshot(seconds.saturating_mul(1000), save_end_ns, enc.generation);
    let Some(snapshot) = snapshot else {
        let has_any = ring.stats().packets > 0;
        return Err(if has_any {
            "Capture restarted; the replay buffer is still rebuilding".to_string()
        } else {
            "Nothing to save: the replay buffer contains no video packets".to_string()
        });
    };
    if snapshot.packets.is_empty() {
        return Err("Nothing to save: the replay buffer contains no video packets".into());
    }
    let _ = events.send(Event::SaveStarted {
        request_id: req.request_id,
    });

    // Audio windows end where the video presentation ends.
    let duration_ms = ((snapshot.presentation_end_ns - snapshot.presentation_start_ns).max(0)
        as u64)
        .div_ceil(1_000_000) as u32;
    let audio = engine.audio();
    let mut errors = Vec::new();
    let mut tracks_data: Vec<(&'static str, Vec<f32>)> = Vec::new();
    let wanted: Vec<(AudioSourceKind, &'static str)> = [
        (
            AudioSourceKind::System,
            SYSTEM_TRACK,
            config.capture.audio_enabled,
        ),
        (
            AudioSourceKind::Microphone,
            MICROPHONE_TRACK,
            !config.capture.microphone_endpoint_id.is_empty(),
        ),
    ]
    .into_iter()
    .filter(|(_, _, enabled)| *enabled)
    .map(|(kind, title, _)| (kind, title))
    .collect();
    // The video waited for its last frame; the audio waits for its sources
    // to deliver the same moment, so the clip does not end on padding.
    let kinds: Vec<AudioSourceKind> = wanted.iter().map(|(kind, _)| *kind).collect();
    wait_for_audio(
        audio.as_ref(),
        &kinds,
        snapshot.presentation_end_ns,
        SAVE_AUDIO_WAIT,
    );
    for (kind, title) in wanted {
        match audio.window(kind, snapshot.presentation_end_ns, duration_ms) {
            Some(samples) if !samples.is_empty() => tracks_data.push((title, samples)),
            _ => errors.push(format!("{title}: the audio source is not capturing")),
        }
    }
    let tracks: Vec<ClipAudio<'_>> = tracks_data
        .iter()
        .map(|(title, samples)| ClipAudio {
            samples,
            sample_rate: fthrust_media::audio_timing::SAMPLE_RATE,
            channels: fthrust_media::audio_timing::CHANNELS as u32,
            title,
        })
        .collect();
    let video = ClipVideo {
        snapshot: &snapshot,
        codec_id: enc.codec_id,
        extradata: &enc.extradata,
        width: enc.width,
        height: enc.height,
        bitrate_kbps: enc.bitrate_kbps,
        separate_audio: config.capture.separate_audio_enabled,
    };
    let request_id = req.request_id;
    let last_reported = std::cell::Cell::new(0u64);
    let progress = |bytes: u64| {
        if bytes - last_reported.get() >= 1 << 20 {
            last_reported.set(bytes);
            let _ = events.send(Event::SaveProgress {
                request_id,
                bytes_written: bytes,
            });
        }
    };
    let result =
        write_clip(&req.path, &video, &tracks, Some(&progress)).map_err(|e| e.to_string())?;
    let mut audio_error = result.audio_error;
    if !errors.is_empty() {
        let mut all = errors;
        if let Some(e) = audio_error.take() {
            all.push(e);
        }
        audio_error = Some(all.join("; "));
    }
    Ok(SavedClip {
        path: result.path,
        bytes_written: result.bytes_written,
        audio_included: result.audio_included,
        audio_error,
        audio_tracks: result.audio_tracks,
    })
}

/// Packets queued for the recorder before it is considered stalled.
pub const RECORD_BACKLOG_SECONDS: u64 = 8;
/// Audio is pulled in windows of this length.
pub const RECORD_AUDIO_STEP_MS: u32 = 100;
/// Audio is pulled this far behind wall time so late blocks still land.
const RECORD_AUDIO_LAG_NS: i64 = 200_000_000;

/// Where a recording is in its life. `Starting` covers opening the file on
/// the record thread; the status reports it as `Idle`.
enum Phase {
    Idle,
    Starting,
    Recording { path: PathBuf, started_ns: i64 },
    Stopping,
}

struct RecordShared {
    phase: Mutex<Phase>,
    stop: AtomicBool,
    /// The request that asked for the stop (0 for a shutdown).
    stop_request: AtomicU64,
}

impl RecordShared {
    fn phase(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn request_stop(&self, request_id: u64) {
        self.stop_request.store(request_id, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Continuous recording control.
pub struct RecordWorker {
    engine: EngineHandle,
    events: Sender<Event>,
    shared: Arc<RecordShared>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    backlog_seconds: u64,
}

impl RecordWorker {
    /// Creates an idle controller.
    pub fn new(engine: EngineHandle, events: Sender<Event>) -> Self {
        Self::with_backlog(engine, events, RECORD_BACKLOG_SECONDS)
    }

    /// Creates an idle controller with a custom packet backlog (seconds of
    /// frames the recorder may fall behind before the recording fails).
    pub fn with_backlog(engine: EngineHandle, events: Sender<Event>, backlog_seconds: u64) -> Self {
        Self {
            engine,
            events,
            shared: Arc::new(RecordShared {
                phase: Mutex::new(Phase::Idle),
                stop: AtomicBool::new(false),
                stop_request: AtomicU64::new(0),
            }),
            thread: Mutex::new(None),
            backlog_seconds,
        }
    }

    /// Current state.
    pub fn state(&self) -> RecordingState {
        match &*self.shared.phase() {
            Phase::Idle | Phase::Starting => RecordingState::Idle,
            Phase::Recording { path, started_ns } => RecordingState::Recording {
                path: path.clone(),
                started_ns: *started_ns,
            },
            Phase::Stopping => RecordingState::Stopping,
        }
    }

    /// Starts recording to `path`; the record thread answers with
    /// `RecordingStarted` or `RecordingError`. Nothing here touches the
    /// file system, so the caller (the daemon loop) never waits on a disk.
    pub fn start(&self, request_id: u64, path: PathBuf) {
        {
            let mut phase = self.shared.phase();
            if !matches!(*phase, Phase::Idle) {
                drop(phase);
                let _ = self.events.send(Event::RecordingError {
                    request_id,
                    message: "A recording is already in progress".into(),
                });
                return;
            }
            *phase = Phase::Starting;
        }
        self.shared.stop.store(false, Ordering::SeqCst);
        let mut thread = self.thread.lock().unwrap_or_else(|p| p.into_inner());
        let job = RecordJob {
            engine: self.engine.clone(),
            events: self.events.clone(),
            shared: self.shared.clone(),
            request_id,
            path,
            backlog_seconds: self.backlog_seconds,
            previous: thread.take(),
        };
        match std::thread::Builder::new()
            .name("fthrust-record".into())
            .spawn(move || job.run())
        {
            Ok(t) => *thread = Some(t),
            Err(e) => {
                *self.shared.phase() = Phase::Idle;
                let _ = self.events.send(Event::RecordingError {
                    request_id,
                    message: format!("The recording could not be started: {e}"),
                });
            }
        }
    }

    /// Asks the recorder to stop; the answer (`RecordingStopped` or
    /// `RecordingError`) comes from the record thread, so this never blocks.
    pub fn stop(&self, request_id: u64) {
        {
            let mut phase = self.shared.phase();
            if !matches!(*phase, Phase::Starting | Phase::Recording { .. }) {
                drop(phase);
                let _ = self.events.send(Event::RecordingError {
                    request_id,
                    message: "No recording is in progress".into(),
                });
                return;
            }
            *phase = Phase::Stopping;
        }
        self.shared.request_stop(request_id);
    }

    /// Stops a recording in progress (or still starting) and waits for its
    /// thread (shutdown).
    pub fn shutdown(&self) {
        {
            let mut phase = self.shared.phase();
            if matches!(*phase, Phase::Starting | Phase::Recording { .. }) {
                *phase = Phase::Stopping;
                self.shared.request_stop(0);
            }
        }
        self.join();
    }

    /// Waits for the record thread to finish.
    pub fn join(&self) {
        let thread = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(t) = thread {
            let _ = t.join();
        }
    }
}

/// One recording, run on its own thread from open to finish.
struct RecordJob {
    engine: EngineHandle,
    events: Sender<Event>,
    shared: Arc<RecordShared>,
    request_id: u64,
    path: PathBuf,
    backlog_seconds: u64,
    /// The previous recording's thread, which may still be finishing.
    previous: Option<std::thread::JoinHandle<()>>,
}

impl RecordJob {
    fn run(mut self) {
        if let Some(previous) = self.previous.take() {
            let _ = previous.join();
        }
        match self.open() {
            Ok(opened) => self.record(opened),
            Err(message) => {
                *self.shared.phase() = Phase::Idle;
                let _ = self.events.send(Event::RecordingError {
                    request_id: self.request_id,
                    message,
                });
            }
        }
        crate::alloc::trim();
    }

    /// Admits the recording, opens the file and installs the packet tap.
    /// The file gets one AAC track per capturing source, titled like a
    /// clip's.
    fn open(&self) -> Result<Opened, String> {
        let status = self.engine.status();
        let Some(enc) = self.engine.encoder_snapshot() else {
            return Err("Capture restarted; the replay buffer is still rebuilding".into());
        };
        if status.health_flags & flags::ACTIVE == 0 {
            return Err(
                "Capture is not receiving new frames; restart capture before recording".into(),
            );
        }
        let config = self.engine.config();
        let sources: Vec<(AudioSourceKind, &str)> = [
            (
                AudioSourceKind::System,
                SYSTEM_TRACK,
                config.capture.audio_enabled,
                &status.audio.system,
            ),
            (
                AudioSourceKind::Microphone,
                MICROPHONE_TRACK,
                !config.capture.microphone_endpoint_id.is_empty(),
                &status.audio.microphone,
            ),
        ]
        .into_iter()
        .filter(|(_, _, enabled, state)| *enabled && **state != SourceStatus::Disabled)
        .map(|(kind, title, _, _)| (kind, title))
        .collect();
        let rec_cfg = RecorderConfig {
            codec_id: enc.codec_id,
            extradata: enc.extradata.clone(),
            width: enc.width,
            height: enc.height,
            fps: enc.fps,
            bitrate_kbps: enc.bitrate_kbps,
            audio: sources
                .iter()
                .map(|(_, title)| RecorderAudio {
                    sample_rate: fthrust_media::audio_timing::SAMPLE_RATE,
                    channels: fthrust_media::audio_timing::CHANNELS as u32,
                    title: (*title).to_string(),
                })
                .collect(),
        };
        let recorder = Recorder::start(&self.path, &rec_cfg).map_err(|e| e.to_string())?;
        let capacity = (u64::from(enc.fps) * self.backlog_seconds) as usize;
        let (tx, rx) = crossbeam_channel::bounded(capacity.max(1));
        self.engine.set_packet_tap(Some(tx));
        Ok(Opened {
            recorder,
            rx,
            sources: sources.into_iter().map(|(kind, _)| kind).collect(),
            generation: enc.generation,
        })
    }

    fn record(&self, opened: Opened) {
        let Opened {
            mut recorder,
            rx,
            sources,
            generation,
        } = opened;
        {
            // A stop asked for while the file was opening stays in force.
            let mut phase = self.shared.phase();
            if !matches!(*phase, Phase::Stopping) {
                *phase = Phase::Recording {
                    path: self.path.clone(),
                    started_ns: monotonic_ns(),
                };
            }
        }
        let _ = self.events.send(Event::RecordingStarted {
            request_id: self.request_id,
            path: self.path.clone(),
        });
        let audio = self.engine.audio();
        let mut audio_cursor_ns = monotonic_ns() - RECORD_AUDIO_LAG_NS;
        let mut failure: Option<String> = None;
        let mut stopped = false;
        'run: loop {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(p) => {
                    if p.generation != generation {
                        failure = Some("the encoder was reopened; the recording was closed".into());
                        break;
                    }
                    if let Err(e) = recorder.write_video(&p) {
                        failure = Some(e.to_string());
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if !sources.is_empty() {
                let now = monotonic_ns() - RECORD_AUDIO_LAG_NS;
                let elapsed_ms = ((now - audio_cursor_ns).max(0) / 1_000_000) as u32;
                if elapsed_ms >= RECORD_AUDIO_STEP_MS {
                    for (track, kind) in sources.iter().enumerate() {
                        if let Some(samples) = audio.window(*kind, now, elapsed_ms)
                            && let Err(e) = recorder.write_audio(track, &samples)
                        {
                            failure = Some(e.to_string());
                            break 'run;
                        }
                    }
                    audio_cursor_ns = now;
                }
            }
            if self.engine.take_record_overflow() {
                failure = Some(format!(
                    "the recorder fell more than {} s behind; the file keeps every committed fragment",
                    self.backlog_seconds
                ));
                break;
            }
            if self.shared.stop.load(Ordering::SeqCst) {
                stopped = true;
                break;
            }
        }
        self.engine.set_packet_tap(None);
        let path = recorder.path().to_path_buf();
        let finished = recorder.finish().map_err(|e| e.to_string());
        *self.shared.phase() = Phase::Idle;
        let request_id = if stopped {
            self.shared.stop_request.load(Ordering::SeqCst)
        } else {
            self.request_id
        };
        let event = match (failure, finished) {
            (None, Ok(bytes)) => Event::RecordingStopped {
                request_id,
                path,
                bytes,
            },
            (Some(message), _) | (None, Err(message)) => Event::RecordingError {
                request_id,
                message,
            },
        };
        let _ = self.events.send(event);
    }
}

/// What [`RecordJob::open`] hands to the recording loop.
struct Opened {
    recorder: Recorder,
    rx: Receiver<EncodedPacket>,
    sources: Vec<AudioSourceKind>,
    generation: u64,
}
