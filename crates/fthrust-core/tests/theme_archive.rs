use fthrust_core::FthrPaths;
use fthrust_core::theme::*;
use std::io::Write;
use std::path::Path;

fn store() -> (tempfile::TempDir, ThemeStore) {
    let tmp = tempfile::tempdir().unwrap();
    let s = ThemeStore::load(FthrPaths::new(tmp.path())).unwrap();
    (tmp, s)
}

fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut zw = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    for (name, body) in entries {
        zw.start_file(*name, opts).unwrap();
        zw.write_all(body).unwrap();
    }
    zw.finish().unwrap();
}

#[test]
fn export_import_round_trip_preserves_fonts_and_icons() {
    let (tmp, mut a) = store();
    let src = tmp.path().join("my.png");
    std::fs::write(&src, b"PNGDATA").unwrap();
    a.set_custom_icon("record.svg", &src).unwrap();
    let font = tmp.path().join("f.ttf");
    std::fs::write(&font, b"FONT").unwrap();
    a.set_custom_font(&["Foo".into(), "Foo Bold".into()], &font)
        .unwrap();
    let zip_path = tmp.path().join("out.zip");
    a.export_zip(&zip_path).unwrap();

    let tmp2 = tempfile::tempdir().unwrap();
    let mut b = ThemeStore::load(FthrPaths::new(tmp2.path())).unwrap();
    b.import_zip(&zip_path).unwrap();
    assert_eq!(b.data.icons, a.data.icons);
    assert_eq!(b.data.font_files, a.data.font_files);
    assert_eq!(
        std::fs::read(b.custom_icon_path("record.svg").unwrap()).unwrap(),
        b"PNGDATA"
    );
    assert_eq!(b.custom_font_paths().len(), 2);
    let reloaded = ThemeStore::load(FthrPaths::new(tmp2.path())).unwrap();
    assert_eq!(reloaded.data, b.data);
}

#[test]
fn import_without_theme_json_fails_and_changes_nothing() {
    let (tmp, mut s) = store();
    let src = tmp.path().join("my.png");
    std::fs::write(&src, b"X").unwrap();
    s.set_custom_icon("record.svg", &src).unwrap();
    let before = s.data.clone();
    let z = tmp.path().join("bad.zip");
    make_zip(&z, &[("icons/other.png", b"Y")]);
    assert!(matches!(
        s.import_zip(&z),
        Err(fthrust_core::CoreError::Invalid(_))
    ));
    assert_eq!(s.data, before);
    assert!(s.custom_icon_path("record.svg").is_some());

    let z2 = tmp.path().join("bad2.zip");
    make_zip(&z2, &[("theme.json", br#"{"colors": 3}"#)]);
    assert!(s.import_zip(&z2).is_err());
    assert_eq!(s.data, before);
    assert!(s.custom_icon_path("record.svg").is_some());
}

#[test]
fn import_wipes_previous_assets() {
    let (tmp, mut s) = store();
    let src = tmp.path().join("my.png");
    std::fs::write(&src, b"X").unwrap();
    s.set_custom_icon("record.svg", &src).unwrap();
    let old = s.custom_icon_path("record.svg").unwrap();
    let z = tmp.path().join("t.zip");
    make_zip(
        &z,
        &[
            ("theme.json", b"{}"),
            ("icons/new.png", b"N"),
            ("../evil.txt", b"E"),
            ("icons/../escape.png", b"E"),
            ("icons/sub/deep.png", b"E"),
            ("/abs.png", b"E"),
        ],
    );
    s.import_zip(&z).unwrap();
    assert!(!old.exists());
    assert!(old.parent().unwrap().join("new.png").exists());
    assert!(!old.parent().unwrap().join("escape.png").exists());
    assert!(
        !old.parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("escape.png")
            .exists()
    );
    assert!(!tmp.path().join("evil.txt").exists());
    assert!(s.data.icons.is_empty());
}

#[test]
fn import_unknown_font_family_falls_back_to_oswald() {
    let (tmp, mut s) = store();
    let z = tmp.path().join("t.zip");
    make_zip(
        &z,
        &[(
            "theme.json",
            br#"{"fonts":{"body":"Nonexistent","display":"Kept"},"font_files":{"Kept":"k.ttf"}}"#,
        )],
    );
    s.import_zip(&z).unwrap();
    assert_eq!(s.data.fonts["body"], "Oswald");
    assert_eq!(s.data.fonts["display"], "Kept");
    assert_eq!(s.data.palette_version, 4);
    assert_eq!(s.data.typography_version, 2);
}

#[test]
fn set_custom_icon_same_file_is_not_truncated() {
    let (_tmp, mut s) = store();
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("a.png");
    std::fs::write(&src, b"DATA").unwrap();
    s.set_custom_icon("record.svg", &src).unwrap();
    let installed = s.custom_icon_path("record.svg").unwrap();
    s.set_custom_icon("record.svg", &installed).unwrap();
    assert_eq!(std::fs::read(&installed).unwrap(), b"DATA");
    assert_eq!(s.data.icons["record.svg"], "record.png");
}

#[test]
fn set_custom_sound_rejects_unknown_key() {
    let (tmp, mut s) = store();
    let src = tmp.path().join("a.wav");
    std::fs::write(&src, b"W").unwrap();
    assert!(matches!(
        s.set_custom_sound("../evil", &src),
        Err(fthrust_core::CoreError::Invalid(_))
    ));
    assert!(s.data.sounds.is_empty());
}
