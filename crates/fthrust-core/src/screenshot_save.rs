//! Screenshot staging (`core/screenshot_save.py`): a timestamped final name
//! with `_01` collision suffixes, a same-directory `.png.partial` staging
//! file, and a publish that never replaces a raced destination.

use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use time::macros::format_description;

/// Largest PNG accepted from the portal (64 MiB).
pub const MAX_PNG_BYTES: u64 = 64 * 1024 * 1024;
/// Largest PNG side accepted.
pub const MAX_PNG_SIDE: u32 = 16384;
/// Largest PNG pixel count accepted.
pub const MAX_PNG_PIXELS: u64 = 100_000_000;

/// A screenshot error with the reference's code.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {detail}")]
pub struct ScreenshotError {
    /// `OUTPUT_DIRECTORY_UNAVAILABLE`, `OUTPUT_COLLISION`, `WRITE_FAILED`,
    /// `IMAGE_ENCODE_FAILED` or `INVALID_IMAGE`.
    pub code: &'static str,
    /// Human-readable detail.
    pub detail: String,
}

impl ScreenshotError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// The final PNG name and its same-directory staging file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenshotPaths {
    /// Where the screenshot ends up.
    pub final_path: PathBuf,
    /// Where it is written first (exists, empty, after reservation).
    pub staged: PathBuf,
}

fn ensure_dir(dir: &Path) -> Result<(), ScreenshotError> {
    std::fs::create_dir_all(dir).map_err(|e| {
        ScreenshotError::new(
            "OUTPUT_DIRECTORY_UNAVAILABLE",
            format!("Could not create the screenshot folder: {e}"),
        )
    })?;
    if !dir.is_dir() {
        return Err(ScreenshotError::new(
            "OUTPUT_DIRECTORY_UNAVAILABLE",
            format!(
                "Screenshot output path is not a directory: {}",
                dir.display()
            ),
        ));
    }
    Ok(())
}

fn next_available(dir: &Path, stem: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{stem}.png"));
    let mut n = 1;
    while candidate.exists() || candidate.symlink_metadata().is_ok() {
        candidate = dir.join(format!("{stem}_{n:02}.png"));
        n += 1;
    }
    candidate
}

fn create_staged(final_path: &Path) -> Result<PathBuf, ScreenshotError> {
    let dir = final_path.parent().unwrap_or(Path::new("."));
    let stem = final_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    for _ in 0..64 {
        let rand = &uuid::Uuid::new_v4().simple().to_string()[..8];
        let p = dir.join(format!(".{stem}.{rand}.png.partial"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(ScreenshotError::new(
                    "OUTPUT_DIRECTORY_UNAVAILABLE",
                    format!("Could not create a staged screenshot file: {e}"),
                ));
            }
        }
    }
    Err(ScreenshotError::new(
        "OUTPUT_DIRECTORY_UNAVAILABLE",
        "Could not create a staged screenshot file: too many collisions",
    ))
}

/// Reserves `screenshot_from_%Y%m%d_%H%M%S_%f.png` (with `_01`… when
/// taken) and an empty staging file next to it.
pub fn reserve_screenshot_paths(
    dir: &Path,
    now: OffsetDateTime,
) -> Result<ScreenshotPaths, ScreenshotError> {
    ensure_dir(dir)?;
    let stamp = now
        .format(format_description!(
            "[year][month][day]_[hour][minute][second]_[subsecond digits:6]"
        ))
        .map_err(|e| ScreenshotError::new("WRITE_FAILED", e.to_string()))?;
    let final_path = next_available(dir, &format!("screenshot_from_{stamp}"));
    let staged = create_staged(&final_path)?;
    Ok(ScreenshotPaths { final_path, staged })
}

/// Moves the staged PNG to its final name without overwriting a file that
/// appeared in the meantime (`OUTPUT_COLLISION`; the staged file is kept).
/// Tries a no-replace rename, then link-and-unlink, then a copy, so it works
/// on filesystems without `RENAME_NOREPLACE` or hard links (exFAT, vFAT).
pub fn publish_staged_png(staged: &Path, final_path: &Path) -> Result<(), ScreenshotError> {
    let collision = || {
        ScreenshotError::new(
            "OUTPUT_COLLISION",
            format!(
                "Refusing to overwrite an existing file: {}",
                final_path
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
        )
    };
    {
        use nix::fcntl::{AT_FDCWD, RenameFlags, renameat2};
        match renameat2(
            AT_FDCWD,
            staged,
            AT_FDCWD,
            final_path,
            RenameFlags::RENAME_NOREPLACE,
        ) {
            Ok(()) => return Ok(()),
            Err(nix::errno::Errno::EEXIST) => return Err(collision()),
            Err(
                nix::errno::Errno::EINVAL
                | nix::errno::Errno::ENOSYS
                | nix::errno::Errno::ENOTSUP
                | nix::errno::Errno::EPERM
                | nix::errno::Errno::EXDEV,
            ) => {}
            Err(e) => {
                return Err(ScreenshotError::new(
                    "WRITE_FAILED",
                    format!("Could not publish the screenshot: {e}"),
                ));
            }
        }
    }
    match std::fs::hard_link(staged, final_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(collision()),
        Err(e)
            if matches!(
                e.raw_os_error().map(nix::errno::Errno::from_raw),
                Some(
                    nix::errno::Errno::EPERM
                        | nix::errno::Errno::ENOTSUP
                        | nix::errno::Errno::EXDEV
                )
            ) =>
        {
            return publish_by_copy(staged, final_path);
        }
        Err(e) => {
            return Err(ScreenshotError::new(
                "WRITE_FAILED",
                format!("Could not publish the screenshot: {e}"),
            ));
        }
    }
    if let Err(e) = std::fs::remove_file(staged) {
        let _ = std::fs::remove_file(final_path);
        return Err(ScreenshotError::new(
            "WRITE_FAILED",
            format!("Could not remove screenshot staging file: {e}"),
        ));
    }
    Ok(())
}

/// The last resort: create the final file exclusively, copy the staged bytes
/// in, flush, then remove the staged file. An existing final file is a
/// collision and the staged file is kept.
pub fn publish_by_copy(staged: &Path, final_path: &Path) -> Result<(), ScreenshotError> {
    use std::io::Write;
    let mut out = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(final_path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(ScreenshotError::new(
                "OUTPUT_COLLISION",
                format!(
                    "Refusing to overwrite an existing file: {}",
                    final_path
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ),
            ));
        }
        Err(e) => {
            return Err(ScreenshotError::new(
                "WRITE_FAILED",
                format!("Could not publish the screenshot: {e}"),
            ));
        }
    };
    let result = std::fs::read(staged)
        .and_then(|bytes| out.write_all(&bytes))
        .and_then(|()| out.sync_all());
    if let Err(e) = result {
        let _ = std::fs::remove_file(final_path);
        return Err(ScreenshotError::new(
            "WRITE_FAILED",
            format!("Could not publish the screenshot: {e}"),
        ));
    }
    if let Err(e) = std::fs::remove_file(staged) {
        return Err(ScreenshotError::new(
            "WRITE_FAILED",
            format!("Could not remove screenshot staging file: {e}"),
        ));
    }
    Ok(())
}

/// Checks the PNG signature, IHDR and the size limits; returns (width, height).
pub fn validate_png_bytes(bytes: &[u8]) -> Result<(u32, u32), ScreenshotError> {
    let invalid = |why: &str| ScreenshotError::new("INVALID_IMAGE", why.to_owned());
    if bytes.len() as u64 > MAX_PNG_BYTES {
        return Err(invalid("The screenshot is larger than 64 MiB."));
    }
    if bytes.len() < 33 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return Err(invalid("The portal did not return a PNG image."));
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if width == 0 || height == 0 || width > MAX_PNG_SIDE || height > MAX_PNG_SIDE {
        return Err(invalid("The screenshot dimensions are out of range."));
    }
    if u64::from(width) * u64::from(height) > MAX_PNG_PIXELS {
        return Err(invalid("The screenshot has more than 100 megapixels."));
    }
    Ok((width, height))
}
