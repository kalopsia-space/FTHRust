//! Thumbnail cache keys, `.meta-v3` sidecars, negative-cache markers and pruning.

use crate::error::Result;
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long a `.failed` marker suppresses a fresh probe.
pub const NEGATIVE_CACHE: Duration = Duration::from_secs(30);
/// Minimum interval between prune runs.
pub const PRUNE_INTERVAL: Duration = Duration::from_secs(30);
/// Default cache size limit (512 MiB).
pub const MAX_CACHE_BYTES: u64 = 512 * 1024 * 1024;
/// Default limit on cache entries (thumbnail generations).
pub const MAX_CACHE_ENTRIES: usize = 8192;

/// Cache key: lowercase hex MD5 of `"{path}|{size}|{mtime_ns}|aspect-v4"`.
///
/// `canonical_path` must already be canonicalised by the caller.
pub fn thumbnail_key(canonical_path: &Path, size: u64, mtime_ns: i128) -> String {
    let mut bytes = canonical_path.as_os_str().as_encoded_bytes().to_vec();
    bytes.extend_from_slice(format!("|{size}|{mtime_ns}|aspect-v4").as_bytes());
    hex(&Md5::digest(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Factual clip metadata stored in a `.meta-v3` sidecar.
#[derive(Debug, Clone, PartialEq)]
pub struct MetaV3 {
    /// Duration in seconds.
    pub duration: f64,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frames per second.
    pub fps: f64,
    /// Video stream bitrate in bits per second.
    pub video_bitrate: u64,
    /// Container bitrate in bits per second.
    pub total_bitrate: u64,
}

impl MetaV3 {
    /// Serialise as `"{duration:.6} {width} {height} {fps:.6} {video_bitrate} {total_bitrate}"`.
    pub fn to_text(&self) -> String {
        format!(
            "{:.6} {} {} {:.6} {} {}",
            self.duration,
            self.width,
            self.height,
            self.fps,
            self.video_bitrate,
            self.total_bitrate
        )
    }

    /// Parse exactly six whitespace-separated fields; `None` on any mismatch or
    /// invalid value (non-finite or non-positive duration, zero size, bad fps).
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.len() != 6 {
            return None;
        }
        let meta = Self {
            duration: parts[0].parse().ok()?,
            width: parts[1].parse().ok()?,
            height: parts[2].parse().ok()?,
            fps: parts[3].parse().ok()?,
            video_bitrate: parts[4].parse().ok()?,
            total_bitrate: parts[5].parse().ok()?,
        };
        let valid = meta.duration.is_finite()
            && meta.duration > 0.0
            && meta.width > 0
            && meta.height > 0
            && meta.fps.is_finite()
            && meta.fps >= 0.0;
        valid.then_some(meta)
    }
}

/// The files that make up one cached thumbnail generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailPaths {
    /// The JPEG thumbnail (`<key>.jpg`).
    pub jpg: PathBuf,
    /// The metadata sidecar (`<key>.meta-v3`).
    pub meta: PathBuf,
    /// The negative-cache marker (`<key>.failed`).
    pub failed: PathBuf,
}

/// Build the cache file paths for `key` inside `dir`.
pub fn paths_for(dir: &Path, key: &str) -> ThumbnailPaths {
    ThumbnailPaths {
        jpg: dir.join(format!("{key}.jpg")),
        meta: dir.join(format!("{key}.meta-v3")),
        failed: dir.join(format!("{key}.failed")),
    }
}

/// True when the marker exists and is younger than 30 seconds at `now`.
pub fn failed_marker_is_fresh(p: &Path, now: SystemTime) -> bool {
    let Ok(modified) = fs::metadata(p).and_then(|m| m.modified()) else {
        return false;
    };
    now.duration_since(modified)
        .map(|age| age < NEGATIVE_CACHE)
        .unwrap_or(false)
}

/// Temporary sibling path for atomic writes: `<final>.tmp-<pid>-<tid>`.
pub fn temp_path_for(final_path: &Path) -> PathBuf {
    let tid: String = format!("{:?}", std::thread::current().id())
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let mut name = final_path.as_os_str().to_os_string();
    name.push(format!(".tmp-{}-{}", std::process::id(), tid));
    PathBuf::from(name)
}

/// Prune the cache with the default limits, at most once per 30 seconds.
pub fn prune(dir: &Path, now: SystemTime, last_prune: &mut Option<SystemTime>) -> Result<()> {
    prune_with_limits(dir, now, last_prune, MAX_CACHE_BYTES, MAX_CACHE_ENTRIES)
}

/// Prune with explicit limits. No-op within 30 s of `last_prune`; otherwise
/// evicts whole generations (a thumbnail plus its sidecars), oldest newest-mtime
/// first, until both limits hold, and records `now` in `last_prune`.
pub fn prune_with_limits(
    dir: &Path,
    now: SystemTime,
    last_prune: &mut Option<SystemTime>,
    max_bytes: u64,
    max_entries: usize,
) -> Result<()> {
    if let Some(last) = *last_prune
        && now.duration_since(last).is_ok_and(|d| d < PRUNE_INTERVAL)
    {
        return Ok(());
    }
    *last_prune = Some(now);

    struct Group {
        files: Vec<PathBuf>,
        bytes: u64,
        newest: SystemTime,
    }
    let mut groups: HashMap<String, Group> = HashMap::new();
    fs::create_dir_all(dir)?;
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.contains(".tmp-") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let stem_end = [".meta-", ".jpg", ".failed"]
            .iter()
            .filter_map(|s| name.find(s))
            .min()
            .unwrap_or(name.len());
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let g = groups.entry(name[..stem_end].to_string()).or_insert(Group {
            files: Vec::new(),
            bytes: 0,
            newest: SystemTime::UNIX_EPOCH,
        });
        g.files.push(entry.path());
        g.bytes += meta.len();
        g.newest = g.newest.max(mtime);
    }

    let mut total: u64 = groups.values().map(|g| g.bytes).sum();
    let mut count = groups.len();
    let mut order: Vec<(String, SystemTime)> =
        groups.iter().map(|(k, g)| (k.clone(), g.newest)).collect();
    order.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    for (key, _) in order {
        if total <= max_bytes && count <= max_entries {
            break;
        }
        let g = &groups[&key];
        for f in &g.files {
            // A concurrent writer or another process may own it; best effort.
            let _ = fs::remove_file(f);
        }
        total = total.saturating_sub(g.bytes);
        count -= 1;
    }
    Ok(())
}
