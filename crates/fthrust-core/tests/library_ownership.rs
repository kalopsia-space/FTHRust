use fthrust_core::CoreError;
use fthrust_core::library_ownership::{
    Ownership, add_import_root, classify_media_path, delete_owned_media, normalize_import_root,
    remove_import_root,
};
use std::fs;
use std::path::PathBuf;

struct Fx {
    _d: tempfile::TempDir,
    owned: PathBuf,
    imported: PathBuf,
    external: PathBuf,
}

fn fx() -> Fx {
    let d = tempfile::tempdir().unwrap();
    let base = d.path().canonicalize().unwrap();
    let owned = base.join("owned");
    let imported = base.join("imported");
    let external = base.join("external");
    for p in [&owned, &imported, &external] {
        fs::create_dir_all(p).unwrap();
        fs::write(p.join("a.mp4"), b"x").unwrap();
    }
    Fx {
        _d: d,
        owned,
        imported,
        external,
    }
}

#[test]
fn owned_can_delete() {
    let f = fx();
    let p = f.owned.join("a.mp4");
    assert_eq!(
        classify_media_path(&p, &f.owned, std::slice::from_ref(&f.imported)),
        Ownership::FthrOwned
    );
    delete_owned_media(&p, &f.owned, std::slice::from_ref(&f.imported)).unwrap();
    assert!(!p.exists());
}

#[test]
fn imported_is_protected() {
    let f = fx();
    let p = f.imported.join("a.mp4");
    let imp = [f.imported.clone()];
    assert_eq!(
        classify_media_path(&p, &f.owned, &imp),
        Ownership::LinkedImported
    );
    // imported wins even when nested inside the owned root
    let nested = f.owned.join("sub");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("n.mp4"), b"x").unwrap();
    assert_eq!(
        classify_media_path(
            &nested.join("n.mp4"),
            &f.owned,
            std::slice::from_ref(&nested)
        ),
        Ownership::LinkedImported
    );
    let e = delete_owned_media(&p, &f.owned, &imp).unwrap_err();
    assert!(matches!(e, CoreError::Invalid(m) if m == "Linked or external media is protected"));
    assert!(p.exists());
}

#[test]
fn external_is_protected() {
    let f = fx();
    let p = f.external.join("a.mp4");
    assert_eq!(classify_media_path(&p, &f.owned, &[]), Ownership::External);
    assert!(delete_owned_media(&p, &f.owned, &[]).is_err());
    assert!(p.exists());
}

#[test]
fn symlink_inside_owned_to_external_is_external() {
    let f = fx();
    let link = f.owned.join("link.mp4");
    std::os::unix::fs::symlink(f.external.join("a.mp4"), &link).unwrap();
    assert_eq!(
        classify_media_path(&link, &f.owned, &[]),
        Ownership::External
    );
    assert!(delete_owned_media(&link, &f.owned, &[]).is_err());
    assert!(f.external.join("a.mp4").exists());
}

#[test]
fn add_import_root_dedupes_equivalent_paths() {
    let f = fx();
    let mut list = vec![];
    assert!(add_import_root(&mut list, &f.imported));
    let alias = f.imported.join("..").join("imported");
    assert!(!add_import_root(&mut list, &alias));
    assert!(!add_import_root(&mut list, &f.imported));
    assert_eq!(list.len(), 1);
    assert_eq!(normalize_import_root(&alias).unwrap(), f.imported);
    assert!(add_import_root(&mut list, &f.external));
    assert_eq!(list.len(), 2);
}

#[test]
fn delete_removes_sidecar_manifest() {
    let f = fx();
    let p = f.owned.join("a.mp4");
    let side = f.owned.join("a.mp4.fthr-audio.json");
    fs::write(&side, b"{}").unwrap();
    delete_owned_media(&p, &f.owned, &[]).unwrap();
    assert!(!p.exists() && !side.exists());
}

#[test]
fn remove_import_root_only_changes_registration() {
    let f = fx();
    let mut list = vec![];
    add_import_root(&mut list, &f.imported);
    let alias = f.imported.join("..").join("imported");
    assert!(remove_import_root(&mut list, &alias));
    assert!(!remove_import_root(&mut list, &f.imported));
    assert!(list.is_empty());
    assert!(f.imported.join("a.mp4").exists());
}
