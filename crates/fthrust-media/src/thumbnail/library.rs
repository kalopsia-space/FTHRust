//! The library thumbnail cache (`ui/clip_grid.py` `_ThumbnailWorker`):
//! `~/.fthr/thumbnails/<md5>.jpg` (quality 85) with a `.meta-v3` sidecar
//! and a `.failed` negative marker valid for 30 s, pruned at most every
//! 30 s. Keys follow the reference (`canonical path | size | mtime_ns |
//! aspect-v4`), so caches are shared with the Python app.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use fthrust_core::thumbnails::{
    MetaV3, failed_marker_is_fresh, paths_for, prune, temp_path_for, thumbnail_key,
};
use image::RgbaImage;

use crate::{MediaError, Result};

/// A cached thumbnail and its metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct CachedThumbnail {
    /// The JPEG on disk.
    pub jpg: PathBuf,
    /// The probed metadata.
    pub meta: MetaV3,
    /// `meta.duration`, for callers that only need the length.
    pub duration_s: f64,
}

static LAST_PRUNE: Mutex<Option<SystemTime>> = Mutex::new(None);

/// The cache key of `path` (its canonical path, size and mtime).
pub fn cache_key(path: &Path) -> std::io::Result<String> {
    let canonical = std::fs::canonicalize(path)?;
    let meta = std::fs::metadata(&canonical)?;
    let mtime_ns = meta
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0);
    Ok(thumbnail_key(&canonical, meta.len(), mtime_ns))
}

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::File::create(path);
}

fn write_atomic(final_path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_path_for(final_path);
    let result = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, final_path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Returns the cached thumbnail, building it when missing. `Ok(None)`
/// means the clip could not be decoded (a `.failed` marker is written, or
/// a fresh one was honoured) or `cancel` was set.
pub fn load_or_build(
    cache_dir: &Path,
    path: &Path,
    cancel: &AtomicBool,
) -> Result<Option<CachedThumbnail>> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let key = cache_key(path)?;
    let paths = paths_for(cache_dir, &key);
    let now = SystemTime::now();
    if paths.jpg.is_file()
        && let Ok(text) = std::fs::read_to_string(&paths.meta)
        && let Some(meta) = MetaV3::parse(&text)
    {
        return Ok(Some(CachedThumbnail {
            jpg: paths.jpg,
            duration_s: meta.duration,
            meta,
        }));
    }
    if failed_marker_is_fresh(&paths.failed, now) {
        return Ok(None);
    }
    let result = build(&paths.jpg, &paths.meta, path, cancel);
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let out = match result {
        Ok(Some(t)) => Some(t),
        Ok(None) => {
            touch(&paths.failed);
            None
        }
        Err(e) => {
            tracing::debug!(path = %path.display(), "thumbnail failed: {e}");
            touch(&paths.failed);
            None
        }
    };
    if let Ok(mut last) = LAST_PRUNE.lock()
        && let Err(e) = prune(cache_dir, now, &mut last)
    {
        tracing::debug!("thumbnail cache prune failed: {e}");
    }
    Ok(out)
}

fn build(
    jpg: &Path,
    meta_path: &Path,
    path: &Path,
    cancel: &AtomicBool,
) -> Result<Option<CachedThumbnail>> {
    let Some(probed) = crate::probe::probe_video_metadata(path) else {
        return Ok(None);
    };
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let meta = MetaV3 {
        duration: probed.duration_seconds.unwrap_or(0.0),
        width: probed.width.unwrap_or(0),
        height: probed.height.unwrap_or(0),
        fps: probed.average_fps.unwrap_or(0.0),
        video_bitrate: probed.video_bitrate_bps.unwrap_or(0),
        total_bitrate: probed.total_bitrate_bps.unwrap_or(0),
    };
    if let Some(parent) = jpg.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_atomic(meta_path, meta.to_text().as_bytes())?;
    let image = super::poster(path)?;
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    let rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 85)
        .encode_image(&rgb)
        .map_err(|e| MediaError::Invalid(format!("jpeg encode: {e}")))?;
    if !jpg.exists() {
        write_atomic(jpg, &bytes)?;
    }
    Ok(Some(CachedThumbnail {
        jpg: jpg.to_path_buf(),
        duration_s: meta.duration,
        meta,
    }))
}

/// A screenshot's thumbnail: the image decoded and fitted within 640×360
/// (never enlarged).
pub fn image_thumbnail(path: &Path) -> Result<RgbaImage> {
    let img = image::open(path).map_err(|e| MediaError::Invalid(format!("image: {e}")))?;
    let (w, h) = super::fit_within(img.width(), img.height(), 640, 360);
    if (w, h) == (img.width(), img.height()) {
        return Ok(img.to_rgba8());
    }
    Ok(img
        .resize_exact(w, h, image::imageops::FilterType::Triangle)
        .to_rgba8())
}
