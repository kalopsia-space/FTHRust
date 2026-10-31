//! Clip filename contract and conservative stale-partial recovery.

use crate::error::Result;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
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

/// Renames `from` to `to` without ever replacing `to` (`AlreadyExists` when
/// it exists): `renameat2(RENAME_NOREPLACE)`; where the filesystem rejects
/// that flag (NFS, FUSE without rename2), [`publish_without_noreplace`].
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use nix::fcntl::{AT_FDCWD, RenameFlags, renameat2};
    match renameat2(AT_FDCWD, from, AT_FDCWD, to, RenameFlags::RENAME_NOREPLACE) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::EINVAL | nix::errno::Errno::ENOSYS) => {
            publish_without_noreplace(from, to, link_then_unlink)
        }
        Err(e) => Err(io::Error::from(e)),
    }
}

/// No-replace move by hard link plus unlink (`AlreadyExists` when `to`
/// exists). Once the link exists the file is at `to`, so a failed unlink
/// only leaves a second name for it (logged).
pub fn link_then_unlink(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::hard_link(from, to)?;
    if let Err(e) = std::fs::remove_file(from) {
        tracing::warn!(
            "moved {} but could not remove the old name {}: {e}",
            to.display(),
            from.display()
        );
    }
    Ok(())
}

/// The move for filesystems without `RENAME_NOREPLACE`: `link` (a hard link
/// plus unlink in production), and where the filesystem has no hard links
/// either (some FUSE mounts, such as gvfs, rclone or exfat-fuse) a plain
/// rename once `to` is checked to be free. That last step is not atomic
/// against another writer creating `to` in between, but the names this is
/// used for are unique, and refusing would leave the file unpublished.
#[doc(hidden)]
pub fn publish_without_noreplace(
    from: &Path,
    to: &Path,
    link: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    match link(from, to) {
        Err(e) if links_unsupported(&e) => {
            match std::fs::symlink_metadata(to) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("{} already exists", to.display()),
                    ));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            std::fs::rename(from, to)
        }
        other => other,
    }
}

/// Whether a failed `link` means the filesystem has no hard links.
fn links_unsupported(e: &io::Error) -> bool {
    use nix::libc::{ENOSYS, EOPNOTSUPP, EPERM};
    matches!(e.raw_os_error(), Some(EPERM | EOPNOTSUPP | ENOSYS))
}

/// Whether the MP4 at `path` is a finished file, judged from its top-level
/// boxes alone. The clip writer's plain MP4 is finished when it has its
/// `mdat` and `moov` (the index, written last) and its boxes end exactly
/// where the file does; a crash leaves no `moov`, or a cut one. The
/// recorder's fragmented MP4 has its `moov` first and plays up to its last
/// whole fragment, so one with at least one `moof` and its `mdat` counts as
/// finished even when a crash cut the next fragment short.
pub fn mp4_is_finished(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let (mut pos, mut moov, mut mdat, mut fragments) = (0u64, false, false, 0u32);
    let (mut after_moof, mut cut) = (false, false);
    while pos < len {
        let mut head = [0u8; 16];
        let avail = (len - pos).min(16) as usize;
        if avail < 8 {
            cut = true;
            break;
        }
        file.seek(SeekFrom::Start(pos))?;
        file.read_exact(&mut head[..avail])?;
        let short = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        let kind = [head[4], head[5], head[6], head[7]];
        let (size, header) = match short {
            // "Runs to the end of the file".
            0 => (len - pos, 8),
            1 if avail == 16 => (
                u64::from_be_bytes([
                    head[8], head[9], head[10], head[11], head[12], head[13], head[14], head[15],
                ]),
                16,
            ),
            1 => (0, 16),
            n => (u64::from(n), 8),
        };
        if size < header || size > len - pos {
            cut = true;
            break;
        }
        match &kind {
            b"moov" => moov = true,
            b"mdat" => {
                mdat = true;
                if after_moof {
                    fragments += 1;
                }
            }
            _ => {}
        }
        after_moof = &kind == b"moof";
        pos += size;
    }
    Ok(if fragments > 0 {
        moov
    } else {
        moov && mdat && !cut
    })
}

/// Where a finished `.partial` went.
enum Kept {
    /// It now has this name.
    Published(PathBuf),
    /// It was already there: the `.partial` was a second name for the clip
    /// published under this one (a link whose unlink failed), and is gone.
    AlreadyPublished(PathBuf),
}

/// Whether `a` and `b` name one file (device and inode), not following
/// symlinks: a symlink to the `.partial` is not a copy of the clip.
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Gives a finished clip left as `.partial` its own name, or
/// `<stem> (recovered).<ext>` (then `(recovered 2)` …) when that is taken.
/// Never replaces a file. When its own name is already the same file, only
/// the `.partial` name goes.
fn publish_finished(partial: &Path) -> io::Result<Kept> {
    let name = partial
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| n.len() > ".partial".len())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a .partial name"))?;
    // The walk matched the suffix case-insensitively; it is ASCII.
    let own = partial.with_file_name(&name[..name.len() - ".partial".len()]);
    if same_file(partial, &own) {
        std::fs::remove_file(partial)?;
        return Ok(Kept::AlreadyPublished(own));
    }
    let stem = own
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = own
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let recovered = (1..=9).map(|n| {
        let tag = if n == 1 {
            " (recovered)".to_owned()
        } else {
            format!(" (recovered {n})")
        };
        partial.with_file_name(format!("{stem}{tag}{ext}"))
    });
    for candidate in std::iter::once(own.clone()).chain(recovered) {
        match rename_no_replace(partial, &candidate) {
            Ok(()) => return Ok(Kept::Published(candidate)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("no free name next to {}", own.display()),
    ))
}

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
        if partial {
            // A finished clip that never got its name (a failed rename) is
            // the only copy: publish it, never delete it.
            match mp4_is_finished(&path) {
                Ok(false) => {}
                Ok(true) => {
                    match publish_finished(&path) {
                        Ok(Kept::Published(to)) => tracing::info!(
                            from = %path.display(),
                            to = %to.display(),
                            "published a finished clip left as .partial"
                        ),
                        Ok(Kept::AlreadyPublished(to)) => {
                            *removed += 1;
                            tracing::info!(
                                from = %path.display(),
                                to = %to.display(),
                                "removed a second name of a published clip"
                            );
                        }
                        Err(e) => tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "a finished clip stays as .partial"
                        ),
                    }
                    continue;
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "cannot read a leftover .partial; keeping it");
                    continue;
                }
            }
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
/// directories. A finished clip or recording is never removed: one left as
/// `.partial` (its MP4 whole, see [`mp4_is_finished`]) is published under
/// its own name, or as `<stem> (recovered).mp4` when that is taken, and
/// stays as it is if renaming fails. A `.partial` that is only a second
/// name for its published clip (a link whose unlink failed) loses that
/// name. Returns the number removed, second names included.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// One MP4 box: size, type, payload.
    fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    /// What the clip writer leaves when it finishes: data, then the index.
    fn finished_clip() -> Vec<u8> {
        [
            mp4_box(b"ftyp", b"isom\0\0\x02\0isomiso2"),
            mp4_box(b"free", b""),
            mp4_box(b"mdat", &[7; 4000]),
            mp4_box(b"moov", &[1; 300]),
        ]
        .concat()
    }

    /// What a crash during the write leaves: FFmpeg's mdat still has its
    /// "runs to the end of the file" size, and there is no index.
    fn crashed_clip() -> Vec<u8> {
        let mut out = [
            mp4_box(b"ftyp", b"isom\0\0\x02\0isomiso2"),
            mp4_box(b"wide", b""),
        ]
        .concat();
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(b"mdat");
        out.extend_from_slice(&[7; 4000]);
        out
    }

    /// A fragmented recording cut short inside its third fragment.
    fn cut_recording() -> Vec<u8> {
        let mut out = [
            mp4_box(b"ftyp", b"isom\0\0\x02\0isomiso2"),
            mp4_box(b"moov", &[1; 300]),
            mp4_box(b"moof", &[2; 100]),
            mp4_box(b"mdat", &[7; 4000]),
            mp4_box(b"moof", &[2; 100]),
            mp4_box(b"mdat", &[7; 4000]),
        ]
        .concat();
        let mut torn = mp4_box(b"moof", &[2; 100]);
        torn.truncate(50);
        out.extend_from_slice(&torn);
        out
    }

    /// Runs the sweep as if every file were two days old.
    fn sweep(root: &Path) -> usize {
        let later = SystemTime::now() + 2 * STALE_PARTIAL_MAX_AGE;
        cleanup_stale_partials(root, later, STALE_PARTIAL_MAX_AGE).unwrap()
    }

    #[test]
    fn a_finished_clip_left_as_partial_is_published_not_removed() {
        let d = tempfile::tempdir().unwrap();
        let partial = d
            .path()
            .join("Neon Drift_clip_from_31Oct2026_20-00-00.mp4.partial");
        std::fs::write(&partial, finished_clip()).unwrap();
        assert_eq!(sweep(d.path()), 0);
        let published = d.path().join("Neon Drift_clip_from_31Oct2026_20-00-00.mp4");
        assert!(!partial.exists());
        assert_eq!(std::fs::read(&published).unwrap(), finished_clip());
    }

    #[test]
    fn a_taken_name_gets_a_recovered_name_and_nothing_is_replaced() {
        let d = tempfile::tempdir().unwrap();
        let taken = d.path().join("desktop_clip_from_31Oct2026_20-00-00.mp4");
        std::fs::write(&taken, b"another clip").unwrap();
        let partial = d
            .path()
            .join("desktop_clip_from_31Oct2026_20-00-00.mp4.partial");
        std::fs::write(&partial, finished_clip()).unwrap();
        assert_eq!(sweep(d.path()), 0);
        assert_eq!(std::fs::read(&taken).unwrap(), b"another clip");
        let recovered = d
            .path()
            .join("desktop_clip_from_31Oct2026_20-00-00 (recovered).mp4");
        assert_eq!(std::fs::read(&recovered).unwrap(), finished_clip());
        assert!(!partial.exists());
    }

    #[test]
    fn a_second_name_for_a_published_clip_is_removed_not_recovered() {
        // A link that published the clip, then an unlink that failed.
        let d = tempfile::tempdir().unwrap();
        let published = d.path().join("desktop_clip_from_31Oct2026_20-00-00.mp4");
        std::fs::write(&published, finished_clip()).unwrap();
        let partial = d
            .path()
            .join("desktop_clip_from_31Oct2026_20-00-00.mp4.partial");
        std::fs::hard_link(&published, &partial).unwrap();
        assert_eq!(sweep(d.path()), 1);
        assert!(!partial.exists());
        assert_eq!(std::fs::read(&published).unwrap(), finished_clip());
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [published.file_name().unwrap()]);
    }

    #[test]
    fn a_link_to_the_partial_is_not_the_same_file() {
        // The clip's own name only points at the .partial: removing the
        // .partial would lose the clip.
        let d = tempfile::tempdir().unwrap();
        let partial = d
            .path()
            .join("desktop_clip_from_31Oct2026_20-00-00.mp4.partial");
        std::fs::write(&partial, finished_clip()).unwrap();
        let own = d.path().join("desktop_clip_from_31Oct2026_20-00-00.mp4");
        std::os::unix::fs::symlink(&partial, &own).unwrap();
        assert_eq!(sweep(d.path()), 0);
        let recovered = d
            .path()
            .join("desktop_clip_from_31Oct2026_20-00-00 (recovered).mp4");
        assert_eq!(std::fs::read(&recovered).unwrap(), finished_clip());
    }

    #[test]
    fn an_unfinished_partial_is_still_removed() {
        let d = tempfile::tempdir().unwrap();
        let crashed = d
            .path()
            .join("desktop_clip_from_31Oct2026_20-00-00.mp4.partial");
        std::fs::write(&crashed, crashed_clip()).unwrap();
        let mut torn_index = finished_clip();
        torn_index.truncate(torn_index.len() - 10);
        let torn = d
            .path()
            .join("desktop_clip_from_31Oct2026_20-00-01.mp4.partial");
        std::fs::write(&torn, torn_index).unwrap();
        assert_eq!(sweep(d.path()), 2);
        assert!(!crashed.exists() && !torn.exists());
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_recording_with_whole_fragments_is_published() {
        let d = tempfile::tempdir().unwrap();
        let partial = d
            .path()
            .join("FTHR_recording_2026-10-31_20-00-00.mp4.partial");
        std::fs::write(&partial, cut_recording()).unwrap();
        let header_only = d
            .path()
            .join("FTHR_recording_2026-10-31_20-00-01.mp4.partial");
        std::fs::write(
            &header_only,
            [mp4_box(b"ftyp", b"isom"), mp4_box(b"moov", &[1; 300])].concat(),
        )
        .unwrap();
        assert_eq!(sweep(d.path()), 1, "the one without a fragment goes");
        assert!(!header_only.exists() && !partial.exists());
        let published = d.path().join("FTHR_recording_2026-10-31_20-00-00.mp4");
        assert_eq!(std::fs::read(&published).unwrap(), cut_recording());
    }
}
