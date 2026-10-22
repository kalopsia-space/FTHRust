//! The save and record workers. Saving runs on its own thread so commands
//! and status keep flowing while a long mux runs (the reference blocked
//! its command loop). Ports `CaptureEngine::SaveClip` (`capture_engine.cpp:435-533`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use fthrust_core::capture_health::flags;
use fthrust_ipc::{Event, RecordingState, SavedClip, StatusSnapshot};
use fthrust_media::clip_writer::{ClipAudio, ClipVideo, write_clip};
use fthrust_media::recorder::{Recorder, RecorderAudio, RecorderConfig};
use fthrust_media::ring::EncodedPacket;
use fthrust_wayland::backend::monotonic_ns;

use crate::audio::AudioSourceKind;
use crate::engine::EngineHandle;

/// Title of the desktop track.
pub const SYSTEM_TRACK: &str = "Default Mix";
/// Title of the microphone track.
pub const MICROPHONE_TRACK: &str = "Microphone";

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
                    let result = run_save(&engine, &req, &worker_events);
                    let _ = worker_events.send(Event::SaveResult {
                        request_id: req.request_id,
                        result,
                    });
                    worker_busy.store(false, Ordering::SeqCst);
                }
            })
            .expect("spawn save thread");
        Self { tx, busy, events }
    }

    /// Queues a save; a second request while one runs is refused at once.
    pub fn submit(&self, req: SaveRequest) {
        if self.busy.swap(true, Ordering::SeqCst) {
            let _ = self.events.send(Event::SaveResult {
                request_id: req.request_id,
                result: Err("A clip save is already in progress".into()),
            });
            return;
        }
        if self.tx.send(req).is_err() {
            self.busy.store(false, Ordering::SeqCst);
        }
    }

    /// Whether a save is running.
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
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
    let wanted: Vec<(AudioSourceKind, &'static str, bool)> = vec![
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
    ];
    for (kind, title, enabled) in wanted {
        if !enabled {
            continue;
        }
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

/// Continuous recording control.
pub struct RecordWorker {
    engine: EngineHandle,
    events: Sender<Event>,
    state: Arc<Mutex<RecordingState>>,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    backlog_seconds: u64,
}

/// Packets queued for the recorder before it is considered stalled.
pub const RECORD_BACKLOG_SECONDS: u64 = 8;
/// Audio is pulled in windows of this length.
pub const RECORD_AUDIO_STEP_MS: u32 = 100;
/// Audio is pulled this far behind wall time so late blocks still land.
const RECORD_AUDIO_LAG_NS: i64 = 200_000_000;

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
            state: Arc::new(Mutex::new(RecordingState::Idle)),
            stop: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(None),
            backlog_seconds,
        }
    }

    /// Current state.
    pub fn state(&self) -> RecordingState {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Starts recording to `path`; answers with `RecordingStarted` or
    /// `RecordingError`.
    pub fn start(&self, request_id: u64, path: PathBuf) {
        let error = |message: String| {
            let _ = self.events.send(Event::RecordingError {
                request_id,
                message,
            });
        };
        {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if *state != RecordingState::Idle {
                return error("A recording is already in progress".into());
            }
        }
        let status = self.engine.status();
        let Some(enc) = self.engine.encoder_snapshot() else {
            return error("Capture restarted; the replay buffer is still rebuilding".into());
        };
        if status.health_flags & flags::ACTIVE == 0 {
            return error(
                "Capture is not receiving new frames; restart capture before recording".into(),
            );
        }
        let config = self.engine.config();
        let with_audio = config.capture.audio_enabled;
        let rec_cfg = RecorderConfig {
            codec_id: enc.codec_id,
            extradata: enc.extradata.clone(),
            width: enc.width,
            height: enc.height,
            fps: enc.fps,
            bitrate_kbps: enc.bitrate_kbps,
            audio: with_audio.then_some(RecorderAudio {
                sample_rate: fthrust_media::audio_timing::SAMPLE_RATE,
                channels: fthrust_media::audio_timing::CHANNELS as u32,
            }),
        };
        let mut recorder = match Recorder::start(&path, &rec_cfg) {
            Ok(r) => r,
            Err(e) => return error(e.to_string()),
        };
        let capacity = (u64::from(enc.fps) * self.backlog_seconds) as usize;
        let backlog_seconds = self.backlog_seconds;
        let (tx, rx): (Sender<EncodedPacket>, Receiver<EncodedPacket>) =
            crossbeam_channel::bounded(capacity.max(1));
        self.engine.set_packet_tap(Some(tx));
        self.stop.store(false, Ordering::SeqCst);
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = RecordingState::Recording {
            path: path.clone(),
            started_ns: monotonic_ns(),
        };
        let _ = self.events.send(Event::RecordingStarted {
            request_id,
            path: path.clone(),
        });
        let engine = self.engine.clone();
        let events = self.events.clone();
        let state = self.state.clone();
        let stop = self.stop.clone();
        let audio = engine.audio();
        let generation = enc.generation;
        let thread = std::thread::Builder::new()
            .name("fthrust-record".into())
            .spawn(move || {
                let mut audio_cursor_ns = monotonic_ns() - RECORD_AUDIO_LAG_NS;
                let mut failure: Option<String> = None;
                let mut stop_request: Option<u64> = None;
                loop {
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(p) => {
                            if p.generation != generation {
                                failure = Some(
                                    "the encoder was reopened; the recording was closed".into(),
                                );
                                break;
                            }
                            if let Err(e) = recorder.write_video(&p) {
                                failure = Some(e.to_string());
                                break;
                            }
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                    if with_audio {
                        let now = monotonic_ns() - RECORD_AUDIO_LAG_NS;
                        let elapsed_ms = ((now - audio_cursor_ns).max(0) / 1_000_000) as u32;
                        if elapsed_ms >= RECORD_AUDIO_STEP_MS {
                            if let Some(samples) =
                                audio.window(AudioSourceKind::System, now, elapsed_ms)
                                && let Err(e) = recorder.write_audio(&samples)
                            {
                                failure = Some(e.to_string());
                                break;
                            }
                            audio_cursor_ns = now;
                        }
                    }
                    if engine.take_record_overflow() {
                        failure = Some(format!(
                            "the recorder fell more than {backlog_seconds} s behind; the file keeps every committed fragment"
                        ));
                        break;
                    }
                    if engine.take_record_overflow() {
                        failure = Some(format!(
                            "the recorder fell more than {backlog_seconds} s behind; the file keeps every committed fragment"
                        ));
                        break;
                    }
                    if stop.load(Ordering::Relaxed) {
                        stop_request = Some(STOP_REQUEST.load(Ordering::Relaxed));
                        break;
                    }
                }
                engine.set_packet_tap(None);
                let path = recorder.path().to_path_buf();
                let finished = recorder.finish().map_err(|e| e.to_string());
                *state.lock().unwrap_or_else(|p| p.into_inner()) = RecordingState::Idle;
                let request_id = stop_request.unwrap_or(request_id);
                match (failure, finished) {
                    (None, Ok(bytes)) => {
                        let _ = events.send(Event::RecordingStopped {
                            request_id,
                            path,
                            bytes,
                        });
                    }
                    (Some(message), _) | (None, Err(message)) => {
                        let _ = events.send(Event::RecordingError {
                            request_id,
                            message,
                        });
                    }
                }
            })
            .expect("spawn record thread");
        *self.thread.lock().unwrap_or_else(|p| p.into_inner()) = Some(thread);
    }

    /// Asks the recorder to stop; the answer (`RecordingStopped` or
    /// `RecordingError`) comes from the record thread, so this never blocks.
    pub fn stop(&self, request_id: u64) {
        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if !matches!(*state, RecordingState::Recording { .. }) {
                drop(state);
                let _ = self.events.send(Event::RecordingError {
                    request_id,
                    message: "No recording is in progress".into(),
                });
                return;
            }
            *state = RecordingState::Stopping;
        }
        STOP_REQUEST.store(request_id, Ordering::Relaxed);
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Waits for the record thread to finish (shutdown).
    pub fn join(&self) {
        let thread = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(t) = thread {
            let _ = t.join();
        }
    }
}

static STOP_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
