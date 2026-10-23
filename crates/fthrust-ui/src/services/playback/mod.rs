//! Playback (plan 6a): the editor's clock, workers and sync. The audio
//! worker ports the reference's `_DecodeWorker`; the video worker decodes a
//! few frames ahead; `tick` applies the sync rule and collects events.

pub mod audio_out;
pub mod sync;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use fthrust_core::audio_manifest::{AudioManifest, read_verified};
use fthrust_media::playback::mixer::Mixer;
use fthrust_media::playback::video::{Frame, VideoDecoder, VideoInfo};
use fthrust_media::playback::{
    CANONICAL_CHANNELS, CANONICAL_SAMPLE_RATE, FRAMES_PER_BLOCK, SourceSpec, proxy,
};

use audio_out::{AudioOutput, PcmQueue};
use sync::{RECOVERY_S, SyncDecision, decide};

/// What happened since the last `events()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackEvent {
    /// The clip ended.
    Eof,
    /// Audio output or decoding stopped; playback continues video-only.
    AudioFailed(String),
    /// A manifest track could not be decoded (by source id).
    SourceFailed(String),
    /// The seek proxy is in use now.
    ProxyReady,
}

/// Per-track mix state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceState {
    /// 0–100.
    pub gain_percent: u8,
    /// Muted.
    pub muted: bool,
}

impl Default for SourceState {
    fn default() -> Self {
        SourceState {
            gain_percent: 100,
            muted: false,
        }
    }
}

/// A clock in milliseconds (real time, or test-controlled).
pub type ClockFn = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Real time since the process started.
pub fn real_clock() -> ClockFn {
    let epoch = std::time::Instant::now();
    Arc::new(move || epoch.elapsed().as_millis() as u64)
}

// ----------------------------------------------------------------------------
// Audio worker

enum AudioCmd {
    Play,
    Pause,
    Seek(u64),
    Rate { rate: f32, preserve_pitch: bool },
    Mix { gains: Vec<f32>, master: f32 },
    Stop,
}

struct AudioShared {
    /// Source-time frames decoded (position × 48 000 / 1000).
    decoded_frames: AtomicU64,
    playing: AtomicBool,
    eof: AtomicBool,
}

struct AudioWorker {
    tx: Sender<AudioCmd>,
    shared: Arc<AudioShared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AudioWorker {
    fn start(
        mixer: Mixer,
        queue: Arc<PcmQueue>,
        events: Sender<PlaybackEvent>,
        gains: Vec<f32>,
        master: f32,
    ) -> AudioWorker {
        let (tx, rx) = channel();
        let shared = Arc::new(AudioShared {
            decoded_frames: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            eof: AtomicBool::new(false),
        });
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("fthrust-playback-audio".into())
                .spawn(move || audio_loop(mixer, queue, rx, events, shared, gains, master))
                .ok()
        };
        AudioWorker { tx, shared, thread }
    }

    fn send(&self, cmd: AudioCmd) {
        let _ = self.tx.send(cmd);
    }

    fn position_ms(&self) -> u64 {
        self.shared.decoded_frames.load(Ordering::Relaxed) * 1000 / u64::from(CANONICAL_SAMPLE_RATE)
    }
}

impl Drop for AudioWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(AudioCmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Linear resampling of a block by `rate` (the no-pitch-preservation fallback).
fn resample_block(block: &[f32], out_frames: usize) -> Vec<f32> {
    let in_frames = block.len() / CANONICAL_CHANNELS;
    if in_frames == 0 || out_frames == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(out_frames * CANONICAL_CHANNELS);
    for i in 0..out_frames {
        let pos = i as f64 * in_frames as f64 / out_frames as f64;
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(in_frames - 1);
        let frac = (pos - i0 as f64) as f32;
        for ch in 0..CANONICAL_CHANNELS {
            let a = block[i0 * CANONICAL_CHANNELS + ch];
            let b = block[i1 * CANONICAL_CHANNELS + ch];
            out.push(a + (b - a) * frac);
        }
    }
    out
}

fn audio_loop(
    mut mixer: Mixer,
    queue: Arc<PcmQueue>,
    rx: Receiver<AudioCmd>,
    events: Sender<PlaybackEvent>,
    shared: Arc<AudioShared>,
    mut gains: Vec<f32>,
    mut master: f32,
) {
    let mut rate = 1.0f32;
    let mut native_pitch = false;
    let mut pending_rate: Option<(f32, bool)> = None;
    loop {
        let playing = shared.playing.load(Ordering::Relaxed);
        let cmd = if playing {
            match rx.recv_timeout(Duration::from_millis(5)) {
                Ok(c) => Some(c),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(_) => return,
            }
        } else {
            match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            }
        };
        if let Some(cmd) = cmd {
            match cmd {
                AudioCmd::Stop => return,
                AudioCmd::Play => {
                    shared.eof.store(false, Ordering::Relaxed);
                    shared.playing.store(true, Ordering::Relaxed);
                }
                AudioCmd::Pause => shared.playing.store(false, Ordering::Relaxed),
                AudioCmd::Seek(ms) => {
                    queue.clear();
                    shared.eof.store(false, Ordering::Relaxed);
                    if let Err(e) = mixer.seek(ms) {
                        let _ = events.send(PlaybackEvent::AudioFailed(e.to_string()));
                        return;
                    }
                    shared.decoded_frames.store(
                        ms * u64::from(CANONICAL_SAMPLE_RATE) / 1000,
                        Ordering::Relaxed,
                    );
                }
                AudioCmd::Rate {
                    rate: r,
                    preserve_pitch,
                } => pending_rate = Some((r, preserve_pitch)),
                AudioCmd::Mix {
                    gains: g,
                    master: m,
                } => {
                    gains = g;
                    master = m;
                }
            }
            continue;
        }
        // Playing and no command: produce one block when there is room.
        if let Some((r, p)) = pending_rate.take() {
            rate = r;
            match mixer.set_rate(r, p) {
                Ok(active) => native_pitch = active,
                Err(e) => {
                    let _ = events.send(PlaybackEvent::AudioFailed(e.to_string()));
                    return;
                }
            }
        }
        if !queue.has_room(FRAMES_PER_BLOCK) {
            continue;
        }
        let requested = if native_pitch {
            FRAMES_PER_BLOCK
        } else {
            ((FRAMES_PER_BLOCK as f32 * rate).round() as usize).max(1)
        };
        match mixer.pull(requested, &gains, master) {
            Ok(Some(block)) => {
                let source_frames = block.len() / CANONICAL_CHANNELS;
                let (out, source_frames) = if native_pitch {
                    let out_frames = source_frames;
                    (block, (out_frames as f32 * rate).round() as usize)
                } else {
                    let out_frames = (source_frames as f32 / rate).round().max(1.0) as usize;
                    (resample_block(&block, out_frames), source_frames)
                };
                queue.push(&out);
                shared
                    .decoded_frames
                    .fetch_add(source_frames as u64, Ordering::Relaxed);
            }
            Ok(None) => {
                shared.playing.store(false, Ordering::Relaxed);
                shared.eof.store(true, Ordering::Relaxed);
            }
            Err(e) => {
                let _ = events.send(PlaybackEvent::AudioFailed(e.to_string()));
                shared.playing.store(false, Ordering::Relaxed);
                return;
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Video worker

enum VideoCmd {
    Play,
    Pause,
    Seek(u64),
    Reopen(PathBuf, u64),
    Stop,
}

const VIDEO_AHEAD: usize = 4;

struct VideoShared {
    frames: Mutex<VecDeque<Frame>>,
    space: Condvar,
    eof: AtomicBool,
}

struct VideoWorker {
    tx: Sender<VideoCmd>,
    shared: Arc<VideoShared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl VideoWorker {
    fn start(decoder: VideoDecoder) -> VideoWorker {
        let (tx, rx) = channel();
        let shared = Arc::new(VideoShared {
            frames: Mutex::new(VecDeque::new()),
            space: Condvar::new(),
            eof: AtomicBool::new(false),
        });
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("fthrust-playback-video".into())
                .spawn(move || video_loop(decoder, rx, shared))
                .ok()
        };
        VideoWorker { tx, shared, thread }
    }

    fn send(&self, cmd: VideoCmd) {
        let _ = self.tx.send(cmd);
        self.shared.space.notify_all();
    }
}

impl Drop for VideoWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(VideoCmd::Stop);
        self.shared.space.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct VideoState {
    decoder: VideoDecoder,
    playing: bool,
    /// After a seek while paused, exactly one frame (the target) is decoded.
    want_one: bool,
}

impl VideoState {
    fn reset(&mut self, shared: &VideoShared) {
        shared.eof.store(false, Ordering::Relaxed);
        if let Ok(mut f) = shared.frames.lock() {
            f.clear();
        }
        self.want_one = true;
    }

    /// `false` on `Stop`.
    fn apply(&mut self, cmd: VideoCmd, shared: &VideoShared) -> bool {
        match cmd {
            VideoCmd::Stop => return false,
            VideoCmd::Play => self.playing = true,
            VideoCmd::Pause => self.playing = false,
            VideoCmd::Seek(ms) => {
                let _ = self.decoder.seek(ms);
                self.reset(shared);
            }
            VideoCmd::Reopen(path, ms) => {
                if let Ok(mut d) = VideoDecoder::open(&path)
                    && d.seek(ms).is_ok()
                {
                    self.decoder = d;
                    self.reset(shared);
                }
            }
        }
        true
    }
}

fn video_loop(decoder: VideoDecoder, rx: Receiver<VideoCmd>, shared: Arc<VideoShared>) {
    let mut st = VideoState {
        decoder,
        playing: false,
        want_one: true,
    };
    loop {
        match rx.try_recv() {
            Ok(cmd) => {
                if !st.apply(cmd, &shared) {
                    return;
                }
                continue;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(_) => return,
        }
        let queued = shared.frames.lock().map(|f| f.len()).unwrap_or(0);
        let need = (st.playing && queued < VIDEO_AHEAD) || (st.want_one && queued == 0);
        if !need || shared.eof.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(4)) {
                Ok(cmd) => {
                    if !st.apply(cmd, &shared) {
                        return;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return,
            }
            continue;
        }
        match st.decoder.next_frame() {
            Ok(Some(frame)) => {
                if let Ok(mut f) = shared.frames.lock() {
                    f.push_back(frame);
                }
                st.want_one = false;
            }
            Ok(None) => {
                shared.eof.store(true, Ordering::Relaxed);
                st.want_one = false;
            }
            Err(e) => {
                tracing::warn!(error = %e, "video decode failed");
                shared.eof.store(true, Ordering::Relaxed);
                st.want_one = false;
            }
        }
    }
}

// ----------------------------------------------------------------------------
// The service

/// One clip's playback.
pub struct PlaybackService {
    info: VideoInfo,
    duration_ms: u64,
    source_path: PathBuf,
    clock: ClockFn,
    queue: Arc<PcmQueue>,
    output: Box<dyn AudioOutput>,
    audio: Option<AudioWorker>,
    audio_live: bool,
    video: VideoWorker,
    events_rx: Receiver<PlaybackEvent>,
    events: Vec<PlaybackEvent>,
    playing: bool,
    holding: bool,
    guard_until_ms: u64,
    /// Video-only clock: position at `clock_started_ms`.
    clock_base_ms: u64,
    clock_started_ms: Option<u64>,
    rate: f32,
    preserve_pitch: bool,
    master_percent: u8,
    sources: Vec<(String, SourceState)>,
    last_frame_pts: Option<u64>,
    eof_reported: bool,
    proxy: Option<(Receiver<PathBuf>, Arc<AtomicBool>)>,
}

impl PlaybackService {
    /// Opens `path` (its manifest from the sidecar when present); `home`
    /// locates the proxy cache; `output` is the audio device; `clock`
    /// drives the video-only position.
    pub fn open(
        path: &Path,
        home: &Path,
        mut output: Box<dyn AudioOutput>,
        clock: ClockFn,
    ) -> Result<PlaybackService, String> {
        let decoder = VideoDecoder::open(path).map_err(|e| e.to_string())?;
        let info = decoder.info().clone();
        let duration_ms = info.duration_ms;
        let (events_tx, events_rx) = channel();
        let queue = Arc::new(PcmQueue::new());
        // Tracks: the manifest's sources, else the best audio stream.
        let manifest: Option<AudioManifest> = read_verified(path).ok().flatten();
        let specs: Vec<SourceSpec> = match &manifest {
            Some(m) => m
                .sources
                .iter()
                .map(|s| SourceSpec {
                    source_id: s.source_uuid.clone(),
                    container_index: s.stream_index as usize,
                })
                .collect(),
            None => default_audio_spec(path).into_iter().collect(),
        };
        let sources: Vec<(String, SourceState)> = specs
            .iter()
            .map(|s| (s.source_id.clone(), SourceState::default()))
            .collect();
        let mut events = Vec::new();
        let mut audio = None;
        let mut audio_live = false;
        if !specs.is_empty() {
            match Mixer::open(path, &specs) {
                Ok(mixer) => {
                    for i in mixer.failed_sources() {
                        if let Some(s) = specs.get(i) {
                            events.push(PlaybackEvent::SourceFailed(s.source_id.clone()));
                        }
                    }
                    match output.start(queue.clone()) {
                        Ok(()) => {
                            let gains: Vec<f32> = sources.iter().map(|_| 1.0).collect();
                            audio = Some(AudioWorker::start(
                                mixer,
                                queue.clone(),
                                events_tx.clone(),
                                gains,
                                1.0,
                            ));
                            audio_live = true;
                        }
                        Err(e) => events.push(PlaybackEvent::AudioFailed(e)),
                    }
                }
                Err(e) => events.push(PlaybackEvent::AudioFailed(e.to_string())),
            }
        }
        let video = VideoWorker::start(decoder);
        // The proxy, if the clip needs one, replaces the video source later.
        let cancel = Arc::new(AtomicBool::new(false));
        let (ptx, prx) = channel();
        {
            let cancel = cancel.clone();
            let cache = proxy::cache_dir(home);
            let src = path.to_path_buf();
            std::thread::Builder::new()
                .name("fthrust-playback-proxy".into())
                .spawn(move || {
                    let p = proxy::prepare(&cache, &src, &cancel);
                    let _ = ptx.send(p);
                })
                .ok();
        }
        Ok(PlaybackService {
            info,
            duration_ms,
            source_path: path.to_path_buf(),
            clock,
            queue,
            output,
            audio,
            audio_live,
            video,
            events_rx,
            events,
            playing: false,
            holding: false,
            guard_until_ms: 0,
            clock_base_ms: 0,
            clock_started_ms: None,
            rate: 1.0,
            preserve_pitch: true,
            master_percent: 100,
            sources,
            last_frame_pts: None,
            eof_reported: false,
            proxy: Some((prx, cancel)),
        })
    }

    /// Stream metadata.
    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// Clip length.
    pub fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    /// Whether audio drives the clock.
    pub fn audio_live(&self) -> bool {
        self.audio_live
    }

    /// Playing.
    pub fn playing(&self) -> bool {
        self.playing
    }

    fn now_ms(&self) -> u64 {
        (self.clock)()
    }

    /// The current position in ms.
    pub fn position_ms(&self) -> u64 {
        if self.eof_reported {
            return self.duration_ms;
        }
        if self.audio_live
            && let Some(a) = &self.audio
        {
            let decoded = a.position_ms();
            let buffered_ms =
                self.output.buffered_frames() as u64 * 1000 / u64::from(CANONICAL_SAMPLE_RATE);
            return decoded.saturating_sub(buffered_ms).min(self.duration_ms);
        }
        let elapsed = self
            .clock_started_ms
            .map(|s| ((self.now_ms().saturating_sub(s)) as f64 * f64::from(self.rate)) as u64)
            .unwrap_or(0);
        (self.clock_base_ms + elapsed).min(self.duration_ms)
    }

    fn arm_guard(&mut self) {
        self.guard_until_ms = self.now_ms() + (RECOVERY_S * 1000.0) as u64;
    }

    /// Starts playing from the current position.
    pub fn play(&mut self) {
        if self.playing {
            return;
        }
        if self.eof_reported {
            self.seek_ms(0);
        }
        self.playing = true;
        self.eof_reported = false;
        let pos = self.position_ms();
        if let Some(a) = &self.audio {
            a.send(AudioCmd::Play);
        }
        self.clock_base_ms = pos;
        self.clock_started_ms = Some(self.now_ms());
        self.video.send(VideoCmd::Play);
        self.arm_guard();
    }

    /// Pauses at the current position.
    pub fn pause(&mut self) {
        if !self.playing {
            return;
        }
        let pos = self.position_ms();
        self.playing = false;
        if let Some(a) = &self.audio {
            a.send(AudioCmd::Pause);
        }
        self.clock_base_ms = pos;
        self.clock_started_ms = None;
        self.video.send(VideoCmd::Pause);
        if self.holding {
            self.output.resume();
            self.holding = false;
        }
    }

    /// Moves to `ms` (clamped to the duration).
    pub fn seek_ms(&mut self, ms: u64) {
        let ms = ms.min(self.duration_ms);
        self.eof_reported = false;
        if let Some(a) = &self.audio {
            a.send(AudioCmd::Seek(ms));
        }
        self.queue.clear();
        self.clock_base_ms = ms;
        self.clock_started_ms = self.playing.then(|| self.now_ms());
        self.video.send(VideoCmd::Seek(ms));
        self.last_frame_pts = None;
        if self.holding {
            self.output.resume();
            self.holding = false;
        }
        self.arm_guard();
    }

    /// Changes the rate, restarting from the current position.
    pub fn set_rate(&mut self, rate: f32, preserve_pitch: bool) {
        let pos = self.position_ms();
        self.rate = rate.clamp(0.25, 4.0);
        self.preserve_pitch = preserve_pitch;
        if let Some(a) = &self.audio {
            a.send(AudioCmd::Rate {
                rate: self.rate,
                preserve_pitch,
            });
            a.send(AudioCmd::Seek(pos));
        }
        self.queue.clear();
        self.clock_base_ms = pos;
        self.clock_started_ms = self.playing.then(|| self.now_ms());
        self.arm_guard();
    }

    /// Master volume.
    pub fn set_master_percent(&mut self, percent: u8) {
        self.master_percent = percent.min(100);
        self.push_mix();
    }

    /// Per-track gain and mute.
    pub fn set_source(&mut self, id: &str, gain_percent: Option<u8>, muted: Option<bool>) {
        if let Some((_, s)) = self.sources.iter_mut().find(|(i, _)| i == id) {
            if let Some(g) = gain_percent {
                s.gain_percent = g.min(100);
            }
            if let Some(m) = muted {
                s.muted = m;
            }
        }
        self.push_mix();
    }

    /// The tracks and their states.
    pub fn sources(&self) -> &[(String, SourceState)] {
        &self.sources
    }

    fn push_mix(&self) {
        if let Some(a) = &self.audio {
            let gains = self
                .sources
                .iter()
                .map(|(_, s)| {
                    if s.muted {
                        0.0
                    } else {
                        f32::from(s.gain_percent) / 100.0
                    }
                })
                .collect();
            a.send(AudioCmd::Mix {
                gains,
                master: f32::from(self.master_percent) / 100.0,
            });
        }
    }

    /// The newest frame whose time has come (at most one per call).
    pub fn take_frame(&mut self) -> Option<Frame> {
        let position = self.position_ms();
        let mut frames = self.video.shared.frames.lock().ok()?;
        let mut chosen: Option<Frame> = None;
        if !self.playing {
            // Paused: the seek target (or whatever is queued) shows at once.
            if let Some(f) = frames.pop_front() {
                frames.clear();
                chosen = Some(f);
            }
        } else {
            while let Some(f) = frames.front() {
                if f.pts_ms <= position + 16 {
                    chosen = frames.pop_front();
                } else {
                    break;
                }
            }
        }
        drop(frames);
        self.video.shared.space.notify_all();
        if let Some(f) = &chosen {
            self.last_frame_pts = Some(f.pts_ms);
        }
        chosen
    }

    /// Runs the sync rule, collects worker events, reports EOF.
    pub fn tick(&mut self) {
        if let Some(null) = self.output.as_null_mut() {
            null.poll();
        }
        while let Ok(e) = self.events_rx.try_recv() {
            if matches!(e, PlaybackEvent::AudioFailed(_)) && self.audio_live {
                // Keep going on the video clock from where audio left off.
                let pos = self.position_ms();
                self.audio_live = false;
                self.clock_base_ms = pos;
                self.clock_started_ms = self.playing.then(|| self.now_ms());
                self.output.stop();
            }
            self.events.push(e);
        }
        if let Some((rx, _)) = &self.proxy
            && let Ok(p) = rx.try_recv()
        {
            if p != self.source_path {
                let pos = self.position_ms();
                self.video.send(VideoCmd::Reopen(p, pos));
                self.events.push(PlaybackEvent::ProxyReady);
            }
            self.proxy = None;
        }
        if !self.playing {
            return;
        }
        if self.audio_live
            && let Some(a) = &self.audio
        {
            if a.shared.eof.load(Ordering::Relaxed) && self.queue.frames() == 0 {
                self.finish();
                return;
            }
            let audio_ms = self.position_ms() as i64;
            let video_ms = self.last_frame_pts.map(|p| p as i64).unwrap_or(audio_ms);
            let guard = self.now_ms() < self.guard_until_ms;
            match decide(audio_ms, video_ms, self.holding, guard) {
                SyncDecision::Hold => {
                    self.output.suspend();
                    self.holding = true;
                }
                SyncDecision::Resume => {
                    self.output.resume();
                    self.holding = false;
                }
                SyncDecision::Reseek(v) => {
                    a.send(AudioCmd::Seek(v));
                    self.queue.clear();
                    self.arm_guard();
                    self.holding = false;
                }
                SyncDecision::None => {}
            }
        } else if self.video.shared.eof.load(Ordering::Relaxed)
            && self.position_ms() >= self.duration_ms
        {
            self.finish();
        }
    }

    fn finish(&mut self) {
        self.playing = false;
        self.clock_base_ms = self.duration_ms;
        self.clock_started_ms = None;
        if let Some(a) = &self.audio {
            a.send(AudioCmd::Pause);
        }
        self.video.send(VideoCmd::Pause);
        if !self.eof_reported {
            self.eof_reported = true;
            self.events.push(PlaybackEvent::Eof);
        }
    }

    /// Events since the last call.
    pub fn events(&mut self) -> Vec<PlaybackEvent> {
        std::mem::take(&mut self.events)
    }

    /// Stops everything.
    pub fn close(mut self) {
        self.playing = false;
        self.output.stop();
        if let Some((_, cancel)) = &self.proxy {
            cancel.store(true, Ordering::Relaxed);
        }
        self.audio = None;
    }
}

/// The best audio stream when there is no manifest.
fn default_audio_spec(path: &Path) -> Option<SourceSpec> {
    fthrust_media::playback::best_audio_stream(path).map(|container_index| SourceSpec {
        source_id: "default".into(),
        container_index,
    })
}
