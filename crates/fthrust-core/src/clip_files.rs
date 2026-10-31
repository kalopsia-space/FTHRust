//! Clip filename contract and conservative stale-partial recovery.

use crate::error::Result;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use time::OffsetDateTime;
use time::macros::format_description;

/// Completed-video suffixes (without the dot).
pub const VIDEO_SUFFIXES: [&str; 3] = ["mp4", "mkv", "avi"];
/// Image suffixes (without the dot).
pub const IMAGE_SUFFIXES: [&str; 3] = ["png", "jpg", "jpeg"];
/// Marker embedded in FTHR-owned clip file names.
pub const PARTIAL_MARKER: &str = "_clip_from_";
const AUDIO_MANIFEST_SUFFIX: &str = ".fthr-audio.json";

/// Path of the audio manifest sidecar for `media`: the full file name with
/// `.fthr-audio.json` appended.
pub fn audio_manifest_sidecar(media: &Path) -> PathBuf {
    let mut name = media.file_name().unwrap_or_default().to_os_string();
    name.push(AUDIO_MANIFEST_SUFFIX);
    media.with_file_name(name)
}

/// Whether `name` is an in-progress file (`*.partial` or `*.partial.*`).
pub fn is_partial_name(name: &str) -> bool {
    let n = name.to_lowercase();
    n.ends_with(".partial") || n.contains(".partial.")
}

/// Whether `p` names a finished, supported video (case-insensitive suffix).
pub fn is_completed_video_path(p: &Path) -> bool {
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if is_partial_name(name) {
        return false;
    }
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_SUFFIXES.iter().any(|s| e.eq_ignore_ascii_case(s)))
}

/// Whether `name` is an app-owned post-processing directory.
pub fn is_fthr_post_processing_dir(name: &str) -> bool {
    name == "Exported" || name == "Shared" || name.to_lowercase().starts_with(".fthr-")
}

/// Turn a window title into a safe folder name (max 32 chars).
pub fn sanitize_folder_name(s: &str) -> String {
    let trim = |x: &str| x.trim_matches(['.', ' ']).to_string();
    let kept: String = s
        .chars()
        .filter(|c| !"\\/:*?\"<>|".contains(*c) && (*c as u32) >= 32)
        .collect();
    let cut: String = trim(&kept).chars().take(32).collect();
    let mut cleaned = cut.trim_end_matches(['.', ' ']).to_string();
    let up = cleaned.to_uppercase();
    let reserved = matches!(up.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|p| {
            up.strip_prefix(p)
                .is_some_and(|d| matches!(d.as_bytes(), [b'1'..=b'9']))
        });
    if reserved {
        cleaned.push_str("_game");
    }
    if cleaned.is_empty() {
        "Unknown".into()
    } else {
        cleaned
    }
}

fn fmt(at: OffsetDateTime, f: &[time::format_description::BorrowedFormatItem<'_>]) -> String {
    at.format(f).expect("static format description is valid")
}

/// Path for a saved replay clip: `<root>/Desktop/desktop_clip_from_<ts>.mp4`
/// or `<root>/<game>/<game>_clip_from_<ts>.mp4`.
pub fn replay_clip_path(root: &Path, game: Option<&str>, at: OffsetDateTime) -> PathBuf {
    let ts = fmt(
        at,
        format_description!("[day][month repr:short][year]_[hour]-[minute]-[second]"),
    );
    match game {
        Some(g) => {
            let g = sanitize_folder_name(g);
            root.join(&g).join(format!("{g}{PARTIAL_MARKER}{ts}.mp4"))
        }
        None => root
            .join("Desktop")
            .join(format!("desktop{PARTIAL_MARKER}{ts}.mp4")),
    }
}

/// Append `_2`, `_3`, ... before the extension while the path exists.
pub fn unique_path(p: PathBuf) -> PathBuf {
    if !p.exists() {
        return p;
    }
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    (2u32..)
        .map(|n| p.with_file_name(format!("{stem}_{n}{ext}")))
        .find(|c| !c.exists())
        .expect("unbounded counter")
}

/// Path for a manual recording: `<dir>/FTHR_recording_<ts>.mp4`.
pub fn recording_path(dir: &Path, at: OffsetDateTime) -> PathBuf {
    let ts = fmt(
        at,
        format_description!("[year]-[month]-[day]_[hour]-[minute]-[second]"),
    );
    dir.join(format!("FTHR_recording_{ts}.mp4"))
}

/// Path for a screenshot: `<root>/Screenshots/screenshot_from_<ts>.png`.
pub fn screenshot_path(root: &Path, at: OffsetDateTime) -> PathBuf {
    let ts = fmt(
        at,
        format_description!("[year][month][day]_[hour][minute][second]_[subsecond digits:6]"),
    );
    root.join("Screenshots")
        .join(format!("screenshot_from_{ts}.png"))
}

/// Which kind of share export a file is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShareKind {
    /// Plain share export.
    Share,
    /// Discord-size-limited export.
    Discord,
}

/// Path for a share export: `<root>/Shared/<stem>_<kind>_<HH-MM-SS>_<8 hex>.<ext>`.
pub fn share_path(
    root: &Path,
    stem: &str,
    kind: ShareKind,
    at: OffsetDateTime,
    lossless: bool,
) -> PathBuf {
    let ts = fmt(at, format_description!("[hour]-[minute]-[second]"));
    let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let kind = match kind {
        ShareKind::Share => "share",
        ShareKind::Discord => "discord",
    };
    let ext = if lossless { "mkv" } else { "mp4" };
    root.join("Shared")
        .join(format!("{stem}_{kind}_{ts}_{id}.{ext}"))
}

/// The start of a manual recording's name (`FTHR_recording_<ts>.mp4`), lower
/// case as names are compared.
pub const RECORDING_PREFIX: &str = "fthr_recording_";
/// How old a leftover must be before it is removed: no save, recording or
/// export of this app runs that long without touching its file.
pub const STALE_PARTIAL_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether a lower-cased name carries the mark of a clip or recording the
/// app wrote.
fn fthr_owned_media_name(name: &str) -> bool {
    name.contains(PARTIAL_MARKER) || name.starts_with(RECORDING_PREFIX)
}

/// `.<stem>.partial.<32 hex>[.<ext>]`: the hidden staging name of an export,
/// a finalize or an audio manifest (`StagedOutput`, `export::staged_name`).
fn is_staged_output_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.') else {
        return false;
    };
    let Some(at) = rest.rfind(".partial.") else {
        return false;
    };
    let after = &rest[at + ".partial.".len()..];
    let (token, ext) = match after.split_once('.') {
        Some((token, ext)) => (token, Some(ext)),
        None => (after, None),
    };
    at > 0
        && token.len() == 32
        && token.bytes().all(|b| b.is_ascii_hexdigit())
        && ext.is_none_or(|e| {
            (1..=8).contains(&e.len()) && e.bytes().all(|b| b.is_ascii_alphanumeric())
        })
}

/// `.screenshot_from_<ts>.<8 hex>.png.partial`: a screenshot staged before it
/// was published.
fn is_screenshot_staging_name(name: &str) -> bool {
    name.starts_with(".screenshot_from_") && name.ends_with(".png.partial")
}

/// `.fthr-finalize-<hex>`: the scratch directory of an overlay burn.
fn is_finalize_dir_name(name: &str) -> bool {
    name.strip_prefix(".fthr-finalize-")
        .is_some_and(|hex| !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn is_old(meta: &std::fs::Metadata, now: SystemTime, max_age: Duration) -> bool {
    meta.modified()
        .ok()
        .and_then(|m| now.duration_since(m).ok())
        .is_some_and(|age| age >= max_age)
}

/// Whether nothing in `dir` (a finalize scratch directory) was touched within
/// `max_age`: a burn still writing keeps updating its files.
fn tree_is_old(dir: &Path, now: SystemTime, max_age: Duration, depth: u32) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return false;
    };
    if !is_old(&meta, now, max_age) {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().all(|e| {
        let path = e.path();
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => depth > 0 && tree_is_old(&path, now, max_age, depth - 1),
            Ok(m) => is_old(&m, now, max_age),
            Err(_) => false,
        }
    })
}

fn walk(dir: &Path, now: SystemTime, max_age: Duration, removed: &mut usize) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(path = %dir.display(), error = %e, "cannot read directory");
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if meta.is_dir() {
            if !is_finalize_dir_name(&name) {
                walk(&path, now, max_age, removed);
            } else if tree_is_old(&path, now, max_age, 2) {
                match std::fs::remove_dir_all(&path) {
                    Ok(()) => *removed += 1,
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "failed to remove stale finalize directory")
                    }
                }
            }
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        let partial = name.ends_with(".mp4.partial") && fthr_owned_media_name(&name);
        let manifest =
            name.ends_with(&format!(".mp4{AUDIO_MANIFEST_SUFFIX}")) && fthr_owned_media_name(&name);
        let staged = is_staged_output_name(&name) || is_screenshot_staging_name(&name);
        if !partial && !manifest && !staged {
            continue;
        }
        if !is_old(&meta, now, max_age) {
            continue;
        }
        if manifest {
            let fname = entry.file_name().to_string_lossy().into_owned();
            let media = path.with_file_name(&fname[..fname.len() - AUDIO_MANIFEST_SUFFIX.len()]);
            if media.exists() {
                continue;
            }
        }
        match std::fs::remove_file(&path) {
            Ok(()) => *removed += 1,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to remove stale partial")
            }
        }
    }
}

/// Removes what a crashed save, recording, export or screenshot left in the
/// library, never following symlinks, when it is older than `max_age`
/// (use [`STALE_PARTIAL_MAX_AGE`]; a running job keeps touching its file).
/// Only names the app itself writes qualify: `*_clip_from_*.mp4.partial` and
/// `FTHR_recording_*.mp4.partial`, their orphaned audio manifests, the hidden
/// `.<name>.partial.<32 hex>.<ext>` staging files of exports and overlay
/// burns, hidden `.png.partial` screenshots and `.fthr-finalize-*`
/// directories. A finished clip or recording is never touched. Returns the
/// number removed.
pub fn cleanup_stale_partials(root: &Path, now: SystemTime, max_age: Duration) -> Result<usize> {
    let mut removed = 0;
    if root.is_dir() {
        walk(root, now, max_age, &mut removed);
    }
    Ok(removed)
}

/// [`cleanup_stale_partials`] over several roots (the clips folder and a
/// recordings folder outside it); a root inside another is not walked twice.
pub fn cleanup_stale_partials_in(roots: &[PathBuf], now: SystemTime, max_age: Duration) -> usize {
    roots
        .iter()
        .filter(|r| !roots.iter().any(|o| o != *r && r.starts_with(o)))
        .map(|r| cleanup_stale_partials(r, now, max_age).unwrap_or(0))
        .sum()
}

/// Runs [`cleanup_stale_partials_in`] with [`STALE_PARTIAL_MAX_AGE`] on a
/// background thread, as the app does at startup: the walk can cover a whole
/// drive and must not delay the window. Logs what it removed.
pub fn spawn_stale_partial_sweep(
    roots: Vec<PathBuf>,
) -> std::io::Result<std::thread::JoinHandle<usize>> {
    std::thread::Builder::new()
        .name("fthr-stale-partial-cleanup".into())
        .spawn(move || {
            let removed =
                cleanup_stale_partials_in(&roots, SystemTime::now(), STALE_PARTIAL_MAX_AGE);
            if removed > 0 {
                tracing::info!(removed, "removed stale partial files left by a crash");
            }
            removed
        })
}
