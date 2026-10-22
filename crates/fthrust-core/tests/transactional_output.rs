use fthrust_core::clip_files::is_partial_name;
use fthrust_core::media_identity::file_identity;
use fthrust_core::transactional_output::StagedOutput;
use std::fs;

#[test]
fn staged_output_commit_and_discard() {
    let d = tempfile::tempdir().unwrap();
    let fin = d.path().join("a.mp4");
    fs::write(&fin, b"old").unwrap();
    let s = StagedOutput::new(&fin);
    assert!(!s.staged_path().exists());
    fs::write(s.staged_path(), b"new").unwrap();
    assert_eq!(s.commit().unwrap(), fin);
    assert_eq!(fs::read(&fin).unwrap(), b"new");

    let s = StagedOutput::new(&fin);
    fs::write(s.staged_path(), b"x").unwrap();
    let st = s.staged_path().to_path_buf();
    s.discard();
    assert!(!st.exists());
    StagedOutput::new(&fin).discard();
}

#[test]
fn staged_output_is_hidden_and_partial() {
    let s = StagedOutput::new(std::path::Path::new("/t/a.mp4"));
    let n = s.staged_path().file_name().unwrap().to_str().unwrap();
    assert!(
        n.starts_with('.') && is_partial_name(n) && n.ends_with(".mp4"),
        "{n}"
    );
    assert_eq!(
        s.staged_path().parent().unwrap(),
        std::path::Path::new("/t")
    );
}

#[test]
fn file_identity_changes_on_write() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f");
    fs::write(&p, b"a").unwrap();
    let a = file_identity(&p).unwrap();
    assert_eq!(a, file_identity(&p).unwrap());
    fs::write(&p, b"abc").unwrap();
    assert_ne!(a, file_identity(&p).unwrap());
    assert!(file_identity(&d.path().join("missing")).is_err());
}
