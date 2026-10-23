//! File and folder pickers (notes-pickers-reference).

use std::path::PathBuf;

use fthrust_ui::services::browse::uri_to_path;

#[test]
fn file_uris_become_paths() {
    assert_eq!(
        uri_to_path("file:///home/a/My%20Clips/x.png"),
        Some(PathBuf::from("/home/a/My Clips/x.png"))
    );
    assert_eq!(
        uri_to_path("file:///tmp/%C3%A9t%C3%A9.png"),
        Some(PathBuf::from("/tmp/été.png"))
    );
    assert_eq!(
        uri_to_path("file://localhost/tmp/a.png"),
        Some(PathBuf::from("/tmp/a.png"))
    );
    assert_eq!(uri_to_path("https://example.com/a.png"), None);
    assert_eq!(uri_to_path("file:///bad%2"), None);
}

use std::path::Path;

use fthrust_ui::services::browse::{apply, service};
use fthrust_ui::state::{AppState, BrowseKind, DialogKind, DialogResult};
use serde_json::json;

fn state(tmp: &Path) -> AppState {
    AppState::for_tests(tmp, json!({}))
}

fn messages(s: &AppState) -> Vec<(String, String)> {
    s.dialogs
        .messages
        .iter()
        .chain(s.dialogs.modal.iter())
        .filter_map(|d| match d {
            DialogKind::Message { title, message }
            | DialogKind::Question { title, message, .. } => Some((title.clone(), message.clone())),
            _ => None,
        })
        .collect()
}

fn png(p: &Path) -> PathBuf {
    image::RgbaImage::from_pixel(40, 30, image::Rgba([0, 200, 0, 255]))
        .save(p)
        .unwrap();
    p.to_path_buf()
}

fn junk(p: &Path) -> PathBuf {
    std::fs::write(p, b"definitely not an image").unwrap();
    p.to_path_buf()
}

#[test]
fn clips_folder_prepares_and_switches() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    let root = tmp.path().join("My Clips");
    apply(&mut s, BrowseKind::ClipsFolder, vec![root.clone()]);
    for sub in ["Desktop", "Recordings", "Exported", "Shared", "Screenshots"] {
        assert!(root.join(sub).is_dir(), "{sub}");
    }
    assert_eq!(
        s.settings.get_str("clips_directory"),
        Some(root.to_str().unwrap())
    );
    // The recording folder was derived from the old root: it follows.
    assert_eq!(
        s.settings.get_str("recording_directory"),
        Some(root.join("Recordings").to_str().unwrap())
    );
    assert!(s.clips_directory_changed);
    assert!(messages(&s).is_empty());
}

#[test]
fn clips_folder_keeps_an_explicit_recording_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    let rec = tmp.path().join("rec");
    s.settings
        .set("recording_directory", json!(rec.to_str().unwrap()));
    apply(&mut s, BrowseKind::ClipsFolder, vec![tmp.path().join("c2")]);
    assert_eq!(
        s.settings.get_str("recording_directory"),
        Some(rec.to_str().unwrap())
    );
}

#[test]
fn unusable_clips_folder_shows_storage_message() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    let before = s.settings.get_str("clips_directory").map(str::to_owned);
    let file = tmp.path().join("a-file");
    std::fs::write(&file, b"x").unwrap();
    apply(&mut s, BrowseKind::ClipsFolder, vec![file.join("inside")]);
    let m = messages(&s);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].0, "Storage location unavailable");
    assert!(
        m[0].1.starts_with("FTHR cannot use the storage folder"),
        "{}",
        m[0].1
    );
    assert!(m[0].1.contains("No files were moved."));
    assert_eq!(
        s.settings.get_str("clips_directory").map(str::to_owned),
        before
    );
    assert!(!s.clips_directory_changed);
}

#[test]
fn recording_folder_outside_the_root_is_linked() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    let rec = tmp.path().join("elsewhere").join("rec");
    apply(&mut s, BrowseKind::RecordingFolder, vec![rec.clone()]);
    assert!(rec.is_dir());
    assert_eq!(
        s.settings.get_str("recording_directory"),
        Some(rec.to_str().unwrap())
    );
    let imported = s.settings.get("imported_clip_folders").unwrap().clone();
    assert_eq!(imported, json!([rec.to_str().unwrap()]));
    assert!(s.imported_folders_changed);
}

#[test]
fn import_folder_dedupes() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    let a = tmp.path().join("a");
    std::fs::create_dir_all(&a).unwrap();
    apply(&mut s, BrowseKind::ImportFolder, vec![a.clone()]);
    apply(&mut s, BrowseKind::ImportFolder, vec![a.join(".")]);
    assert_eq!(
        s.settings.get("imported_clip_folders").unwrap(),
        &json!([a.to_str().unwrap()])
    );
    assert!(s.imported_folders_changed);
}

#[test]
fn gary_image_is_checked() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    apply(
        &mut s,
        BrowseKind::GaryImage,
        vec![junk(&tmp.path().join("x.png"))],
    );
    assert_eq!(
        messages(&s),
        [(
            "Image not supported".to_owned(),
            "Choose a valid PNG, JPG, WEBP, BMP, or GIF image.".to_owned()
        )]
    );
    assert!(s.settings.get_str("gary_image_path").is_none());
    let good = png(&tmp.path().join("g.png"));
    apply(&mut s, BrowseKind::GaryImage, vec![good.clone()]);
    assert_eq!(
        s.settings.get_str("gary_image_path"),
        Some(good.to_str().unwrap())
    );
}

#[test]
fn image_layers_append_readable_files() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    let a = png(&tmp.path().join("a.png"));
    let b = junk(&tmp.path().join("b.png"));
    let c = png(&tmp.path().join("c.png"));
    apply(
        &mut s,
        BrowseKind::ImageLayers,
        vec![a.clone(), b, c.clone()],
    );
    assert_eq!(s.settings.get_bool("image_overlay_enabled"), Some(true));
    let layers = fthrust_core::finalize::image_layers(&s.settings);
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0].path, a);
    assert_eq!(layers[1].path, c);
    // Start slots by index: (0.76, 0.04) then (0.04, 0.04).
    assert_eq!((layers[0].rect.x, layers[1].rect.x), (0.76, 0.04));
    assert!(
        messages(&s).is_empty(),
        "unreadable files are dropped silently"
    );
}

#[test]
fn preview_background_ignores_unreadable() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    apply(
        &mut s,
        BrowseKind::PreviewBackground,
        vec![junk(&tmp.path().join("x.jpg"))],
    );
    assert_eq!(
        s.settings
            .get_str("input_overlay_preview_background")
            .unwrap_or(""),
        ""
    );
    let good = png(&tmp.path().join("bg.png"));
    apply(&mut s, BrowseKind::PreviewBackground, vec![good.clone()]);
    assert_eq!(
        s.settings.get_str("input_overlay_preview_background"),
        Some(good.to_str().unwrap())
    );
}

#[test]
fn theme_archive_asks_then_imports() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    s.theme
        .data
        .colors
        .insert("ACCENT".into(), "#123456".into());
    let zip = tmp.path().join("t.zip");
    s.theme.export_zip(&zip).unwrap();
    s.theme
        .data
        .colors
        .insert("ACCENT".into(), "#abcdef".into());
    apply(&mut s, BrowseKind::ThemeArchive, vec![zip]);
    let (tag, message) = match s.dialogs.modal.front() {
        Some(DialogKind::Question { tag, message, .. }) => (tag.clone(), message.clone()),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        message,
        "This will fully replace your current theme (typography, colors, icons, and sounds).\n\nContinue?"
    );
    s.dialogs.modal.clear();
    s.dialogs
        .results
        .push(DialogResult::Answer { tag, yes: true });
    service(&mut s);
    assert_eq!(s.theme.color("ACCENT"), "#123456");
    assert_eq!(
        messages(&s),
        [(
            "Imported".to_owned(),
            "Theme imported successfully.\nClick APPLY THEME to see changes in the app.".to_owned()
        )]
    );
}

#[test]
fn font_file_registers_its_family_for_the_role() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    let font = fthrust_ui::assets_dir()
        .join("fonts")
        .join("Oswald-Bold.ttf");
    apply(&mut s, BrowseKind::FontFile("body".into()), vec![font]);
    assert_eq!(
        s.theme.data.fonts.get("body").map(String::as_str),
        Some("Oswald")
    );
    assert!(
        s.theme.data.font_files.contains_key("Oswald"),
        "{:?}",
        s.theme.data.font_files
    );
    apply(
        &mut s,
        BrowseKind::FontFile("display".into()),
        vec![junk(&tmp.path().join("bad.ttf"))],
    );
    assert_eq!(
        messages(&s),
        [(
            "Font not supported".to_owned(),
            "Choose a valid OpenType (.otf) or TrueType (.ttf) font file.".to_owned()
        )]
    );
}

#[test]
fn icon_and_sound_files_are_stored() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    apply(
        &mut s,
        BrowseKind::IconFile("close".into()),
        vec![png(&tmp.path().join("big.png"))],
    );
    let name = s.theme.data.icons.get("close").expect("icon set").clone();
    assert!(name.ends_with(".png"), "{name}");
    let wav = tmp.path().join("ding.wav");
    std::fs::write(&wav, b"RIFF").unwrap();
    apply(
        &mut s,
        BrowseKind::SoundFile("clip_captured".into()),
        vec![wav],
    );
    assert_eq!(
        s.theme.data.sounds.get("clip_captured").map(String::as_str),
        Some("clip_captured.wav")
    );
}

#[test]
fn cancel_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    apply(&mut s, BrowseKind::ClipsFolder, vec![]);
    apply(&mut s, BrowseKind::GaryImage, vec![]);
    assert!(!s.clips_directory_changed);
    assert!(messages(&s).is_empty());
}

#[test]
fn accepted_images_also_render() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 20, 30, 255]));
    for name in ["g.bmp", "g.gif", "g.webp"] {
        let p = tmp.path().join(name);
        img.save(&p).unwrap();
        apply(&mut s, BrowseKind::GaryImage, vec![p.clone()]);
        assert_eq!(
            s.settings.get_str("gary_image_path"),
            Some(p.to_str().unwrap()),
            "{name}"
        );
        // What the preview and the overlay decode.
        assert!(image::open(&p).is_ok(), "{name} renders");
    }
}

#[test]
fn unusable_icon_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = state(tmp.path());
    apply(
        &mut s,
        BrowseKind::IconFile("close".into()),
        vec![junk(&tmp.path().join("x.svg"))],
    );
    assert_eq!(messages(&s)[0].0, "Icon import failed");
}
