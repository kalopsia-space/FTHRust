//! Asset directory lookup order: packaged (next to the executable), then
//! installed (`<prefix>/share/fthrust/assets`), then the repository.

use fthrust_ui::assets_dir_for;
use std::path::PathBuf;

#[test]
fn exe_adjacent_assets_win_over_dev_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(bin.join("assets")).unwrap();
    let dev = tmp.path().join("dev");
    std::fs::create_dir_all(&dev).unwrap();
    let got = assets_dir_for(Some(&bin.join("fthrust")), &dev);
    assert_eq!(got, bin.join("assets"));
}

#[test]
fn dev_tree_used_when_no_adjacent_assets() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let dev = tmp.path().join("dev");
    std::fs::create_dir_all(&dev).unwrap();
    let got = assets_dir_for(Some(&bin.join("fthrust")), &dev);
    assert_eq!(got, dev.canonicalize().unwrap());
}

#[test]
fn falls_back_to_relative_assets() {
    let tmp = tempfile::tempdir().unwrap();
    let got = assets_dir_for(None, &tmp.path().join("missing"));
    assert_eq!(got, PathBuf::from("assets"));
}

#[test]
fn installed_prefix_assets_used_when_no_adjacent_assets() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let share = tmp.path().join("share/fthrust/assets");
    std::fs::create_dir_all(&share).unwrap();
    let dev = tmp.path().join("dev");
    std::fs::create_dir_all(&dev).unwrap();
    let got = assets_dir_for(Some(&bin.join("fthrust")), &dev);
    assert_eq!(got, share.canonicalize().unwrap());
}

#[test]
fn adjacent_assets_win_over_installed_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(bin.join("assets")).unwrap();
    std::fs::create_dir_all(tmp.path().join("share/fthrust/assets")).unwrap();
    let got = assets_dir_for(Some(&bin.join("fthrust")), &tmp.path().join("dev"));
    assert_eq!(got, bin.join("assets"));
}
