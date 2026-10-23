//! File and folder pickers through the FileChooser portal (the reference
//! used Qt's dialogs). A Settings page sets `browse_requested`; the portal
//! runs on the services runtime and its answer comes back as
//! [`super::Event::Picked`], which [`apply`] turns into settings changes.

use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

/// A `file://` URI as a path (`None` for other schemes or bad escapes).
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let s = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(s, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

use std::path::Path;

use serde_json::{Value, json};

use crate::state::{AppState, BrowseKind, DialogKind, DialogResult};

/// Dialog tag of the theme-import confirmation.
const THEME_TAG: &str = "browse-theme-import:";

/// Portal filters as `(label, globs)`.
pub type Filters = Vec<(&'static str, Vec<&'static str>)>;

/// The portal request for a picker: `(title, directory, multiple,
/// filters)` with filters as `(label, globs)`.
pub fn request_for(kind: &BrowseKind) -> (String, bool, bool, Filters) {
    let images = vec!["*.png", "*.jpg", "*.jpeg", "*.webp", "*.bmp", "*.gif"];
    let all = ("All files", vec!["*"]);
    match kind {
        BrowseKind::ClipsFolder => ("Select Clips Storage Folder".into(), true, false, vec![]),
        BrowseKind::RecordingFolder => {
            ("Select Manual Recording Folder".into(), true, false, vec![])
        }
        BrowseKind::ImportFolder => ("Select Clips Folder".into(), true, false, vec![]),
        BrowseKind::GaryImage => (
            "Choose Gary Mode image".into(),
            false,
            false,
            vec![("Images", images), all],
        ),
        BrowseKind::ImageLayers => (
            "Add image overlays".into(),
            false,
            true,
            vec![
                (
                    "Images",
                    vec!["*.png", "*.jpg", "*.jpeg", "*.webp", "*.bmp"],
                ),
                all,
            ],
        ),
        BrowseKind::PreviewBackground => (
            "Choose preview background".into(),
            false,
            false,
            vec![("Images", images), all],
        ),
        BrowseKind::ThemeArchive => (
            "Import Theme".into(),
            false,
            false,
            vec![("ZIP files", vec!["*.zip"])],
        ),
        BrowseKind::FontFile(_) => (
            "Import font".into(),
            false,
            false,
            vec![("Font files", vec!["*.ttf", "*.otf"])],
        ),
        BrowseKind::IconFile(label) => (
            format!("Select icon for {label}"),
            false,
            false,
            vec![(
                "Images",
                vec!["*.png", "*.jpg", "*.jpeg", "*.bmp", "*.ico", "*.svg"],
            )],
        ),
        BrowseKind::SoundFile(label) => (
            format!("Select sound for {label}"),
            false,
            false,
            vec![(
                "Audio files",
                vec![
                    "*.mp3", "*.wav", "*.ogg", "*.flac", "*.m4a", "*.wma", "*.aac",
                ],
            )],
        ),
    }
}

/// Opens the FileChooser portal; the chosen paths (empty on cancel).
pub async fn pick(kind: &BrowseKind) -> Result<Vec<PathBuf>, String> {
    use ashpd::desktop::file_chooser::{FileChooserProxy, FileFilter, OpenFileOptions};
    let (title, directory, multiple, filters) = request_for(kind);
    let proxy = FileChooserProxy::new().await.map_err(|e| e.to_string())?;
    let options = OpenFileOptions::default()
        .set_modal(true)
        .set_directory(directory)
        .set_multiple(multiple)
        .set_filters(filters.into_iter().map(|(label, globs)| {
            globs
                .into_iter()
                .fold(FileFilter::new(label), FileFilter::glob)
        }));
    let request = proxy
        .open_file(None, &title, options)
        .await
        .map_err(|e| e.to_string())?;
    match request.response() {
        Ok(files) => Ok(files
            .uris()
            .iter()
            .filter_map(|u| uri_to_path(u.as_str()))
            .collect()),
        // Cancelled.
        Err(ashpd::Error::Response(_)) => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

fn storage_failure(state: &mut AppState, message: String) {
    state
        .dialogs
        .push_message("Storage location unavailable", message);
}

/// `ensure_storage_directory(probe_writable=True)`: creates the folder and
/// proves it writable.
fn ensure_writable(dir: &Path) -> Result<PathBuf, String> {
    let resolved = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(dir)
    };
    let fail = |e: &dyn std::fmt::Display| {
        format!(
            "FTHR cannot use the storage folder \"{}\". Choose a writable folder in Settings. No files were moved. ({e})",
            resolved.display()
        )
    };
    std::fs::create_dir_all(&resolved).map_err(|e| fail(&e))?;
    let probe = resolved.join(format!(
        ".fthr-storage-probe-{}",
        fthrust_media::export::random_hex(8)
    ));
    std::fs::write(&probe, b"").map_err(|e| fail(&e))?;
    let _ = std::fs::remove_file(&probe);
    Ok(resolved)
}

fn path_key(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `add_import_root`: appends `dir` unless an equal path is listed.
fn add_import_root(state: &mut AppState, dir: &Path) -> bool {
    let mut roots: Vec<String> = state
        .settings
        .get("imported_clip_folders")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let key = path_key(dir);
    if roots.iter().any(|r| path_key(Path::new(r)) == key) {
        return false;
    }
    roots.push(key.to_string_lossy().into_owned());
    state.set_setting("imported_clip_folders", json!(roots));
    true
}

fn readable_image(p: &Path) -> bool {
    p.is_file() && fthrust_media::export::image_decodes(p)
}

/// The family names a font file exposes.
fn font_families(path: &Path) -> Vec<String> {
    use skrifa::MetadataProvider;
    use skrifa::raw::types::NameId;
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(font) = skrifa::FontRef::new(&bytes) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for id in [NameId::TYPOGRAPHIC_FAMILY_NAME, NameId::FAMILY_NAME] {
        if let Some(name) = font.localized_strings(id).english_or_first() {
            let name = name.to_string();
            if !name.trim().is_empty() && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

fn save_theme(state: &mut AppState) {
    if let Err(e) = state.theme.save() {
        state.dialogs.push_message("Theme not saved", e.to_string());
    }
}

/// Applies a picker's answer (`paths` empty on cancel).
pub fn apply(state: &mut AppState, kind: BrowseKind, paths: Vec<PathBuf>) {
    let Some(first) = paths.first().cloned() else {
        return;
    };
    match kind {
        BrowseKind::ClipsFolder => {
            let derived = match state.settings.get_str("recording_directory") {
                None => true,
                Some(rec) => state
                    .settings
                    .clips_directory()
                    .is_ok_and(|root| Path::new(rec) == root.join("Recordings")),
            };
            let root = match ensure_writable(&first).and_then(|r| {
                fthrust_core::settings::prepare_clips_storage(&r)
                    .map(|_| r.clone())
                    .map_err(|e| {
                        format!(
                            "FTHR cannot use the storage folder \"{}\". Choose a writable folder in Settings. No files were moved. ({e})",
                            r.display()
                        )
                    })
            }) {
                Ok(r) => r,
                Err(e) => return storage_failure(state, e),
            };
            state
                .settings
                .set("clips_directory", json!(root.to_string_lossy()));
            if derived {
                state.settings.set(
                    "recording_directory",
                    json!(root.join("Recordings").to_string_lossy()),
                );
            }
            if state.settings.save().is_err() {
                return storage_failure(
                    state,
                    "FTHR could not save the new storage location. Your current library remains selected; choose a writable settings folder and try again. No files were moved.".into(),
                );
            }
            state.clips_directory_changed = true;
        }
        BrowseKind::RecordingFolder => {
            let dir = match ensure_writable(&first) {
                Ok(d) => d,
                Err(e) => return storage_failure(state, e),
            };
            state.set_setting("recording_directory", json!(dir.to_string_lossy()));
            let inside = state
                .settings
                .clips_directory()
                .is_ok_and(|root| dir.starts_with(root));
            if !inside && add_import_root(state, &dir) {
                state.imported_folders_changed = true;
            }
        }
        BrowseKind::ImportFolder => {
            if add_import_root(state, &first) {
                state.imported_folders_changed = true;
            }
        }
        BrowseKind::GaryImage => {
            if !readable_image(&first) {
                state.dialogs.push_message(
                    "Image not supported",
                    "Choose a valid PNG, JPG, WEBP, BMP, or GIF image.",
                );
                return;
            }
            state.set_setting("gary_image_path", json!(first.to_string_lossy()));
        }
        BrowseKind::ImageLayers => {
            let mut layers: Vec<Value> = state
                .settings
                .get("image_overlays")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let before = layers.len();
            for p in paths.iter().filter(|p| readable_image(p)) {
                let i = layers.len();
                let (x, y) = [
                    (0.76, 0.04),
                    (0.04, 0.04),
                    (0.04, 0.72),
                    (0.76, 0.72),
                    (0.40, 0.38),
                ][i % 5];
                layers.push(json!({
                    "path": p.to_string_lossy(),
                    "enabled": true,
                    "opacity": 100,
                    "fit": "fit",
                    "rect": {"x": x, "y": y, "w": 0.20, "h": 0.24},
                }));
            }
            if layers.len() == before {
                return;
            }
            state.settings.set("image_overlays", json!(layers));
            state.set_setting("image_overlay_enabled", json!(true));
            state.selected_image_layer = layers.len() - 1;
        }
        BrowseKind::PreviewBackground => {
            if readable_image(&first) {
                state.set_setting(
                    "input_overlay_preview_background",
                    json!(first.to_string_lossy()),
                );
            }
        }
        BrowseKind::ThemeArchive => {
            state.dialogs.push_modal(DialogKind::Question {
                title: "Import Theme".into(),
                message: "This will fully replace your current theme (typography, colors, icons, and sounds).\n\nContinue?".into(),
                tag: format!("{THEME_TAG}{}", first.display()),
            });
        }
        BrowseKind::FontFile(role) => {
            let families = font_families(&first);
            if families.is_empty() {
                state.dialogs.push_message(
                    "Font not supported",
                    "Choose a valid OpenType (.otf) or TrueType (.ttf) font file.",
                );
                return;
            }
            if let Err(e) = state.theme.set_custom_font(&families, &first) {
                state
                    .dialogs
                    .push_message("Font import failed", e.to_string());
                return;
            }
            state.theme.data.fonts.insert(role, families[0].clone());
            save_theme(state);
        }
        BrowseKind::IconFile(logical) => {
            // Resized to the default icon's size class and stored as PNG
            // (the reference cropped it in a dialog first).
            let Ok(img) = image::open(&first) else {
                return;
            };
            let img = if img.width().max(img.height()) > 64 {
                img.resize(64, 64, image::imageops::FilterType::Lanczos3)
            } else {
                img
            };
            let temp = std::env::temp_dir().join(format!(
                "fthr-icon-{}.png",
                fthrust_media::export::random_hex(8)
            ));
            if img.save(&temp).is_err() {
                return;
            }
            let r = state.theme.set_custom_icon(&logical, &temp);
            let _ = std::fs::remove_file(&temp);
            match r {
                Ok(()) => {
                    save_theme(state);
                    state.icons.clear();
                }
                Err(e) => state
                    .dialogs
                    .push_message("Icon import failed", e.to_string()),
            }
        }
        BrowseKind::SoundFile(key) => match state.theme.set_custom_sound(&key, &first) {
            Ok(()) => save_theme(state),
            Err(e) => state
                .dialogs
                .push_message("Sound import failed", e.to_string()),
        },
    }
}

/// Answers to this module's dialogs (the theme-import confirmation).
pub fn service(state: &mut AppState) {
    let results = std::mem::take(&mut state.dialogs.results);
    let mut rest = Vec::new();
    for r in results {
        match &r {
            DialogResult::Answer { tag, yes } if tag.starts_with(THEME_TAG) => {
                if *yes {
                    let path = PathBuf::from(&tag[THEME_TAG.len()..]);
                    match state.theme.import_zip(&path) {
                        Ok(()) => state.dialogs.push_message(
                            "Imported",
                            "Theme imported successfully.\nClick APPLY THEME to see changes in the app.",
                        ),
                        Err(e) => state.dialogs.push_message("Theme import failed", e.to_string()),
                    }
                }
            }
            _ => rest.push(r),
        }
    }
    state.dialogs.results = rest;
}
