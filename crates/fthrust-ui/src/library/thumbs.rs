//! Thumbnail loading for the library: a two-worker pool over the media
//! crate's cache, results delivered on a channel, and a bounded LRU of
//! live egui textures (one per visible card, at most 96).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use egui::TextureHandle;
use fthrust_core::thumbnails::MetaV3;
use fthrust_media::thumbnail::library::{CachedThumbnail, image_thumbnail, load_or_build};

/// How many card textures stay alive.
pub const MAX_LIVE_TEXTURES: usize = 96;
/// Longest thumbnail job queue.
pub const MAX_QUEUE: usize = 128;
/// How long a failed thumbnail waits before the next try (a clip that changed
/// is tried at once).
pub const FAILED_RETRY_S: f64 = 30.0;

/// Clips whose thumbnail could not be made, with the file they failed on and
/// the time to try again.
#[derive(Debug, Default)]
pub struct ThumbFailures {
    entries: HashMap<PathBuf, (String, f64)>,
}

impl ThumbFailures {
    /// Records a failure of `path` as the file was (`fingerprint`) at `now`.
    pub fn insert(&mut self, path: PathBuf, fingerprint: String, now: f64) {
        self.entries
            .insert(path, (fingerprint, now + FAILED_RETRY_S));
    }

    /// `path` has a recorded failure (expired or not).
    pub fn contains(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }

    /// Whether `path` is still not worth a try: the file is what it was when
    /// it failed, and the retry time has not come. An entry that no longer
    /// holds is dropped.
    pub fn blocks(&mut self, path: &Path, fingerprint: &str, now: f64) -> bool {
        match self.entries.get(path) {
            None => false,
            Some((was, retry_at)) if was == fingerprint && now < *retry_at => true,
            Some(_) => {
                self.entries.remove(path);
                false
            }
        }
    }

    /// Forgets `path`.
    pub fn remove(&mut self, path: &Path) {
        self.entries.remove(path);
    }

    /// Recorded failures.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// No failures.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A live texture and the metadata it came with.
pub struct ThumbEntry {
    /// The GPU texture.
    pub texture: TextureHandle,
    /// Probed metadata (videos).
    pub meta: Option<MetaV3>,
}

/// LRU of live textures.
#[derive(Default)]
pub struct ThumbTextures {
    entries: Vec<(PathBuf, ThumbEntry)>,
    order: VecDeque<PathBuf>,
}

impl ThumbTextures {
    /// Inserts (or replaces) a texture, evicting the least recently used
    /// beyond [`MAX_LIVE_TEXTURES`].
    pub fn insert(&mut self, path: PathBuf, texture: TextureHandle, meta: Option<MetaV3>) {
        self.entries.retain(|(p, _)| *p != path);
        self.order.retain(|p| *p != path);
        self.entries
            .push((path.clone(), ThumbEntry { texture, meta }));
        self.order.push_back(path);
        while self.entries.len() > MAX_LIVE_TEXTURES {
            if let Some(old) = self.order.pop_front() {
                self.entries.retain(|(p, _)| *p != old);
            }
        }
    }

    /// The entry for `path`, marking it recently used.
    pub fn get(&mut self, path: &Path) -> Option<&ThumbEntry> {
        if self.entries.iter().any(|(p, _)| p == path) {
            self.order.retain(|p| p != path);
            self.order.push_back(path.to_path_buf());
        }
        self.entries.iter().find(|(p, _)| p == path).map(|(_, e)| e)
    }

    /// Whether a texture is live without touching the order.
    pub fn contains(&self, path: &Path) -> bool {
        self.entries.iter().any(|(p, _)| p == path)
    }

    /// Drops one texture.
    pub fn remove(&mut self, path: &Path) {
        self.entries.retain(|(p, _)| p != path);
        self.order.retain(|p| p != path);
    }

    /// Live texture count.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// No textures.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drops everything.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

/// A finished job.
#[derive(Debug)]
pub struct ThumbResult {
    /// The clip.
    pub path: PathBuf,
    /// The pool generation the job belonged to.
    pub generation: u64,
    /// The record fingerprint the job was made for.
    pub fingerprint: String,
    /// The decoded thumbnail (video: JPEG path + meta; image: pixels).
    pub image: Option<ThumbImage>,
    /// Job time.
    pub elapsed_ms: u64,
}

/// What a job produced.
#[derive(Debug)]
pub enum ThumbImage {
    /// A cached video poster and its pixels, decoded by the worker (the UI
    /// thread only uploads them). Without pixels the clip's details are
    /// still good but its poster could not be decoded, even rebuilt.
    Video(CachedThumbnail, Option<image::RgbaImage>),
    /// A decoded screenshot (RGBA, ≤ 640×360).
    Image(image::RgbaImage),
}

struct Job {
    path: PathBuf,
    generation: u64,
    fingerprint: String,
    is_video: bool,
    cancel: Arc<AtomicBool>,
}

struct Queue {
    jobs: VecDeque<Job>,
    inflight: Vec<PathBuf>,
}

/// The worker pool.
pub struct ThumbPool {
    queue: Arc<(Mutex<Queue>, std::sync::Condvar)>,
    results: Receiver<ThumbResult>,
    cancel: Mutex<Arc<AtomicBool>>,
    generation: Mutex<u64>,
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
    repaint: Arc<Mutex<Option<egui::Context>>>,
}

impl ThumbPool {
    /// Starts two workers writing into `cache_dir`.
    pub fn start(cache_dir: PathBuf) -> ThumbPool {
        ThumbPool::start_with(cache_dir, 2)
    }

    /// Starts `workers` workers; with none, jobs only queue (tests).
    pub fn start_with(cache_dir: PathBuf, workers: usize) -> ThumbPool {
        let queue = Arc::new((
            Mutex::new(Queue {
                jobs: VecDeque::new(),
                inflight: Vec::new(),
            }),
            std::sync::Condvar::new(),
        ));
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let repaint: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));
        let threads = (0..workers)
            .map(|i| {
                let queue = queue.clone();
                let tx = tx.clone();
                let stop = stop.clone();
                let cache_dir = cache_dir.clone();
                let repaint = repaint.clone();
                std::thread::Builder::new()
                    .name(format!("fthrust-thumbs-{i}"))
                    .spawn(move || worker(queue, tx, stop, cache_dir, repaint))
                    .expect("spawn thumbnail worker")
            })
            .collect();
        ThumbPool {
            queue,
            results: rx,
            cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
            generation: Mutex::new(0),
            stop,
            threads,
            repaint,
        }
    }

    /// Wakes `ctx` whenever a result is ready (the UI only repaints on events).
    pub fn set_repaint(&self, ctx: egui::Context) {
        if let Ok(mut r) = self.repaint.lock() {
            *r = Some(ctx);
        }
    }

    /// The current generation.
    pub fn generation(&self) -> u64 {
        *self.generation.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Cancels every queued and running job and starts a new generation.
    pub fn cancel_generation(&self) -> u64 {
        if let Ok(c) = self.cancel.lock() {
            c.store(true, Ordering::Relaxed);
        }
        if let Ok(mut c) = self.cancel.lock() {
            *c = Arc::new(AtomicBool::new(false));
        }
        let (q, _) = &*self.queue;
        if let Ok(mut q) = q.lock() {
            q.jobs.clear();
        }
        let mut g = self.generation.lock().unwrap_or_else(|e| e.into_inner());
        *g += 1;
        *g
    }

    /// Queues a job unless it is already queued or running or the queue is
    /// full; returns whether it was queued.
    pub fn request(&self, path: &Path, fingerprint: &str, is_video: bool) -> bool {
        let generation = self.generation();
        let cancel = self
            .cancel
            .lock()
            .map(|c| c.clone())
            .unwrap_or_else(|e| e.into_inner().clone());
        let (q, cv) = &*self.queue;
        let mut q = q.lock().unwrap_or_else(|e| e.into_inner());
        if q.inflight.iter().any(|p| p == path) || q.jobs.iter().any(|j| j.path == path) {
            return false;
        }
        if q.jobs.len() >= MAX_QUEUE {
            return false;
        }
        q.jobs.push_back(Job {
            path: path.to_path_buf(),
            generation,
            fingerprint: fingerprint.to_owned(),
            is_video,
            cancel,
        });
        cv.notify_one();
        true
    }

    /// Drops the queued jobs of clips outside `keep` (the cards that are
    /// built now) and returns their paths: a job for a card that scrolled
    /// away would only delay the thumbnails of the cards in view. Running
    /// jobs finish.
    pub fn retain(&self, keep: &[PathBuf]) -> Vec<PathBuf> {
        let (q, _) = &*self.queue;
        let mut q = q.lock().unwrap_or_else(|e| e.into_inner());
        let mut dropped = Vec::new();
        q.jobs.retain(|j| {
            let wanted = keep.contains(&j.path);
            if !wanted {
                dropped.push(j.path.clone());
            }
            wanted
        });
        dropped
    }

    /// Finished jobs.
    pub fn results(&self) -> &Receiver<ThumbResult> {
        &self.results
    }

    /// Queued plus running jobs.
    pub fn inflight(&self) -> usize {
        let (q, _) = &*self.queue;
        q.lock()
            .map(|q| q.jobs.len() + q.inflight.len())
            .unwrap_or(0)
    }
}

impl Drop for ThumbPool {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(c) = self.cancel.lock() {
            c.store(true, Ordering::Relaxed);
        }
        let (_, cv) = &*self.queue;
        cv.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn decode_jpeg(jpg: &Path) -> Option<image::RgbaImage> {
    Some(image::open(jpg).ok()?.to_rgba8())
}

/// A video's poster and details: from the cache, built on a miss. A cached
/// JPEG that does not decode is damaged: it is deleted and built again, once.
/// If the rebuilt one does not decode either, the details (resolution, frame
/// rate, length) are still returned, without pixels, and the bad file is gone
/// so that the next try starts clean. `None` when the clip itself cannot be
/// read (or the job was cancelled).
fn video_poster(
    cache_dir: &Path,
    path: &Path,
    cancel: &AtomicBool,
    decode: &dyn Fn(&Path) -> Option<image::RgbaImage>,
) -> Option<ThumbImage> {
    let cached = load_or_build(cache_dir, path, cancel).ok().flatten()?;
    if let Some(rgba) = decode(&cached.jpg) {
        return Some(ThumbImage::Video(cached, Some(rgba)));
    }
    let _ = std::fs::remove_file(&cached.jpg);
    match load_or_build(cache_dir, path, cancel).ok().flatten() {
        Some(fresh) => match decode(&fresh.jpg) {
            Some(rgba) => Some(ThumbImage::Video(fresh, Some(rgba))),
            None => {
                let _ = std::fs::remove_file(&fresh.jpg);
                Some(ThumbImage::Video(fresh, None))
            }
        },
        // The clip's details were fine; only the poster is missing.
        None => Some(ThumbImage::Video(cached, None)),
    }
}

fn worker(
    queue: Arc<(Mutex<Queue>, std::sync::Condvar)>,
    tx: Sender<ThumbResult>,
    stop: Arc<AtomicBool>,
    cache_dir: PathBuf,
    repaint: Arc<Mutex<Option<egui::Context>>>,
) {
    loop {
        let job = {
            let (q, cv) = &*queue;
            let mut q = q.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(job) = q.jobs.pop_front() {
                    q.inflight.push(job.path.clone());
                    break job;
                }
                q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let started = std::time::Instant::now();
        let image = if job.cancel.load(Ordering::Relaxed) {
            None
        } else if job.is_video {
            video_poster(&cache_dir, &job.path, &job.cancel, &decode_jpeg)
        } else {
            image_thumbnail(&job.path).ok().map(ThumbImage::Image)
        };
        {
            let (q, _) = &*queue;
            if let Ok(mut q) = q.lock() {
                q.inflight.retain(|p| *p != job.path);
            }
        }
        if job.cancel.load(Ordering::Relaxed) {
            continue;
        }
        let _ = tx.send(ThumbResult {
            path: job.path,
            generation: job.generation,
            fingerprint: job.fingerprint,
            image,
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
        if let Ok(r) = repaint.lock()
            && let Some(ctx) = r.as_ref()
        {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fthrust_core::thumbnails::paths_for;
    use fthrust_media::thumbnail::library::cache_key;

    /// A clip file that was never decodable and a cache entry for it that
    /// holds good details and a "poster" that is not a JPEG.
    fn damaged_entry(dir: &Path) -> (PathBuf, PathBuf, MetaV3) {
        let clip = dir.join("clip.mp4");
        std::fs::write(&clip, b"not a video").unwrap();
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let paths = paths_for(&cache, &cache_key(&clip).unwrap());
        let meta = MetaV3 {
            duration: 12.5,
            width: 1920,
            height: 1080,
            fps: 60.0,
            video_bitrate: 25_000_000,
            total_bitrate: 25_200_000,
        };
        std::fs::write(&paths.meta, meta.to_text()).unwrap();
        std::fs::write(&paths.jpg, b"damaged").unwrap();
        (clip, cache, meta)
    }

    #[test]
    fn a_poster_that_does_not_decode_keeps_the_details_and_is_deleted() {
        let d = tempfile::tempdir().unwrap();
        let (clip, cache, meta) = damaged_entry(d.path());
        let result = video_poster(&cache, &clip, &AtomicBool::new(false), &decode_jpeg);
        match result {
            Some(ThumbImage::Video(cached, None)) => {
                assert_eq!(cached.meta, meta, "the details survive");
            }
            other => panic!("expected the details without pixels, got {other:?}"),
        }
        let paths = paths_for(&cache, &cache_key(&clip).unwrap());
        assert!(!paths.jpg.exists(), "the damaged poster was deleted");
        assert!(paths.meta.exists(), "the details stay cached");
    }

    #[test]
    fn a_damaged_poster_is_decoded_once_and_then_rebuilt_not_decoded_again() {
        let d = tempfile::tempdir().unwrap();
        let (clip, cache, _) = damaged_entry(d.path());
        // The first decode fails, so the poster is dropped and looked up
        // again; the clip is not a video, so there is nothing to rebuild it
        // from and no second decode happens.
        let calls = std::cell::Cell::new(0);
        let result = video_poster(&cache, &clip, &AtomicBool::new(false), &|_| {
            calls.set(calls.get() + 1);
            None
        });
        assert!(matches!(result, Some(ThumbImage::Video(_, None))));
        assert_eq!(calls.get(), 1, "one decode of the damaged file");
    }
}
