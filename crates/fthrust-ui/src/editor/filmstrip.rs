//! Timeline filmstrip frames (`_load_timeline_thumbnails_if_idle`): a
//! worker decodes `count` evenly spaced 180×100 frames; finished strips
//! stay in a small process-wide LRU (8 strips, 20 MB) so returning to a
//! parked editor does not decode again.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use egui::ColorImage;
use fthrust_media::playback::video::VideoDecoder;

/// Frame size.
pub const FRAME_W: u32 = 180;
/// Frame size.
pub const FRAME_H: u32 = 100;
const CACHE_ENTRIES: usize = 8;
const CACHE_BYTES: usize = 20 * 1024 * 1024;

/// `count = min(120, max(24, 24 + duration_s / 60))`.
pub fn frame_count(duration_ms: u64) -> usize {
    (24 + (duration_ms / 1000 / 60) as usize).clamp(24, 120)
}

type Strip = Arc<Vec<ColorImage>>;
/// `(path, size, mtime_ns)`.
type Key = (PathBuf, u64, i128);

/// `(path, size, mtime_ns)` → strip, most recent last.
static CACHE: Mutex<Vec<(Key, Strip)>> = Mutex::new(Vec::new());

fn cache_key(path: &Path) -> Option<Key> {
    let id = fthrust_core::media_identity::file_identity(path).ok()?;
    Some((path.to_path_buf(), id.size, id.mtime_ns))
}

fn cache_get(key: &Key) -> Option<Strip> {
    let mut c = CACHE.lock().ok()?;
    let i = c.iter().position(|(k, _)| k == key)?;
    let entry = c.remove(i);
    let strip = entry.1.clone();
    c.push(entry);
    Some(strip)
}

fn cache_put(key: Key, strip: Strip) {
    let Ok(mut c) = CACHE.lock() else {
        return;
    };
    c.retain(|(k, _)| *k != key);
    c.push((key, strip));
    let bytes = |c: &Vec<(_, Strip)>| -> usize {
        c.iter()
            .map(|(_, s)| s.iter().map(|f| f.pixels.len() * 4).sum::<usize>())
            .sum()
    };
    while c.len() > CACHE_ENTRIES || (c.len() > 1 && bytes(&c) > CACHE_BYTES) {
        c.remove(0);
    }
}

/// Loading state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilmstripState {
    /// Nothing requested (or cancelled).
    Idle,
    /// The worker is decoding.
    Loading,
    /// Frames are available.
    Ready,
}

/// One editor's filmstrip.
pub struct Filmstrip {
    rx: Option<Receiver<Option<Strip>>>,
    cancel: Arc<AtomicBool>,
    ready: Option<Strip>,
    taken: bool,
}

impl Default for Filmstrip {
    fn default() -> Self {
        Filmstrip {
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            ready: None,
            taken: false,
        }
    }
}

impl Filmstrip {
    /// Current state (polls the worker).
    pub fn state(&mut self) -> FilmstripState {
        self.poll();
        if self.ready.is_some() {
            FilmstripState::Ready
        } else if self.rx.is_some() {
            FilmstripState::Loading
        } else {
            FilmstripState::Idle
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else {
            return;
        };
        match rx.try_recv() {
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) | Ok(None) => self.rx = None,
            Ok(Some(strip)) => {
                self.rx = None;
                self.ready = Some(strip);
                self.taken = false;
            }
        }
    }

    /// Starts decoding `count` frames of `path` (a cached strip is ready at
    /// once).
    pub fn request(&mut self, path: &Path, duration_ms: u64, count: usize) {
        self.cancel();
        let key = cache_key(path);
        if let Some(strip) = key.as_ref().and_then(cache_get) {
            self.ready = Some(strip);
            self.taken = false;
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        self.rx = Some(rx);
        let path = path.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("filmstrip".into())
            .spawn(move || {
                let strip = decode(&path, duration_ms, count, &cancel).map(Arc::new);
                if let (Some(s), Some(k)) = (&strip, key) {
                    cache_put(k, s.clone());
                }
                let _ = tx.send(strip);
            });
        if spawned.is_err() {
            self.rx = None;
        }
    }

    /// Stops a running decode (finished frames stay).
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.rx = None;
    }

    /// The frames, once, for upload.
    pub fn take_ready(&mut self) -> Option<Vec<ColorImage>> {
        self.poll();
        if self.taken {
            return None;
        }
        let strip = self.ready.as_ref()?;
        self.taken = true;
        Some(strip.as_ref().clone())
    }
}

fn decode(
    path: &Path,
    duration_ms: u64,
    count: usize,
    cancel: &AtomicBool,
) -> Option<Vec<ColorImage>> {
    let mut dec = VideoDecoder::open(path).ok()?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let at = duration_ms * i as u64 / count.max(1) as u64;
        dec.seek(at).ok()?;
        let frame = dec.next_frame().ok()??;
        let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)?;
        let small = image::imageops::resize(
            &img,
            FRAME_W,
            FRAME_H,
            image::imageops::FilterType::Triangle,
        );
        out.push(ColorImage::from_rgba_unmultiplied(
            [FRAME_W as usize, FRAME_H as usize],
            small.as_raw(),
        ));
    }
    Some(out)
}
