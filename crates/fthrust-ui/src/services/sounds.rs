//! Notification sounds (`capture_card.py` `_play_sound` + `SoundPlayback`):
//! six cues decoded once, played through a PipeWire playback stream at the
//! per-cue volume times the master volume.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fthrust_core::settings::SettingsStore;
use fthrust_core::theme::ThemeStore;
use fthrust_media::audio_decode::{Pcm, decode_to_pcm};

use super::CardKind;

/// `(key, bundled file, volume setting)`.
pub const CUES: [(&str, &str, &str); 6] = [
    ("clip_captured", "clip_captured.wav", "sound_volume_clip"),
    (
        "screenshot_captured",
        "screenshot_saved.wav",
        "sound_volume_screenshot",
    ),
    ("error", "error.wav", "sound_volume_error"),
    ("startup", "startup.wav", "sound_volume_startup"),
    (
        "upload_successful",
        "upload_successful.wav",
        "sound_volume_upload_successful",
    ),
    (
        "upload_failed",
        "upload_failed.wav",
        "sound_volume_upload_failed",
    ),
];

/// The cue a card plays.
pub fn sound_key_for(kind: &CardKind) -> Option<&'static str> {
    match kind {
        CardKind::Clip { .. } => Some("clip_captured"),
        CardKind::Screenshot => Some("screenshot_captured"),
        CardKind::Error { .. } => Some("error"),
        CardKind::Upload { .. } => Some("upload_successful"),
        CardKind::UploadFailed { .. } => Some("upload_failed"),
        CardKind::Startup => Some("startup"),
        CardKind::RecordingSaved { .. } | CardKind::Prompt { .. } | CardKind::Capturing { .. } => {
            None
        }
    }
}

fn clamp_volume(v: i64) -> i64 {
    v.clamp(0, 100)
}

/// `round(volume · master / 100)`, ignoring the enabled gate.
pub fn mixed_volume(settings: &SettingsStore, key: &str) -> Option<u8> {
    let (_, _, vkey) = CUES.iter().find(|(k, _, _)| *k == key)?;
    let volume = clamp_volume(settings.get_i64(vkey).unwrap_or(100));
    let master = clamp_volume(settings.get_i64("sound_volume_master").unwrap_or(100));
    Some(((volume * master) as f64 / 100.0).round() as u8)
}

/// The volume to play `key` at, `None` when sounds are off.
pub fn effective_volume(settings: &SettingsStore, key: &str) -> Option<u8> {
    if !settings
        .get_bool("notification_sounds_enabled")
        .unwrap_or(true)
    {
        return None;
    }
    mixed_volume(settings, key)
}

/// The theme's custom file for `key`, else the bundled cue.
pub fn resolve_sound_path(theme: &ThemeStore, assets: &Path, key: &str) -> Option<PathBuf> {
    let (_, file, _) = CUES.iter().find(|(k, _, _)| *k == key)?;
    if let Some(custom) = theme.custom_sound_path(key)
        && custom.exists()
    {
        return Some(custom);
    }
    Some(assets.join("sounds").join(file))
}

/// Where decoded cues go.
pub trait Sink: Send + Sync {
    /// Plays `pcm` at `gain` (0–1), replacing whatever is playing.
    fn play(&self, pcm: Arc<Pcm>, gain: f32);
    /// Stops playback.
    fn stop(&self);
}

/// A sink that drops everything (PipeWire unavailable).
pub struct NullSink;

impl Sink for NullSink {
    fn play(&self, _pcm: Arc<Pcm>, _gain: f32) {}

    fn stop(&self) {}
}

type Cache = Arc<Mutex<HashMap<PathBuf, Arc<Pcm>>>>;

/// The player.
pub struct SoundPlayer {
    sink: Arc<dyn Sink>,
    assets: PathBuf,
    cache: Cache,
}

impl SoundPlayer {
    /// A player over `sink`.
    pub fn with_sink(sink: Arc<dyn Sink>, assets: PathBuf) -> Self {
        SoundPlayer {
            sink,
            assets,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// A player over PipeWire (or a silent one when PipeWire is unavailable).
    pub fn start(assets: PathBuf) -> Self {
        let sink: Arc<dyn Sink> = match pipewire_sink::PipeWireSink::start() {
            Ok(s) => Arc::new(s),
            Err(e) => {
                tracing::warn!(error = %e, "notification sounds unavailable");
                Arc::new(NullSink)
            }
        };
        Self::with_sink(sink, assets)
    }

    fn decode(&self, path: &Path) -> Option<Arc<Pcm>> {
        if let Some(p) = self.cache.lock().ok().and_then(|c| c.get(path).cloned()) {
            return Some(p);
        }
        match decode_to_pcm(path) {
            Ok(pcm) => {
                let pcm = Arc::new(pcm);
                if let Ok(mut c) = self.cache.lock() {
                    c.insert(path.to_path_buf(), pcm.clone());
                }
                Some(pcm)
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "sound decode failed");
                None
            }
        }
    }

    /// Decodes the six cues now (theme custom sounds first).
    pub fn preload(&mut self, theme: &ThemeStore) {
        for (key, _, _) in CUES {
            if let Some(p) = resolve_sound_path(theme, &self.assets, key) {
                let _ = self.decode(&p);
            }
        }
    }

    /// Decodes the six cues on a background thread.
    pub fn preload_async(&self, theme: &ThemeStore) {
        let paths: Vec<PathBuf> = CUES
            .iter()
            .filter_map(|(key, _, _)| resolve_sound_path(theme, &self.assets, key))
            .collect();
        let cache = self.cache.clone();
        std::thread::Builder::new()
            .name("fthrust-sound-preload".into())
            .spawn(move || {
                for p in paths {
                    if let Ok(pcm) = decode_to_pcm(&p)
                        && let Ok(mut c) = cache.lock()
                    {
                        c.insert(p, Arc::new(pcm));
                    }
                }
            })
            .ok();
    }

    fn play_at(&self, theme: &ThemeStore, key: &str, volume: u8) {
        let Some(path) = resolve_sound_path(theme, &self.assets, key) else {
            return;
        };
        if let Some(pcm) = self.decode(&path) {
            self.sink.play(pcm, f32::from(volume) / 100.0);
        }
    }

    /// Plays `key` if sounds are enabled.
    pub fn play(&self, settings: &SettingsStore, theme: &ThemeStore, key: &str) {
        if let Some(v) = effective_volume(settings, key) {
            self.play_at(theme, key, v);
        }
    }

    /// Plays `key` regardless of the enabled gate (the Customize page).
    pub fn preview(&self, settings: &SettingsStore, theme: &ThemeStore, key: &str) {
        if let Some(v) = mixed_volume(settings, key) {
            self.play_at(theme, key, v);
        }
    }

    /// Stops playback.
    pub fn stop(&self) {
        self.sink.stop();
    }
}

mod pipewire_sink {
    use std::sync::{Arc, Mutex};

    use fthrust_media::audio_decode::Pcm;
    use pipewire as pw;

    use super::Sink;

    const RATE: u32 = 48_000;
    const CHANNELS: usize = 2;

    struct Cue {
        pcm: Arc<Pcm>,
        pos: usize,
        gain: f32,
    }

    #[derive(Default)]
    struct Shared {
        cue: Mutex<Option<Cue>>,
    }

    enum Msg {
        Quit,
    }

    /// A PipeWire playback stream on its own thread.
    pub struct PipeWireSink {
        shared: Arc<Shared>,
        quit: Option<pw::channel::Sender<Msg>>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl PipeWireSink {
        pub fn start() -> Result<Self, String> {
            let shared = Arc::new(Shared::default());
            let (tx, rx) = pw::channel::channel::<Msg>();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let thread = {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name("fthrust-sound".into())
                    .spawn(move || run(shared, rx, ready_tx))
                    .map_err(|e| e.to_string())?
            };
            match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(Ok(())) => Ok(PipeWireSink {
                    shared,
                    quit: Some(tx),
                    thread: Some(thread),
                }),
                Ok(Err(e)) => Err(e),
                Err(_) => Err("PipeWire did not answer".into()),
            }
        }
    }

    impl Sink for PipeWireSink {
        fn play(&self, pcm: Arc<Pcm>, gain: f32) {
            if let Ok(mut c) = self.shared.cue.lock() {
                *c = Some(Cue { pcm, pos: 0, gain });
            }
        }

        fn stop(&self) {
            if let Ok(mut c) = self.shared.cue.lock() {
                *c = None;
            }
        }
    }

    impl Drop for PipeWireSink {
        fn drop(&mut self) {
            if let Some(q) = self.quit.take() {
                let _ = q.send(Msg::Quit);
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
                RATE as i32
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

    fn run(
        shared: Arc<Shared>,
        quit_rx: pw::channel::Receiver<Msg>,
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
            *pw::keys::MEDIA_ROLE => "Notification",
            *pw::keys::NODE_NAME => "FTHRust notifications",
            *pw::keys::APP_NAME => "FTHR Clips",
        };
        let stream = match pw::stream::StreamRc::new(core.clone(), "fthrust-sounds", props) {
            Ok(s) => s,
            Err(e) => return fail(format!("stream: {e}")),
        };
        let listener = stream
            .add_local_listener_with_user_data(())
            .process({
                let shared = shared.clone();
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
                        let max_frames = bytes.len() / (4 * CHANNELS);
                        let frames = if requested > 0 {
                            requested.min(max_frames)
                        } else {
                            max_frames.min(1024)
                        };
                        let mut cue = shared.cue.lock().unwrap_or_else(|p| p.into_inner());
                        let mut written = 0usize;
                        if let Some(c) = cue.as_mut() {
                            let samples = &c.pcm.samples;
                            let want = frames * CHANNELS;
                            let avail = samples.len().saturating_sub(c.pos);
                            let n = want.min(avail);
                            for (i, s) in samples[c.pos..c.pos + n].iter().enumerate() {
                                let v = (s * c.gain).to_le_bytes();
                                bytes[i * 4..i * 4 + 4].copy_from_slice(&v);
                            }
                            c.pos += n;
                            written = n;
                            if c.pos >= samples.len() {
                                *cue = None;
                            }
                        }
                        for b in &mut bytes[written * 4..frames * CHANNELS * 4] {
                            *b = 0;
                        }
                        let chunk = data.chunk_mut();
                        *chunk.offset_mut() = 0;
                        *chunk.stride_mut() = (4 * CHANNELS) as i32;
                        *chunk.size_mut() = (frames * CHANNELS * 4) as u32;
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
        let _quit = quit_rx.attach(main_loop.loop_(), {
            let main_loop = main_loop.clone();
            move |Msg::Quit| main_loop.quit()
        });
        main_loop.run();
        let _ = stream.disconnect();
    }
}
