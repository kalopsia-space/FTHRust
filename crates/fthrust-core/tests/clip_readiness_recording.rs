//! A manual recording is written straight into its final name, so nothing
//! about the file says it is not a clip yet: the registry has to.

use fthrust_core::clip_readiness::{ClipReadinessRegistry, ReadinessState};

#[test]
fn a_recording_is_not_a_clip_until_it_is_over() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("FTHR_recording_2027-01-15_10-00-00.mp4");
    let mut r = ClipReadinessRegistry::new();
    // Marked when the name is chosen, before the recorder creates the file.
    r.recording(&path);
    std::fs::write(&path, b"first fragment").unwrap();
    assert_eq!(r.state(&path), ReadinessState::Recording);
    assert!(
        !r.can_access(&path),
        "an existing file is not ready while it is written"
    );
    assert!(
        r.has_blocked(),
        "the page asks per card while one is blocked"
    );
    // Stopped: the app checks the finished file before it is a clip.
    assert_eq!(r.engine_committed(&path, true), ReadinessState::Finalizing);
    assert!(!r.can_access(&path));
    // Over, whether the check passed or the recording failed and its
    // fragments were kept: the file is an ordinary clip.
    r.forget(&path);
    assert!(r.can_access(&path));
    assert_eq!(r.state(&path), ReadinessState::Ready);
    assert!(!r.has_blocked());
}

#[test]
fn a_file_that_is_not_there_yet_keeps_its_name_when_it_appears() {
    // The key of a missing file used to be its path as given, and that of an
    // existing one its real path: a folder behind a link made them differ, so
    // a recording marked before it was created was ready as soon as it was.
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = d.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let path = link.join("FTHR_recording_x.mp4");
    let mut r = ClipReadinessRegistry::new();
    r.recording(&path);
    std::fs::write(real.join("FTHR_recording_x.mp4"), b"fragment").unwrap();
    assert_eq!(
        r.state(&path),
        ReadinessState::Recording,
        "by the given name"
    );
    assert_eq!(
        r.state(&real.join("FTHR_recording_x.mp4")),
        ReadinessState::Recording,
        "by its real name"
    );
    r.forget(&path);
    assert!(r.can_access(&real.join("FTHR_recording_x.mp4")));
}

#[test]
fn forgetting_a_deleted_file_through_a_link_drops_its_entry() {
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = d.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let path = link.join("gone.mp4");
    std::fs::write(&path, b"x").unwrap();
    let mut r = ClipReadinessRegistry::new();
    r.engine_committed(&path, true);
    assert!(r.has_blocked());
    std::fs::remove_file(&path).unwrap();
    r.forget(&path);
    assert!(
        !r.has_blocked(),
        "the entry went with the file, though the file is gone"
    );
}
