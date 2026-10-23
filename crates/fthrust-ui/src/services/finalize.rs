//! Post-save overlays (`_apply_image_overlay`, `_apply_camera_overlay`):
//! after the engine commits a clip, image layers and the webcam window are
//! burned in one re-encode on a worker thread, one clip at a time. The
//! clip stays `Finalizing` until then; every failure keeps the base clip
//! and becomes a readiness warning.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fthrust_core::finalize::{self as plan, ImageLayer, Rect};
use fthrust_media::camera::CameraSnapshot;
use fthrust_media::export::{ExportState, Job, Quality};

use crate::shell::error_bar::Level;
use crate::state::AppState;

const CAMERA_UNAVAILABLE: &str = "Camera overlay was requested but the camera was unavailable.";
const CAMERA_NOT_RENDERED: &str =
    "Camera overlay could not be rendered; the base clip was retained.";
const CAMERA_FAILED: &str = "Camera overlay failed; the base clip was retained.";
const PANICKED: &str = "Optional processing failed; the base clip was retained.";
const IMAGE_FAILED: &str = "Image overlay failed; the base clip was retained.";

/// The camera at save time.
#[derive(Clone)]
pub enum CameraUse {
    /// The overlay is off.
    Off,
    /// On, but the camera is not recording.
    Unavailable,
    /// The camera's frames, taken from the ring when the save completed
    /// (`None`: it was recording but had no frames in the window).
    Frames(Option<CameraSnapshot>),
}

/// One clip to finalize.
pub struct FinalizeJob {
    /// The committed clip.
    pub clip: PathBuf,
    /// The save instant (the end of the clip).
    pub end: Instant,
    /// The clip's length.
    pub duration_s: f64,
    /// Enabled image layers.
    pub layers: Vec<ImageLayer>,
    /// The webcam.
    pub camera: CameraUse,
    /// Where the webcam goes.
    pub camera_rect: Rect,
    /// Test hook: the burn panics.
    #[doc(hidden)]
    pub panic_for_tests: bool,
}

/// How one clip ended.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// The clip.
    pub clip: PathBuf,
    /// Readiness warnings, in order.
    pub warnings: Vec<String>,
    /// The camera burn failed (the reference's `CAMERA OVERLAY FAILED`).
    pub camera_failed: bool,
}

/// Whether a save needs finalizing: the camera overlay or any enabled
/// image layer is on.
pub fn needed(state: &AppState) -> bool {
    state.settings.get_bool("camera_enabled").unwrap_or(false)
        || !plan::image_layers(&state.settings).is_empty()
}

/// The finalize worker.
pub struct Finalizer {
    tx: Option<Sender<FinalizeJob>>,
    rx: Receiver<Outcome>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pending: std::cell::Cell<usize>,
}

impl Default for Finalizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Finalizer {
    /// Starts the worker (`fthr-finalize`).
    pub fn new() -> Finalizer {
        let (tx, jobs) = channel::<FinalizeJob>();
        let (out, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("fthr-finalize".into())
            .spawn(move || {
                for job in jobs {
                    if c.load(Ordering::Relaxed) {
                        break;
                    }
                    // A panic must not leave this clip, or the ones queued
                    // after it, Finalizing for the rest of the session.
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        finalize(&job, &c)
                    }))
                    .unwrap_or_else(|_| Outcome {
                        clip: job.clip.clone(),
                        warnings: vec![PANICKED.into()],
                        camera_failed: false,
                    });
                    if out.send(outcome).is_err() {
                        break;
                    }
                }
            })
            .ok();
        Finalizer {
            tx: Some(tx),
            rx,
            cancel,
            thread,
            pending: std::cell::Cell::new(0),
        }
    }

    /// Queues a clip.
    pub fn submit(&self, job: FinalizeJob) {
        if let Some(tx) = &self.tx
            && tx.send(job).is_ok()
        {
            self.pending.set(self.pending.get() + 1);
        }
    }

    /// Finished clips, in submission order.
    pub fn poll(&self) -> Vec<Outcome> {
        let out: Vec<Outcome> = self.rx.try_iter().collect();
        self.pending
            .set(self.pending.get().saturating_sub(out.len()));
        out
    }

    /// Clips are queued or burning (the UI keeps waking to show the result).
    pub fn busy(&self) -> bool {
        self.pending.get() > 0
    }

    /// Cancels the running burn (its base clip stays) and waits up to
    /// `timeout` for the worker.
    pub fn shutdown(&mut self, timeout: Duration) {
        self.cancel.store(true, Ordering::Relaxed);
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let end = Instant::now() + timeout;
            while !t.is_finished() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

impl Drop for Finalizer {
    fn drop(&mut self) {
        self.shutdown(Duration::from_secs(2));
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A `.fthr-finalize-*` directory next to the clip (same filesystem, so
/// the final rename is atomic); removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(clip: &Path) -> std::io::Result<TempDir> {
        let dir = clip.parent().unwrap_or(Path::new("."));
        let p = dir.join(format!(
            ".fthr-finalize-{}",
            fthrust_media::export::random_hex(12)
        ));
        std::fs::create_dir(&p)?;
        Ok(TempDir(p))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn finalize(job: &FinalizeJob, cancel: &Arc<AtomicBool>) -> Outcome {
    assert!(!job.panic_for_tests, "finalize test panic");
    let mut warnings = Vec::new();
    let mut layers = Vec::new();
    for l in &job.layers {
        if !l.path.is_file() {
            warnings.push(format!(
                "Image overlay source is missing: {}",
                file_name(&l.path)
            ));
        } else if !fthrust_media::export::image_decodes(&l.path) {
            warnings.push(format!(
                "Image overlay could not be read: {}",
                file_name(&l.path)
            ));
        } else {
            layers.push(l.clone());
        }
    }
    let outcome = |warnings: Vec<String>, camera_failed| Outcome {
        clip: job.clip.clone(),
        warnings,
        camera_failed,
    };
    let recorder = match &job.camera {
        CameraUse::Off => None,
        CameraUse::Unavailable => {
            warnings.push(CAMERA_UNAVAILABLE.into());
            None
        }
        CameraUse::Frames(None) => {
            warnings.push(CAMERA_NOT_RENDERED.into());
            None
        }
        CameraUse::Frames(Some(s)) => Some(s.clone()),
    };
    if layers.is_empty() && recorder.is_none() {
        return outcome(warnings, false);
    }
    let tmp = match TempDir::new(&job.clip) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "finalize: no temp directory");
            if !layers.is_empty() {
                warnings.push(IMAGE_FAILED.into());
            }
            if recorder.is_some() {
                warnings.push(CAMERA_NOT_RENDERED.into());
            }
            return outcome(warnings, false);
        }
    };
    // The clip's real length: a replay buffer holding less than the
    // requested length makes a shorter clip, which ends at the save.
    let facts = fthrust_media::export::media_facts(&job.clip);
    let duration_s = facts
        .as_ref()
        .ok()
        .map(|f| f.duration_s)
        .filter(|d| *d > 0.0)
        .unwrap_or(job.duration_s);
    let mut camera = None;
    if let Some(r) = recorder {
        let path = tmp.0.join("camera.mkv");
        match r.write(job.end, duration_s, &path) {
            Ok(_) => camera = Some(path),
            Err(e) => {
                tracing::warn!(error = %e, "[Camera] segment not written");
                warnings.push(CAMERA_NOT_RENDERED.into());
            }
        }
    }
    let size = match &facts {
        Ok(f) => (f.width, f.height),
        Err(e) => {
            tracing::warn!(error = %e, "[Post] Could not probe clip dimensions");
            (1920, 1080)
        }
    };
    let Some(graph) = plan::plan(&layers, camera.as_deref(), job.camera_rect, size) else {
        return outcome(warnings, false);
    };
    let ext = job
        .clip
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mp4".into());
    let staged = tmp.0.join(format!("output.{ext}"));
    let export = Job {
        source: job.clip.clone(),
        output: staged.clone(),
        plan: graph,
        quality: Quality::Quality,
        video_bitrate_kbps: None,
        max_rate_kbps: None,
        expected_duration_s: job.duration_s,
        target_size_mb: None,
        output_fps: None,
        decode_from_s: 0.0,
        copy_metadata: true,
        inactivity_s: None,
        fail_first_encoder: false,
    };
    let r = fthrust_media::export::run(&export, cancel, &mut |_| {}, &mut |_| {});
    let burned = r.state == ExportState::Completed
        && std::fs::rename(&staged, &job.clip)
            .map_err(|e| tracing::warn!(error = %e, "finalize: rename"))
            .is_ok();
    if !burned {
        if r.state != ExportState::Cancelled {
            tracing::warn!(detail = %r.detail, "[Post] overlay burn failed");
        }
        if !layers.is_empty() {
            warnings.push(IMAGE_FAILED.into());
        }
        let camera_failed = camera.is_some();
        if camera_failed {
            warnings.push(CAMERA_FAILED.into());
        }
        return outcome(warnings, camera_failed && r.state != ExportState::Cancelled);
    }
    if let Err(e) = fthrust_core::audio_manifest::rebind_after_media_replace(&job.clip) {
        tracing::warn!(error = %e, "finalize: manifest not rebound");
    }
    tracing::info!(clip = %job.clip.display(), layers = layers.len(), camera = camera.is_some(), "overlays applied");
    outcome(warnings, false)
}

/// Applies an outcome: readiness warnings, the clip becomes accessible,
/// `CLIP SAVED WITH WARNING` shows the last warning, `CAMERA OVERLAY
/// FAILED` when the camera burn failed.
pub fn apply(state: &mut AppState, outcome: &Outcome) {
    for w in &outcome.warnings {
        state.readiness.record_warning(&outcome.clip, w);
    }
    state.readiness.complete(&outcome.clip);
    // The card's thumbnail and details were read from the base clip.
    state.thumb_textures.remove(&outcome.clip);
    state.thumb_meta.remove(&outcome.clip);
    if state.library.upsert(&outcome.clip, false) {
        state.library_dirty = true;
    }
    state.library_dirty = true;
    if outcome.camera_failed {
        state.error_bar.push_error(
            "CAMERA OVERLAY FAILED",
            "FFmpeg error. Clip saved without camera overlay.",
            Level::Warning,
            Vec::new(),
            None,
            false,
        );
    }
    if let Some(last) = outcome.warnings.last() {
        state.error_bar.push_error(
            "CLIP SAVED WITH WARNING",
            last,
            Level::Warning,
            Vec::new(),
            None,
            false,
        );
    }
}
