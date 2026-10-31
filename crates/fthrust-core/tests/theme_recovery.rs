//! A theme import that was killed before it finished is undone at the next
//! load: every folder it had parked is put back, whatever stands in its place.
//! One that got as far as replacing `theme.json` did finish and is left alone.

use std::fs;
use std::path::Path;

use fthrust_core::FthrPaths;
use fthrust_core::theme::ThemeStore;

const OLD_JSON: &[u8] = br#"{"old":true}"#;
const NEW_JSON: &[u8] = br#"{"new":true}"#;

fn home() -> (tempfile::TempDir, FthrPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(tmp.path());
    fs::create_dir_all(paths.root()).unwrap();
    (tmp, paths)
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
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
fn a_cut_short_import_is_undone_as_a_whole() {
    let (_t, p) = home();
    let staging = p.theme_dir().join(".import-fedcba9876543210");
    write(&p.theme_file(), OLD_JSON);
    write(&staging.join("old/theme.json"), OLD_JSON);
    // icons: already swapped, the live folder is the staged one.
    write(&p.theme_icons_dir().join("new.png"), b"NEW");
    write(&staging.join("old/icons/old.png"), b"OLD");
    // sounds: the app stopped between the two renames.
    write(&staging.join("old/sounds/old.wav"), b"OLD");
    write(&staging.join("new/sounds/half.wav"), b"H");
    // fonts: not reached, nothing parked.
    write(&p.theme_fonts_dir().join("mine.ttf"), b"MINE");

    let store = ThemeStore::load(p.clone()).unwrap();

    assert_eq!(
        fs::read(p.theme_icons_dir().join("old.png")).unwrap(),
        b"OLD",
        "a folder that was swapped in goes back too, not only the missing one"
    );
    assert!(!p.theme_icons_dir().join("new.png").exists());
    assert_eq!(
        fs::read(p.theme_sounds_dir().join("old.wav")).unwrap(),
        b"OLD"
    );
    assert_eq!(
        fs::read(p.theme_fonts_dir().join("mine.ttf")).unwrap(),
        b"MINE"
    );
    assert_eq!(fs::read(p.theme_file()).unwrap(), OLD_JSON);
    assert!(!staging.exists());
    let issues = store.startup_issues();
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert!(
        issues[0].contains("icons") && issues[0].contains("sounds") && !issues[0].contains("fonts"),
        "{issues:?}"
    );
}

#[test]
fn an_import_killed_after_all_its_swaps_but_before_theme_json_is_undone() {
    let (_t, p) = home();
    let staging = p.theme_dir().join(".import-0000000000000004");
    // Nothing is missing: all three folders are the staged ones, and
    // theme.json is still the old one, which they do not belong to.
    write(&p.theme_file(), OLD_JSON);
    write(&staging.join("old/theme.json"), OLD_JSON);
    for (live, parked, name) in [
        (p.theme_icons_dir(), "old/icons", "a.png"),
        (p.theme_sounds_dir(), "old/sounds", "a.wav"),
        (p.theme_fonts_dir(), "old/fonts", "a.ttf"),
    ] {
        write(&live.join("new"), b"NEW");
        write(&staging.join(parked).join(name), b"OLD");
    }

    let store = ThemeStore::load(p.clone()).unwrap();

    for (live, name) in [
        (p.theme_icons_dir(), "a.png"),
        (p.theme_sounds_dir(), "a.wav"),
        (p.theme_fonts_dir(), "a.ttf"),
    ] {
        assert_eq!(fs::read(live.join(name)).unwrap(), b"OLD");
        assert!(!live.join("new").exists());
    }
    assert!(!staging.exists());
    assert!(
        store
            .startup_issues()
            .iter()
            .any(|m| m.contains("icons") && m.contains("sounds") && m.contains("fonts")),
        "{:?}",
        store.startup_issues()
    );
}

#[test]
fn an_import_that_got_as_far_as_theme_json_is_left_as_it_is() {
    let (_t, p) = home();
    let staging = p.theme_dir().join(".import-0000000000000001");
    // Swapped, and theme.json replaced; the app was killed before the parked
    // old files were removed. The new theme is the one in force.
    write(&p.theme_file(), NEW_JSON);
    write(&staging.join("old/theme.json"), OLD_JSON);
    write(&p.theme_icons_dir().join("new.png"), b"NEW");
    write(&staging.join("old/icons/old.png"), b"OLD");

    let store = ThemeStore::load(p.clone()).unwrap();

    assert!(p.theme_icons_dir().join("new.png").exists());
    assert!(!p.theme_icons_dir().join("old.png").exists());
    assert_eq!(fs::read(p.theme_file()).unwrap(), NEW_JSON);
    assert!(!staging.exists(), "the old files are removed");
    assert!(store.startup_issues().is_empty());
}

#[test]
fn a_theme_json_that_did_not_exist_before_the_import_marks_its_finish() {
    let (_t, p) = home();
    let staging = p.theme_dir().join(".import-0000000000000002");
    // No theme.json to keep a copy of; there is one now.
    write(&p.theme_file(), NEW_JSON);
    write(&p.theme_icons_dir().join("new.png"), b"NEW");
    write(&staging.join("old/icons/old.png"), b"OLD");

    let store = ThemeStore::load(p.clone()).unwrap();

    assert!(p.theme_icons_dir().join("new.png").exists());
    assert!(!p.theme_icons_dir().join("old.png").exists());
    assert!(!staging.exists());
    assert!(store.startup_issues().is_empty());
}

/// The startup note an import that could not be undone leaves, naming where
/// the old files are.
fn stuck_note(store: &ThemeStore, staging: &Path) -> Option<String> {
    store
        .startup_issues()
        .iter()
        .find(|m| {
            m.contains("could not be undone")
                && m.contains(&staging.join("old").display().to_string())
        })
        .cloned()
}

#[test]
fn a_folder_that_cannot_be_moved_aside_leaves_everything_where_it_is() {
    let (_t, p) = home();
    let staging = p.theme_dir().join(".import-0000000000000003");
    // The live icons are the staged ones, the old ones are parked, and the
    // place the live folder would be moved to is taken by a file.
    write(&p.theme_file(), OLD_JSON);
    write(&staging.join("old/theme.json"), OLD_JSON);
    write(&p.theme_icons_dir().join("new.png"), b"NEW");
    write(&staging.join("old/icons/old.png"), b"OLD");
    write(&staging.join("undone"), b"in the way");

    let store = ThemeStore::load(p.clone()).unwrap();

    assert_eq!(
        fs::read(p.theme_icons_dir().join("new.png")).unwrap(),
        b"NEW",
        "the live folder is not deleted when it cannot be moved aside"
    );
    assert_eq!(
        fs::read(staging.join("old/icons/old.png")).unwrap(),
        b"OLD",
        "left for the next start"
    );
    let note = stuck_note(&store, &staging);
    assert!(note.is_some(), "{:?}", store.startup_issues());
    assert!(
        store
            .startup_issues()
            .iter()
            .all(|m| !m.contains("were put back")),
        "{:?}",
        store.startup_issues()
    );
}

#[test]
fn an_import_that_cannot_be_judged_is_left_alone_and_reported() {
    let (_t, p) = home();
    let staging = p.theme_dir().join(".import-0000000000000004");
    // theme.json cannot be read (it is a directory), so there is no telling
    // whether the import finished: nothing may be deleted.
    fs::create_dir_all(p.theme_file()).unwrap();
    write(&p.theme_icons_dir().join("new.png"), b"NEW");
    write(&staging.join("old/icons/old.png"), b"OLD");

    let store = ThemeStore::load(p.clone()).unwrap();

    assert!(p.theme_icons_dir().join("new.png").exists());
    assert!(staging.join("old/icons/old.png").exists());
    assert!(
        stuck_note(&store, &staging).is_some(),
        "{:?}",
        store.startup_issues()
    );
}

#[test]
fn whatever_stands_in_the_place_of_a_parked_folder_is_replaced_not_in_the_way() {
    let (_t, p) = home();
    let staging = p.theme_dir().join(".import-0000000000000005");
    // A file where the icons folder belongs does not stop the old folder
    // from coming back: it is moved aside with the rest.
    write(&p.theme_dir().join("icons"), b"in the way");
    write(&staging.join("old/icons/old.png"), b"OLD");

    let store = ThemeStore::load(p.clone()).unwrap();

    assert_eq!(
        fs::read(p.theme_icons_dir().join("old.png")).unwrap(),
        b"OLD"
    );
    assert!(!staging.exists());
    assert!(store.startup_issues().iter().any(|m| m.contains("icons")));
}
