use std::os::unix::fs::PermissionsExt;

use fthrust_core::FthrPaths;
use fthrust_portal::screencast::{persist_token, should_retry_startup};
use fthrust_portal::token::{TokenError, load, path, save};

#[test]
fn the_token_start_returns_replaces_the_presented_one() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("tok");
    save(&p, "old").unwrap();
    persist_token(&p, Some("old"), Some("new"));
    assert_eq!(load(&p).unwrap().as_deref(), Some("new"));
    persist_token(&p, Some("new"), Some("new"));
    assert_eq!(load(&p).unwrap().as_deref(), Some("new"));
    persist_token(&p, None, Some("fresh"));
    assert_eq!(load(&p).unwrap().as_deref(), Some("fresh"));
    // A portal that returns none does not persist sessions: the used one
    // must not linger.
    persist_token(&p, Some("fresh"), None);
    assert_eq!(load(&p).unwrap(), None);
}

#[test]
fn token_file_roundtrip_permissions_and_deletion() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(tmp.path());
    let p = path(&paths);
    assert_eq!(p, tmp.path().join(".fthr").join("portal_screencast_token"));
    assert_eq!(load(&p).unwrap(), None, "missing is None");
    save(&p, "abc-123").unwrap();
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(load(&p).unwrap(), Some("abc-123".to_string()));
    assert!(
        !tmp.path()
            .join(".fthr")
            .join("portal_screencast_token.tmp")
            .exists()
    );
    save(&p, "").unwrap();
    assert!(!p.exists(), "an empty token deletes the file");
    assert_eq!(load(&p).unwrap(), None);
}

#[test]
fn damaged_tokens_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("tok");
    std::fs::write(&p, "abc\n").unwrap();
    assert_eq!(load(&p), Err(TokenError::Damaged));
    std::fs::write(&p, " ").unwrap();
    assert_eq!(load(&p), Err(TokenError::Damaged));
    std::fs::write(&p, "ab\x07c").unwrap();
    assert_eq!(load(&p), Err(TokenError::Damaged));
    std::fs::write(&p, "").unwrap();
    assert_eq!(
        load(&p),
        Err(TokenError::Damaged),
        "empty file is damaged, not absent"
    );
}

#[test]
fn retry_once_without_token() {
    assert!(should_retry_startup(0, true));
    assert!(!should_retry_startup(0, false));
    assert!(!should_retry_startup(1, true));
}
