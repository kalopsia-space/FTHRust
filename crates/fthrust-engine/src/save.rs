//! The save and record workers. Saving runs on its own thread so commands
//! and status keep flowing while a long mux runs (the reference blocked
//! its command loop). Ports `CaptureEngine::SaveClip` (`capture_engine.cpp:435-533`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use fthrust_core::capture_health::flags;
use fthrust_ipc::{Event, RecordingState, SavedClip, SourceStatus, StatusSnapshot};
use fthrust_media::audio_timing::{CHANNELS, SAMPLE_RATE};
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
    /// Set while an accepted save still reads live capture.
    live: Arc<AtomicBool>,
    events: Sender<Event>,
}

impl SaveWorker {
    /// Spawns the worker thread.
    pub fn spawn(engine: EngineHandle, events: Sender<Event>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<SaveRequest>();
        let busy = Arc::new(AtomicBool::new(false));
        let live = Arc::new(AtomicBool::new(false));
        let (worker_busy, worker_live) = (busy.clone(), live.clone());
        let worker_events = events.clone();
        std::thread::Builder::new()
            .name("fthrust-save".into())
            .spawn(move || {
                for req in rx {
                    let result = run_save_guarded(&engine, &req, &worker_events, &worker_live);
                    worker_live.store(false, Ordering::SeqCst);
                    let _ = worker_events.send(Event::SaveResult {
                        request_id: req.request_id,
                        result,
                    });
                    worker_busy.store(false, Ordering::SeqCst);
                    crate::alloc::trim();
                }
            })
            .expect("spawn save thread");
        Self {
            tx,
            busy,
            live,
            events,
        }
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
        self.live.store(true, Ordering::SeqCst);
        if let Err(crossbeam_channel::SendError(req)) = self.tx.send(req) {
            self.live.store(false, Ordering::SeqCst);
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

    /// Whether an accepted save still needs live capture: its admission
    /// check, the wait for its last frame, the snapshot and the audio
    /// windows (each bounded). From then on it only reads what it took, so
    /// capture can stop under it.
    pub fn needs_capture(&self) -> bool {
        self.live.load(Ordering::SeqCst)
    }
}

/// [`run_save`] with a panic turned into a failed save, so the worker keeps
/// serving requests and its busy flag is cleared. A `.partial` the
/// panicking save left behind is removed.
fn run_save_guarded(
    engine: &EngineHandle,
    req: &SaveRequest,
    events: &Sender<Event>,
    live: &AtomicBool,
) -> Result<SavedClip, String> {
    let partial = fthrust_media::transactional_save::temporary_path_for(&req.path);
    let partial_existed = partial.exists();
    let run = std::panic::AssertUnwindSafe(|| run_save(engine, req, events, live));
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
    live: &AtomicBool,
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
    // Everything live is taken: the rest only reads the snapshot and PCM.
    live.store(false, Ordering::SeqCst);
    let tracks: Vec<ClipAudio<'_>> = tracks_data
        .iter()
        .map(|(title, samples)| ClipAudio {
            samples,
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS as u32,
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
/// Audio is pulled in windows of at least this length.
pub const RECORD_AUDIO_STEP_MS: u32 = 100;
/// Audio is pulled this far behind wall time so late blocks still land.
pub const RECORD_AUDIO_LAG_NS: i64 = 200_000_000;
/// Longest a stopping recording waits for the frames, and then the audio,
/// captured before the stop.
pub const RECORD_STOP_WAIT: Duration = Duration::from_millis(500);
/// Largest audio window pulled in one piece.
const RECORD_AUDIO_MAX_MS: u32 = 1000;
/// Frames per millisecond at the capture rate.
const FRAMES_PER_MS: u64 = SAMPLE_RATE as u64 / 1000;

/// A recording's audio clock. It starts at the capture time of the
/// track's first sample and counts the frames handed out, always whole
/// milliseconds (48 frames at 48 kHz), so every window starts exactly where
/// the last one ended and the track stays as long as the video however the
/// pulls fall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCursor {
    start_ns: i64,
    frames: u64,
}

impl AudioCursor {
    /// A cursor whose first frame was captured at `start_ns`.
    pub fn new(start_ns: i64) -> Self {
        Self {
            start_ns,
            frames: 0,
        }
    }

    /// Frames (per channel) handed out so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Capture time of the next frame.
    pub fn next_ns(&self) -> i64 {
        self.start_ns + (self.frames / FRAMES_PER_MS) as i64 * 1_000_000
    }

    /// The next window, `(end_ns, duration_ms)`, for audio captured up to
    /// `until_ns`: whole milliseconds, at most a second, or `None` while
    /// less than `min_ms` is due.
    pub fn due(&self, until_ns: i64, min_ms: u32) -> Option<(i64, u32)> {
        let due_ms = (until_ns - self.next_ns()).div_euclid(1_000_000);
        if due_ms < i64::from(min_ms.max(1)) {
            return None;
        }
        let ms = due_ms.min(i64::from(RECORD_AUDIO_MAX_MS)) as u32;
        Some((self.next_ns() + i64::from(ms) * 1_000_000, ms))
    }

    /// Counts the window from [`AudioCursor::due`] as handed out.
    pub fn advance(&mut self, duration_ms: u32) {
        self.frames += u64::from(duration_ms) * FRAMES_PER_MS;
    }
}

/// Pulls every window due for audio captured up to `until_ns` (the first
/// at least `min_ms` long) from each of `sources` and hands it to `write`
/// as `(track, samples)`, track `i` being `sources[i]`. A source with
/// nothing for a window (reconnecting, or not capturing) gets silence of
/// the window's length, so the audio after the gap keeps its place.
pub fn pump_audio<E>(
    cursor: &mut AudioCursor,
    audio: &dyn AudioProvider,
    sources: &[AudioSourceKind],
    until_ns: i64,
    min_ms: u32,
    mut write: impl FnMut(usize, &[f32]) -> Result<(), E>,
) -> Result<(), E> {
    while let Some((end_ns, ms)) = cursor.due(until_ns, min_ms) {
        let samples = (u64::from(ms) * FRAMES_PER_MS) as usize * CHANNELS;
        for (track, kind) in sources.iter().enumerate() {
            let mut pcm = audio.window(*kind, end_ns, ms).unwrap_or_default();
            pcm.resize(samples, 0.0);
            write(track, &pcm)?;
        }
        cursor.advance(ms);
    }
    Ok(())
}

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
    /// When the stop was asked for: frames captured before it are kept.
    stop_ns: AtomicI64,
}

impl RecordShared {
    fn phase(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn request_stop(&self, request_id: u64) {
        self.stop_request.store(request_id, Ordering::SeqCst);
        self.stop_ns.store(monotonic_ns(), Ordering::SeqCst);
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
                stop_ns: AtomicI64::new(0),
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
                    sample_rate: SAMPLE_RATE,
                    channels: CHANNELS as u32,
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
            frame_ns: 1_000_000_000 / i64::from(enc.fps.max(1)),
        })
    }

    fn record(&self, opened: Opened) {
        let Opened {
            mut recorder,
            rx,
            sources,
            generation,
            frame_ns,
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
        let priming_ns =
            i64::from(recorder.audio_priming_frames()) * 1_000_000_000 / i64::from(SAMPLE_RATE);
        let mut session = Session {
            recorder: &mut recorder,
            cursor: None,
            last_video_ns: None,
            generation,
            priming_ns,
            audio_tracks: !sources.is_empty(),
        };
        let end = loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(p) => {
                    if let Err(e) = session.write(&p) {
                        break LoopEnd::Failed(e);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break LoopEnd::TapClosed,
            }
            let until_ns = monotonic_ns() - RECORD_AUDIO_LAG_NS;
            if let Err(e) = session.pump(audio.as_ref(), &sources, until_ns, RECORD_AUDIO_STEP_MS) {
                break LoopEnd::Failed(e);
            }
            if self.engine.take_record_overflow() {
                break LoopEnd::Failed(format!(
                    "the recorder fell more than {} s behind; the file keeps every committed fragment",
                    self.backlog_seconds
                ));
            }
            if self.shared.stop.load(Ordering::SeqCst) {
                break LoopEnd::Stopped;
            }
        };
        let stopped = matches!(end, LoopEnd::Stopped);
        let failure = match end {
            LoopEnd::Stopped => {
                self.finish_after_stop(&mut session, &rx, audio.as_ref(), &sources, frame_ns)
            }
            LoopEnd::TapClosed => {
                self.engine.set_packet_tap(None);
                None
            }
            LoopEnd::Failed(e) => {
                self.engine.set_packet_tap(None);
                Some(e)
            }
        };
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

    /// After a stop: keeps the frames captured before it that are still on
    /// their way through the encoder, then the audio up to the end of the
    /// last frame. Stopping used to drop both: the packets still queued and
    /// the last 200-300 ms of audio.
    fn finish_after_stop(
        &self,
        session: &mut Session<'_>,
        rx: &Receiver<EncodedPacket>,
        audio: &dyn AudioProvider,
        sources: &[AudioSourceKind],
        frame_ns: i64,
    ) -> Option<String> {
        let stop_ns = self.shared.stop_ns.load(Ordering::SeqCst);
        let generation = session.generation;
        let before_stop =
            |p: &EncodedPacket| p.generation == generation && p.wall_time_ns < stop_ns;
        let deadline = Instant::now() + RECORD_STOP_WAIT;
        let mut kept = Vec::new();
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(p) if before_stop(&p) => kept.push(p),
                // A frame from after the stop: the encoder has caught up.
                Ok(_) | Err(_) => break,
            }
        }
        self.engine.set_packet_tap(None);
        kept.extend(rx.try_iter().filter(before_stop));
        for p in &kept {
            if let Err(e) = session.write(p) {
                return Some(e);
            }
        }
        let end_ns = session.last_video_ns? + frame_ns;
        wait_for_audio(audio, sources, end_ns, RECORD_STOP_WAIT);
        session.pump(audio, sources, end_ns, 1).err()
    }
}

/// What [`RecordJob::open`] hands to the recording loop.
struct Opened {
    recorder: Recorder,
    rx: Receiver<EncodedPacket>,
    sources: Vec<AudioSourceKind>,
    generation: u64,
    frame_ns: i64,
}

/// Why the recording loop ended.
enum LoopEnd {
    /// `stop` or `shutdown` asked for it.
    Stopped,
    /// Nothing will send packets any more.
    TapClosed,
    /// The recording cannot go on.
    Failed(String),
}

/// The live state of one recording's writer.
struct Session<'a> {
    recorder: &'a mut Recorder,
    /// Created at the first written keyframe.
    cursor: Option<AudioCursor>,
    /// Capture time of the last video packet written.
    last_video_ns: Option<i64>,
    generation: u64,
    priming_ns: i64,
    audio_tracks: bool,
}

impl Session<'_> {
    /// Writes one video packet; the first written keyframe starts the audio
    /// clock. The encoder's priming plays before the first sample handed to
    /// it, so the audio starts that long after the keyframe and every
    /// sample plays at its capture time.
    fn write(&mut self, p: &EncodedPacket) -> Result<(), String> {
        if p.generation != self.generation {
            return Err("the encoder was reopened; the recording was closed".into());
        }
        let before = self.recorder.video_packets();
        self.recorder.write_video(p).map_err(|e| e.to_string())?;
        if self.recorder.video_packets() > before {
            self.last_video_ns = Some(p.wall_time_ns);
        }
        if self.cursor.is_none()
            && self.audio_tracks
            && let Some(start) = self.recorder.video_start_ns()
        {
            self.cursor = Some(AudioCursor::new(start + self.priming_ns));
        }
        Ok(())
    }

    fn pump(
        &mut self,
        audio: &dyn AudioProvider,
        sources: &[AudioSourceKind],
        until_ns: i64,
        min_ms: u32,
    ) -> Result<(), String> {
        let Some(cursor) = self.cursor.as_mut() else {
            return Ok(());
        };
        let recorder = &mut *self.recorder;
        pump_audio(cursor, audio, sources, until_ns, min_ms, |track, pcm| {
            recorder.write_audio(track, pcm)
        })
        .map_err(|e| e.to_string())
    }
}
