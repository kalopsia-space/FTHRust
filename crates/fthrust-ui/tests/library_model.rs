//! Plan 5 Task 5: the library model and the scan service.

use std::path::PathBuf;
use std::time::Duration;

use fthrust_core::clip_readiness::ClipReadinessRegistry;
use fthrust_core::library_index::{Filter, LibraryRecord, MediaKind, Sort};
use fthrust_ui::library::model::{
    EmptyState, Library, game_name, heading, section_label, title_from_path,
};
use fthrust_ui::library::scan::{ScanEvent, ScanKind, ScanService};

fn record(path: &str, kind: MediaKind, mtime_ns: i128, imported: bool) -> LibraryRecord {
    LibraryRecord {
        path: PathBuf::from(path),
        identity: PathBuf::from(path),
        root_ids: vec![0],
        imported,
        kind,
        size: 1,
        mtime_ns,
    }
}

const DAY: i128 = 86_400_000_000_000;

#[test]
fn sections_group_by_day_and_name_game_folder() {
    let root = PathBuf::from("/clips");
    let mut lib = Library::new(root.clone());
    let base: i128 = 1_800_000_000 * 1_000_000_000; // 2027-01-15, stable local label below
    lib.records = vec![
        record(
            "/clips/Desktop/desktop_clip_from_15Jan2027_10-00-00.mp4",
            MediaKind::Video,
            base,
            false,
        ),
        record(
            "/clips/Overwatch/Overwatch_clip_from_15Jan2027_09-00-00.mp4",
            MediaKind::Video,
            base - 3_600_000_000_000,
            false,
        ),
        record(
            "/clips/Screenshots/screenshot_from_20270114_101500_000000.png",
            MediaKind::Image,
            base - DAY,
            false,
        ),
        record("/imports/old.mp4", MediaKind::Video, base - 2 * DAY, true),
    ];
    lib.rebuild("F12");
    assert_eq!(lib.count, 4);
    assert!(lib.empty.is_none());
    assert_eq!(lib.sections.len(), 3);
    assert_eq!(lib.sections[0].label, section_label(base));
    assert_eq!(
        lib.sections[0].paths.len(),
        2,
        "same day: desktop then game clip (newest first)"
    );
    assert_eq!(
        lib.sections[0].game.as_deref(),
        Some("Desktop"),
        "the newest file's folder names the section"
    );
    assert_eq!(lib.sections[1].game.as_deref(), Some("Screenshots"));
    assert_eq!(
        game_name(&PathBuf::from("/clips/Overwatch/x.mp4"), &root),
        "Overwatch"
    );
    assert_eq!(game_name(&PathBuf::from("/clips/x.mp4"), &root), "Desktop");
    assert_eq!(
        title_from_path(&PathBuf::from(
            "/clips/Desktop/desktop_clip_from_15Jan2027_10-00-00.mp4"
        )),
        "desktop_clip_from_15Jan2027_10-00-00"
    );
    assert_eq!(lib.heading_text(), "ALL CLIPS  (4)");
    lib.sort = Sort::Oldest;
    lib.rebuild("F12");
    assert_eq!(lib.sections[0].paths[0], PathBuf::from("/imports/old.mp4"));
    lib.filter = Filter::Imported;
    lib.rebuild("F12");
    assert_eq!(lib.count, 1);
    assert_eq!(lib.heading_text(), "IMPORTED CLIPS  (1)");
}

#[test]
fn empty_state_texts_per_filter() {
    let mut lib = Library::new(PathBuf::from("/clips"));
    lib.rebuild("F12");
    assert_eq!(
        lib.empty,
        Some(EmptyState {
            title: "No clips yet".into(),
            detail: None
        })
    );
    assert_eq!(
        lib.heading_text(),
        "All clips",
        "initial label until a scan ran"
    );
    lib.scanned = true;
    assert_eq!(lib.heading_text(), "ALL CLIPS");
    lib.filter = Filter::Screenshots;
    lib.rebuild("F12");
    assert_eq!(
        lib.empty,
        Some(EmptyState {
            title: "No screenshots yet".into(),
            detail: Some("PRESS F12 TO TAKE A SCREENSHOT".into())
        })
    );
    lib.filter = Filter::Imported;
    lib.rebuild("F12");
    assert_eq!(lib.empty.as_ref().unwrap().title, "No imported clips");
    lib.filter = Filter::Clips;
    lib.rebuild("F12");
    assert_eq!(lib.empty.as_ref().unwrap().title, "No clips yet");
    assert_eq!(heading(Filter::All), "ALL CLIPS");
    assert_eq!(heading(Filter::Clips), "CLIPS");
    assert_eq!(heading(Filter::Screenshots), "SCREENSHOTS");
    assert_eq!(heading(Filter::Imported), "IMPORTED CLIPS");
}

#[test]
fn upsert_rename_remove_keep_order() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("clips");
    std::fs::create_dir_all(root.join("Desktop")).unwrap();
    let a = root.join("Desktop/a.mp4");
    let b = root.join("Desktop/b.mp4");
    std::fs::write(&a, b"a").unwrap();
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&b, b"b").unwrap();
    let mut lib = Library::new(root.clone());
    assert!(lib.upsert(&a, false));
    assert!(lib.upsert(&b, false));
    assert!(
        !lib.upsert(&b, false),
        "re-upsert of the same file is not a change"
    );
    lib.rebuild("F12");
    assert_eq!(
        lib.sections[0].paths,
        vec![b.clone(), a.clone()],
        "newest first"
    );
    let c = root.join("Desktop/c.mp4");
    std::fs::rename(&b, &c).unwrap();
    lib.rename(&b, &c);
    lib.rebuild("F12");
    assert_eq!(lib.sections[0].paths, vec![c.clone(), a.clone()]);
    lib.remove(&a);
    lib.rebuild("F12");
    assert_eq!(lib.sections[0].paths, vec![c.clone()]);
    assert!(
        !lib.upsert(&root.join("missing.mp4"), false),
        "a missing file is not inserted"
    );
}

#[test]
fn adjacent_skips_images_and_unready() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let v1 = root.join("v1.mp4");
    let shot = root.join("s.png");
    let v2 = root.join("v2.mp4");
    let v3 = root.join("v3.mp4");
    for (i, p) in [&v1, &shot, &v2, &v3].iter().enumerate() {
        std::fs::write(p, b"x").unwrap();
        let t = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + i as u64);
        std::fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }
    let mut lib = Library::new(root.clone());
    for p in [&v1, &shot, &v2, &v3] {
        lib.upsert(p, false);
    }
    lib.rebuild("F12");
    let mut ready = ClipReadinessRegistry::new();
    ready.engine_committed(&v2, true);
    assert_eq!(
        lib.navigation_paths(false),
        vec![v3.clone(), v2.clone(), shot.clone(), v1.clone()]
    );
    assert_eq!(
        lib.navigation_paths_videos(&ready),
        vec![v3.clone(), v1.clone()],
        "images and unready clips excluded"
    );
    assert_eq!(
        lib.adjacent(&v3, 1, &ready),
        Some(v1.clone()),
        "skips the finalizing clip and the screenshot"
    );
    assert_eq!(lib.adjacent(&v1, -1, &ready), Some(v3.clone()));
    assert_eq!(lib.adjacent(&v1, 1, &ready), None);
    assert_eq!(lib.adjacent(&root.join("nope.mp4"), 1, &ready), None);
}

#[test]
fn scan_service_reports_batches_and_watches_the_root() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("clips");
    std::fs::create_dir_all(root.join("Desktop")).unwrap();
    std::fs::write(root.join("Desktop/one.mp4"), b"1").unwrap();
    let svc = ScanService::start(root.clone(), vec![]);
    svc.request(ScanKind::Foreground);
    let mut done = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        match svc.events().recv_timeout(Duration::from_millis(200)) {
            Ok(ScanEvent::Done { records, kind, .. }) => {
                done = Some((records, kind));
                break;
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    let (records, kind) = done.expect("scan completes");
    assert_eq!(kind, ScanKind::Foreground);
    assert_eq!(records.len(), 1);
    // A new file under the watched root triggers a coalesced watcher scan.
    std::fs::write(root.join("Desktop/two.mp4"), b"2").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut watched = false;
    while std::time::Instant::now() < deadline {
        if let Ok(ScanEvent::Done {
            records,
            kind: ScanKind::Watcher,
            ..
        }) = svc.events().recv_timeout(Duration::from_millis(200))
            && records.len() == 2
        {
            watched = true;
            break;
        }
    }
    assert!(watched, "watcher scan after a new file");
    svc.stop();
}

#[test]
fn scan_service_does_not_rescan_without_changes() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("clips");
    std::fs::create_dir_all(root.join("Desktop")).unwrap();
    std::fs::write(root.join("Desktop/a.mp4"), b"x").unwrap();
    let svc = ScanService::start(root.clone(), vec![]);
    let wait_done = |svc: &ScanService| {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(ScanEvent::Done { .. }) =
                svc.events().recv_timeout(Duration::from_millis(100))
            {
                return true;
            }
        }
        false
    };
    svc.request(ScanKind::Foreground);
    assert!(wait_done(&svc), "first scan");
    // The first scan installed the watches; a second scan opens every watched
    // directory, which must not count as a change.
    svc.request(ScanKind::Foreground);
    assert!(wait_done(&svc), "second scan");
    let mut extra = 0;
    let deadline = std::time::Instant::now() + Duration::from_millis(1500);
    while std::time::Instant::now() < deadline {
        if let Ok(ScanEvent::Done { .. }) = svc.events().recv_timeout(Duration::from_millis(100)) {
            extra += 1;
        }
    }
    assert_eq!(extra, 0, "no scan without a change on disk");
    svc.stop();
}

#[test]
fn forwarder_replaces_the_direct_channel() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("clips");
    std::fs::create_dir_all(&root).unwrap();
    let svc = ScanService::start(root, vec![]);
    let (tx, rx) = std::sync::mpsc::channel();
    svc.forward_to(tx);
    svc.request(ScanKind::Foreground);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut done = false;
    while std::time::Instant::now() < deadline {
        if let Ok(ScanEvent::Done { .. }) = rx.recv_timeout(Duration::from_millis(100)) {
            done = true;
            break;
        }
    }
    assert!(done, "the forwarder receives the scan");
    assert!(
        svc.events().try_recv().is_err(),
        "nothing is queued on the unread direct channel"
    );
    svc.stop();
}
