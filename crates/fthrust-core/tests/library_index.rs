use fthrust_core::library_index::{
    Filter, LibraryRecord, MediaKind, ScanRoots, Sort, apply, scan, watched_dirs,
};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

fn touch(p: &Path, bytes: usize) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, vec![0u8; bytes]).unwrap();
}

fn roots(owned: &Path) -> ScanRoots {
    ScanRoots {
        owned: owned.to_path_buf(),
        imported: vec![],
    }
}

fn run(r: &ScanRoots) -> Vec<LibraryRecord> {
    scan(r, &AtomicBool::new(false), |_| {}).unwrap()
}

#[test]
fn scan_finds_videos_and_images_with_kinds() {
    let d = tempfile::tempdir().unwrap();
    touch(&d.path().join("a/x.mp4"), 3);
    touch(&d.path().join("Screenshots/s.png"), 2);
    touch(&d.path().join("notes.txt"), 1);
    let mut recs = run(&roots(d.path()));
    recs.sort_by_key(|r| r.kind == MediaKind::Image);
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0].kind, MediaKind::Video);
    assert_eq!(recs[0].size, 3);
    assert_eq!(recs[1].kind, MediaKind::Image);
    assert!(!recs[0].imported);
    assert_eq!(recs[0].root_ids, vec![0]);
}

#[test]
fn scan_skips_partials_and_post_processing_dirs() {
    let d = tempfile::tempdir().unwrap();
    touch(&d.path().join("ok.mp4"), 1);
    touch(&d.path().join("x.mp4.partial"), 1);
    touch(&d.path().join("Exported/e.mp4"), 1);
    touch(&d.path().join("Shared/s.mp4"), 1);
    touch(&d.path().join(".fthr-tmp/t.mp4"), 1);
    let recs = run(&roots(d.path()));
    assert_eq!(recs.len(), 1);
    assert!(recs[0].path.ends_with("ok.mp4"));
}

#[test]
fn scan_skips_symlinked_dirs() {
    let d = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    touch(&outside.path().join("secret.mp4"), 1);
    touch(&outside.path().join("f.mp4"), 1);
    touch(&d.path().join("in.mp4"), 1);
    std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("f.mp4"), d.path().join("fl.mp4")).unwrap();
    let recs = run(&roots(d.path()));
    assert_eq!(recs.len(), 1);
    assert!(recs[0].path.ends_with("in.mp4"));
}

#[test]
fn scan_dedupes_overlapping_roots() {
    let d = tempfile::tempdir().unwrap();
    touch(&d.path().join("sub/a.mp4"), 1);
    touch(&d.path().join("b.mp4"), 1);
    let r = ScanRoots {
        owned: d.path().to_path_buf(),
        imported: vec![d.path().join("sub")],
    };
    let mut recs = run(&r);
    recs.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(recs.len(), 2);
    let a = recs.iter().find(|x| x.path.ends_with("a.mp4")).unwrap();
    assert_eq!(a.root_ids, vec![0, 1]);
    assert!(a.imported);
    let b = recs.iter().find(|x| x.path.ends_with("b.mp4")).unwrap();
    assert_eq!(b.root_ids, vec![0]);
    assert!(!b.imported);
}

#[test]
fn cancelled_scan_publishes_no_further_batch() {
    let d = tempfile::tempdir().unwrap();
    for i in 0..300 {
        touch(&d.path().join(format!("c{i}.mp4")), 1);
    }
    let r = roots(d.path());
    let pre = AtomicBool::new(true);
    let mut n = 0;
    assert!(scan(&r, &pre, |_| n += 1).unwrap().is_empty());
    assert_eq!(n, 0);

    let cancel = AtomicBool::new(false);
    let mut batches = 0;
    let out = scan(&r, &cancel, |_| {
        batches += 1;
        cancel.store(true, Ordering::SeqCst);
    })
    .unwrap();
    assert!(out.is_empty());
    assert_eq!(batches, 1);
}

#[test]
fn batches_published_before_end() {
    let d = tempfile::tempdir().unwrap();
    for i in 0..200 {
        touch(&d.path().join(format!("c{i}.mp4")), 1);
    }
    let mut sizes = vec![];
    let out = scan(&roots(d.path()), &AtomicBool::new(false), |b| {
        sizes.push(b.len())
    })
    .unwrap();
    assert_eq!(out.len(), 200);
    assert!(sizes.len() >= 2);
    assert_eq!(sizes[0], fthrust_core::library_index::DEFAULT_BATCH_SIZE);
    assert_eq!(sizes.iter().sum::<usize>(), 200);
}

#[test]
fn watched_dirs_are_capped() {
    let d = tempfile::tempdir().unwrap();
    for i in 0..300 {
        fs::create_dir_all(d.path().join(format!("d{i}"))).unwrap();
    }
    let w = watched_dirs(&roots(d.path()));
    assert_eq!(w.len(), 256);
    assert_eq!(w[0], d.path());
}

#[test]
fn sorts_and_filters() {
    let d = tempfile::tempdir().unwrap();
    touch(&d.path().join("old.mp4"), 10);
    touch(&d.path().join("new.mp4"), 5);
    touch(&d.path().join("tie.mp4"), 5);
    touch(&d.path().join("shot.png"), 1);
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for (n, off) in [
        ("old.mp4", 0),
        ("new.mp4", 300),
        ("tie.mp4", 200),
        ("shot.png", 100),
    ] {
        let f = fs::File::options()
            .write(true)
            .open(d.path().join(n))
            .unwrap();
        f.set_modified(base + Duration::from_secs(off)).unwrap();
    }
    let r = ScanRoots {
        owned: d.path().to_path_buf(),
        imported: vec![],
    };
    let recs = run(&r);
    let names = |v: Vec<LibraryRecord>| -> Vec<String> {
        v.iter()
            .map(|x| x.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    };
    assert_eq!(
        names(apply(&recs, Filter::All, Sort::Newest)),
        ["new.mp4", "tie.mp4", "shot.png", "old.mp4"]
    );
    assert_eq!(
        names(apply(&recs, Filter::All, Sort::Oldest)),
        ["old.mp4", "shot.png", "tie.mp4", "new.mp4"]
    );
    assert_eq!(
        names(apply(&recs, Filter::All, Sort::Longest)),
        ["old.mp4", "new.mp4", "tie.mp4", "shot.png"]
    );
    assert_eq!(
        names(apply(&recs, Filter::Screenshots, Sort::Newest)),
        ["shot.png"]
    );
    assert_eq!(apply(&recs, Filter::Clips, Sort::Newest).len(), 3);
    assert!(apply(&recs, Filter::Imported, Sort::Newest).is_empty());
}

#[test]
fn duplicate_import_root_is_ignored() {
    let d = tempfile::tempdir().unwrap();
    touch(&d.path().join("a.mp4"), 1);
    let r = ScanRoots {
        owned: d.path().to_path_buf(),
        imported: vec![
            d.path().to_path_buf(),
            d.path().join("."),
            d.path().to_path_buf(),
        ],
    };
    let recs = run(&r);
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].root_ids, vec![0]);
    assert!(!recs[0].imported);
    assert_eq!(watched_dirs(&r), vec![d.path().to_path_buf()]);
}
