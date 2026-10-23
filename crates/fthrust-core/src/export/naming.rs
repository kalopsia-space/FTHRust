//! Output names (`export_queue.submit`, the share window) and durations.

use std::path::{Path, PathBuf};

use super::filters::normalise_speed;

/// `{stem}_export_{12 hex}.mp4|.mkv`.
pub fn export_name(stem: &str, hex12: &str, lossless: bool) -> String {
    format!(
        "{stem}_export_{hex12}.{}",
        if lossless { "mkv" } else { "mp4" }
    )
}

/// `{stem}_share_{HH-MM-SS}_{8 hex}` (`_discord_` for the Discord preset).
pub fn share_name(stem: &str, hms: &str, hex8: &str, discord: bool, lossless: bool) -> String {
    let kind = if discord { "discord" } else { "share" };
    format!(
        "{stem}_{kind}_{hms}_{hex8}.{}",
        if lossless { "mkv" } else { "mp4" }
    )
}

/// `.{stem}.partial.{32 hex}{ext}` beside `final_path`.
pub fn staged_name(final_path: &Path, hex32: &str) -> PathBuf {
    let stem = final_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = final_path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    final_path.with_file_name(format!(".{stem}.partial.{hex32}{ext}"))
}

/// The path itself, else `_2`, `_3`… until it does not exist.
pub fn unique(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem}_{n}{ext}")))
        .find(|p| !p.exists())
        .expect("an unused name")
}

/// `_speed_adjusted_duration`: at least 0.1 s.
pub fn speed_adjusted_duration(seconds: f64, speed: f64) -> f64 {
    (seconds / normalise_speed(speed)).max(0.1)
}
