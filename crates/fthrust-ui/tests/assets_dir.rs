//! Asset directory lookup order: packaged (next to the executable) first.

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
