//! A theme import that was killed between its renames is undone at the next
//! load: the folder it had parked is put back.

use std::fs;

use fthrust_core::FthrPaths;
use fthrust_core::theme::ThemeStore;

fn home() -> (tempfile::TempDir, FthrPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(tmp.path());
    fs::create_dir_all(paths.root()).unwrap();
    (tmp, paths)
}

#[test]
fn a_folder_parked_by_an_import_that_was_cut_short_is_put_back() {
    let (_t, p) = home();
    // The state an import killed between renaming `icons` away and renaming
    // the staged one in leaves: no live icons folder, the old one parked.
    fs::create_dir_all(p.theme_dir().join("sounds")).unwrap();
    fs::create_dir_all(p.theme_dir().join("fonts")).unwrap();
    let staging = p.theme_dir().join(".import-0123456789abcdef");
    fs::create_dir_all(staging.join("old/icons")).unwrap();
    fs::write(staging.join("old/icons/mine.png"), b"MINE").unwrap();
    fs::create_dir_all(staging.join("new/icons")).unwrap();
    fs::write(staging.join("new/icons/half.png"), b"H").unwrap();
    let store = ThemeStore::load(p.clone()).unwrap();
    assert_eq!(
        fs::read(p.theme_icons_dir().join("mine.png")).unwrap(),
        b"MINE",
        "the previous icons are back"
    );
    assert!(!p.theme_icons_dir().join("half.png").exists());
    assert!(!staging.exists(), "the staging is cleaned up");
    assert!(
        store
            .startup_issues()
            .iter()
            .any(|m| m.contains("import was interrupted") && m.contains("icons")),
        "{:?}",
        store.startup_issues()
    );
}

#[test]
fn every_folder_it_had_parked_comes_back() {
    let (_t, p) = home();
    let staging = p.theme_dir().join(".import-aaaaaaaaaaaaaaaa");
    for folder in ["icons", "sounds", "fonts"] {
        fs::create_dir_all(staging.join("old").join(folder)).unwrap();
        fs::write(staging.join("old").join(folder).join("x"), folder).unwrap();
    }
    let store = ThemeStore::load(p.clone()).unwrap();
    assert_eq!(fs::read(p.theme_icons_dir().join("x")).unwrap(), b"icons");
    assert_eq!(fs::read(p.theme_sounds_dir().join("x")).unwrap(), b"sounds");
    assert_eq!(fs::read(p.theme_fonts_dir().join("x")).unwrap(), b"fonts");
    assert!(!staging.exists());
    assert_eq!(store.startup_issues().len(), 1);
}

#[test]
fn a_folder_that_exists_is_never_replaced_by_a_parked_one() {
    let (_t, p) = home();
    // The swap got as far as this folder: the live one is the new one.
    fs::create_dir_all(p.theme_icons_dir()).unwrap();
    fs::write(p.theme_icons_dir().join("new.png"), b"NEW").unwrap();
    let staging = p.theme_dir().join(".import-fedcba9876543210");
    fs::create_dir_all(staging.join("old/icons")).unwrap();
    fs::write(staging.join("old/icons/old.png"), b"OLD").unwrap();
    let store = ThemeStore::load(p.clone()).unwrap();
    assert_eq!(
        fs::read(p.theme_icons_dir().join("new.png")).unwrap(),
        b"NEW"
    );
    assert!(!p.theme_icons_dir().join("old.png").exists());
    assert!(!staging.exists());
    assert!(store.startup_issues().is_empty());
}
