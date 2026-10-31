//! A theme archive is untrusted input, and an import must not leave a
//! half-replaced theme behind.

use fthrust_core::FthrPaths;
use fthrust_core::theme::*;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

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

fn theme_dir_names(tmp: &tempfile::TempDir) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(FthrPaths::new(tmp.path()).theme_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// A store with one custom icon, and the archive path for the next import.
fn store_with_icon() -> (tempfile::TempDir, ThemeStore, PathBuf) {
    let (tmp, mut s) = store();
    let src = tmp.path().join("mine.png");
    std::fs::write(&src, b"MINE").unwrap();
    s.set_custom_icon("record.svg", &src).unwrap();
    let z = tmp.path().join("t.zip");
    (tmp, s, z)
}

fn assert_theme_untouched(tmp: &tempfile::TempDir, s: &ThemeStore, before: &ThemeData) {
    assert_eq!(&s.data, before);
    assert_eq!(
        std::fs::read(s.custom_icon_path("record.svg").unwrap()).unwrap(),
        b"MINE"
    );
    assert_eq!(
        theme_dir_names(tmp),
        ["fonts", "icons", "sounds"],
        "no staging directory is left behind"
    );
}

/// Writes `mib` MiB of zeros as one deflated entry.
fn zero_entry<W: Write + Seek>(zw: &mut zip::ZipWriter<W>, name: &str, mib: usize) {
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(1));
    zw.start_file(name, opts).unwrap();
    let chunk = vec![0u8; 1024 * 1024];
    for _ in 0..mib {
        zw.write_all(&chunk).unwrap();
    }
}

#[test]
fn an_archive_with_too_many_entries_is_refused() {
    let (tmp, mut s, z) = store_with_icon();
    let before = s.data.clone();
    let mut zw = zip::ZipWriter::new(std::fs::File::create(&z).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    zw.start_file("theme.json", opts).unwrap();
    zw.write_all(b"{}").unwrap();
    for i in 0..MAX_ENTRIES {
        zw.start_file(format!("icons/n{i}.png"), opts).unwrap();
        zw.write_all(b"x").unwrap();
    }
    zw.finish().unwrap();
    let err = s.import_zip(&z).unwrap_err();
    assert!(
        matches!(&err, fthrust_core::CoreError::Invalid(m) if m.contains("too many entries")),
        "{err}"
    );
    assert_theme_untouched(&tmp, &s, &before);
}

#[test]
fn assets_that_add_up_to_too_much_are_refused() {
    let (tmp, mut s, z) = store_with_icon();
    let before = s.data.clone();
    let mut zw = zip::ZipWriter::new(std::fs::File::create(&z).unwrap());
    zw.start_file("theme.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    zw.write_all(b"{}").unwrap();
    // Each is under the single-asset limit; together they are a zip bomb.
    let mib = (MAX_ASSET_BYTES / (1024 * 1024)) as usize - 4;
    let count = (MAX_TOTAL_ASSET_BYTES / (mib as u64 * 1024 * 1024)) as usize + 1;
    for i in 0..count {
        zero_entry(&mut zw, &format!("sounds/s{i}.wav"), mib);
    }
    zw.finish().unwrap();
    let err = s.import_zip(&z).unwrap_err();
    assert!(
        matches!(&err, fthrust_core::CoreError::Invalid(m) if m.contains("too large together")),
        "{err}"
    );
    assert_theme_untouched(&tmp, &s, &before);
}

#[test]
fn a_giant_theme_json_is_refused_before_it_is_parsed() {
    let (tmp, mut s, z) = store_with_icon();
    let before = s.data.clone();
    let mut zw = zip::ZipWriter::new(std::fs::File::create(&z).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    zw.start_file("theme.json", opts).unwrap();
    zw.write_all(b"{").unwrap();
    zw.write_all(&vec![b' '; MAX_CONFIG_BYTES as usize])
        .unwrap();
    zw.write_all(b"}").unwrap();
    zw.finish().unwrap();
    assert!(matches!(
        s.import_zip(&z),
        Err(fthrust_core::CoreError::Invalid(_))
    ));
    assert_theme_untouched(&tmp, &s, &before);
}

#[test]
fn a_failed_import_keeps_the_previous_assets_and_theme() {
    let (tmp, mut s, z) = store_with_icon();
    let before = s.data.clone();
    make_zip(
        &z,
        &[
            ("theme.json", br#"{"icons":{"play.png":"new.png"}}"#),
            ("icons/new.png", b"NEW"),
            ("sounds/ding.wav", b"WAV"),
        ],
    );
    // theme.json cannot be written (a directory is in the way), which only
    // shows up after the assets are extracted.
    let paths = FthrPaths::new(tmp.path());
    std::fs::create_dir(paths.theme_file()).unwrap();
    assert!(s.import_zip(&z).is_err());
    assert!(
        !paths.theme_icons_dir().join("new.png").exists(),
        "the new assets did not go live"
    );
    assert!(!paths.theme_sounds_dir().join("ding.wav").exists());
    std::fs::remove_dir(paths.theme_file()).unwrap();
    assert_theme_untouched(&tmp, &s, &before);
}

#[test]
fn stored_and_deflated_entries_both_import() {
    let (tmp, mut s, z) = store_with_icon();
    let mut zw = zip::ZipWriter::new(std::fs::File::create(&z).unwrap());
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zw.start_file("theme.json", stored).unwrap();
    zw.write_all(br#"{"icons":{"play.png":"a.png","pause.png":"b.png"}}"#)
        .unwrap();
    zw.start_file("icons/a.png", stored).unwrap();
    zw.write_all(b"AAAA").unwrap();
    zw.start_file("icons/b.png", deflated).unwrap();
    zw.write_all(&vec![b'B'; 5000]).unwrap();
    zw.finish().unwrap();
    s.import_zip(&z).unwrap();
    let icons = FthrPaths::new(tmp.path()).theme_icons_dir();
    assert_eq!(std::fs::read(icons.join("a.png")).unwrap(), b"AAAA");
    assert_eq!(std::fs::read(icons.join("b.png")).unwrap().len(), 5000);
    assert_eq!(
        theme_dir_names(&tmp),
        ["fonts", "icons", "sounds", "theme.json"]
    );
}

#[test]
fn the_staging_left_by_a_crashed_import_is_cleaned_up() {
    let (tmp, mut s, z) = store_with_icon();
    let stale = FthrPaths::new(tmp.path())
        .theme_dir()
        .join(".import-0123456789abcdef/new/icons");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("half.png"), b"H").unwrap();
    make_zip(&z, &[("theme.json", b"{}")]);
    s.import_zip(&z).unwrap();
    assert_eq!(
        theme_dir_names(&tmp),
        ["fonts", "icons", "sounds", "theme.json"]
    );
}
