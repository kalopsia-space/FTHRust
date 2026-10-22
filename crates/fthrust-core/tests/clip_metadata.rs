use fthrust_core::clip_metadata::{ClipMetadataStore, DraftFile, EditorDraft};
use fthrust_core::media_identity::file_identity;
use fthrust_core::{CoreError, FthrPaths};
use serde_json::json;
use std::path::{Path, PathBuf};

fn setup() -> (tempfile::TempDir, FthrPaths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(dir.path());
    std::fs::create_dir_all(paths.clip_metadata_db().parent().unwrap()).unwrap();
    (dir, paths)
}

fn draft_for(path: &Path, updated_at: f64, state: serde_json::Value) -> EditorDraft {
    let id = file_identity(path).unwrap();
    EditorDraft {
        version: 1,
        file: DraftFile {
            mtime_ns: id.mtime_ns,
            size: id.size,
        },
        updated_at,
        state,
    }
}

fn raw_value(paths: &FthrPaths, key: &str) -> String {
    let c = rusqlite::Connection::open(paths.clip_metadata_db()).unwrap();
    c.query_row("SELECT value FROM entries WHERE path=?1", [key], |r| {
        r.get(0)
    })
    .unwrap()
}

#[test]
fn defaults_for_unknown_clip() {
    let (_d, paths) = setup();
    let store = ClipMetadataStore::open(&paths).unwrap();
    let e = store.get(Path::new("/nope.mp4")).unwrap();
    assert_eq!(e.tag, "");
    assert_eq!(e.description, "");
    assert!(e.editor_draft.is_none());
}

#[test]
fn partial_update_preserves_other_fields() {
    let (_d, paths) = setup();
    let mut store = ClipMetadataStore::open(&paths).unwrap();
    let p = Path::new("/a.mp4");
    store.update(p, Some("t"), Some("d")).unwrap();
    store.update(p, Some("t2"), None).unwrap();
    let e = store.get(p).unwrap();
    assert_eq!((e.tag.as_str(), e.description.as_str()), ("t2", "d"));
    store.update(p, None, Some("d2")).unwrap();
    let e = store.get(p).unwrap();
    assert_eq!((e.tag.as_str(), e.description.as_str()), ("t2", "d2"));
}

#[test]
fn value_json_is_compact() {
    let (_d, paths) = setup();
    let mut store = ClipMetadataStore::open(&paths).unwrap();
    store
        .update(Path::new("/a.mp4"), Some("t"), Some("d"))
        .unwrap();
    assert_eq!(
        raw_value(&paths, "/a.mp4"),
        r#"{"tag":"t","description":"d"}"#
    );
}

#[test]
fn draft_bound_to_file_identity() {
    let (d, paths) = setup();
    let clip = d.path().join("c.mp4");
    std::fs::write(&clip, b"abc").unwrap();
    let mut store = ClipMetadataStore::open(&paths).unwrap();
    store
        .set_editor_draft(&clip, draft_for(&clip, 1.0, json!({"trim_start": 0.1})))
        .unwrap();
    let id = file_identity(&clip).unwrap();
    let got = store.editor_draft_if_current(&clip, &id).unwrap().unwrap();
    assert_eq!(got.state, json!({"trim_start": 0.1}));
    std::fs::write(&clip, b"abcdef").unwrap();
    let id2 = file_identity(&clip).unwrap();
    assert!(
        store
            .editor_draft_if_current(&clip, &id2)
            .unwrap()
            .is_none()
    );
}

#[test]
fn clear_draft_preserves_details() {
    let (d, paths) = setup();
    let clip = d.path().join("c.mp4");
    std::fs::write(&clip, b"abc").unwrap();
    let mut store = ClipMetadataStore::open(&paths).unwrap();
    store.update(&clip, Some("t"), Some("d")).unwrap();
    store
        .set_editor_draft(&clip, draft_for(&clip, 1.0, json!({})))
        .unwrap();
    store.clear_editor_draft(&clip).unwrap();
    let e = store.get(&clip).unwrap();
    assert_eq!((e.tag.as_str(), e.description.as_str()), ("t", "d"));
    assert!(e.editor_draft.is_none());
}

#[test]
fn draft_storage_bounded_to_250() {
    let (d, paths) = setup();
    let clip = d.path().join("c.mp4");
    std::fs::write(&clip, b"abc").unwrap();
    let mut store = ClipMetadataStore::open(&paths).unwrap();
    for i in 0..255u32 {
        let p = PathBuf::from(format!("/clip{i}.mp4"));
        store
            .set_editor_draft(&p, draft_for(&clip, f64::from(i), json!({})))
            .unwrap();
    }
    let with_draft = (0..255u32)
        .filter(|i| {
            store
                .get(Path::new(&format!("/clip{i}.mp4")))
                .unwrap()
                .editor_draft
                .is_some()
        })
        .count();
    assert_eq!(with_draft, 250);
    assert!(
        store
            .get(Path::new("/clip0.mp4"))
            .unwrap()
            .editor_draft
            .is_none()
    );
    assert!(
        store
            .get(Path::new("/clip254.mp4"))
            .unwrap()
            .editor_draft
            .is_some()
    );
}

#[test]
fn legacy_json_imported_once_and_left_in_place() {
    let (_d, paths) = setup();
    let legacy = paths.legacy_clip_metadata_json();
    std::fs::write(&legacy, r#"{"/a.mp4": {"tag": "old", "description": "x"}}"#).unwrap();
    {
        let mut store = ClipMetadataStore::open(&paths).unwrap();
        assert_eq!(store.get(Path::new("/a.mp4")).unwrap().tag, "old");
        store
            .update(Path::new("/a.mp4"), Some("new"), None)
            .unwrap();
    }
    assert!(legacy.exists());
    std::fs::write(&legacy, r#"{"/b.mp4": {"tag": "other"}}"#).unwrap();
    let store = ClipMetadataStore::open(&paths).unwrap();
    assert_eq!(store.get(Path::new("/a.mp4")).unwrap().tag, "new");
    assert_eq!(store.get(Path::new("/b.mp4")).unwrap().tag, "");
}

#[test]
fn rename_moves_entry() {
    let (_d, paths) = setup();
    let mut store = ClipMetadataStore::open(&paths).unwrap();
    store.update(Path::new("/a.mp4"), Some("t"), None).unwrap();
    store
        .rename_path(Path::new("/a.mp4"), Path::new("/b.mp4"))
        .unwrap();
    assert_eq!(store.get(Path::new("/b.mp4")).unwrap().tag, "t");
    assert_eq!(store.get(Path::new("/a.mp4")).unwrap().tag, "");
    store
        .rename_path(Path::new("/zzz"), Path::new("/yyy"))
        .unwrap();
    assert_eq!(store.get(Path::new("/yyy")).unwrap().tag, "");
}

#[test]
fn locked_database_errors_not_fallback() {
    let (_d, paths) = setup();
    std::fs::write(
        paths.legacy_clip_metadata_json(),
        r#"{"/a.mp4": {"tag": "legacy"}}"#,
    )
    .unwrap();
    let mut store = ClipMetadataStore::open(&paths).unwrap();
    let blocker = rusqlite::Connection::open(paths.clip_metadata_db()).unwrap();
    blocker.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let err = store
        .update(Path::new("/a.mp4"), Some("x"), None)
        .unwrap_err();
    assert!(matches!(err, CoreError::Sqlite(_)), "{err:?}");
    blocker.execute_batch("ROLLBACK").unwrap();
    assert_eq!(store.get(Path::new("/a.mp4")).unwrap().tag, "legacy");
}

#[test]
fn update_preserves_unparseable_draft_and_unknown_keys() {
    let (_d, paths) = setup();
    let mut store = ClipMetadataStore::open(&paths).unwrap();
    let raw = r#"{"tag":"a","description":"b","extra":1,"editor_draft":{"version":9}}"#;
    {
        let c = rusqlite::Connection::open(paths.clip_metadata_db()).unwrap();
        c.execute("INSERT INTO entries VALUES ('/a.mp4', ?1)", [raw])
            .unwrap();
    }
    store.update(Path::new("/a.mp4"), Some("c"), None).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw_value(&paths, "/a.mp4")).unwrap();
    assert_eq!(
        v,
        json!({"tag":"c","description":"b","extra":1,"editor_draft":{"version":9}})
    );
}

#[test]
fn editor_draft_if_current_ignores_other_versions() {
    let (d, paths) = setup();
    let clip = d.path().join("c.mp4");
    std::fs::write(&clip, b"abc").unwrap();
    let mut store = ClipMetadataStore::open(&paths).unwrap();
    let id = file_identity(&clip).unwrap();
    let mut draft = draft_for(&clip, 1.0, json!({}));
    draft.version = 2;
    store.set_editor_draft(&clip, draft).unwrap();
    assert!(store.editor_draft_if_current(&clip, &id).unwrap().is_none());
    store
        .set_editor_draft(&clip, draft_for(&clip, 1.0, json!([1])))
        .unwrap();
    assert!(store.editor_draft_if_current(&clip, &id).unwrap().is_none());
    store
        .set_editor_draft(&clip, draft_for(&clip, 1.0, json!({})))
        .unwrap();
    assert!(store.editor_draft_if_current(&clip, &id).unwrap().is_some());
}

#[test]
fn edited_duration_applies_trim_and_deleted_ranges() {
    let (d, paths) = setup();
    let clip = d.path().join("c.mp4");
    std::fs::write(&clip, b"abc").unwrap();
    let mut store = ClipMetadataStore::open(&paths).unwrap();
    assert_eq!(store.edited_duration(&clip, 100.0).unwrap(), 100.0);
    let state = json!({
        "trim_start": 0.1, "trim_end": 0.9,
        "segments": [[0.0, 0.4, false], [0.4, 0.6, true], [0.6, 1.0, false]]
    });
    store
        .set_editor_draft(&clip, draft_for(&clip, 1.0, state))
        .unwrap();
    // kept: 0.1..0.4 (0.3) + 0.6..0.9 (0.3) = 0.6
    let got = store.edited_duration(&clip, 100.0).unwrap();
    assert!((got - 60.0).abs() < 1e-9, "{got}");
    let bad = json!({"trim_start": 0.1, "trim_end": 0.9, "segments": [[0.0, 0.5, false]]});
    store
        .set_editor_draft(&clip, draft_for(&clip, 2.0, bad))
        .unwrap();
    assert_eq!(store.edited_duration(&clip, 100.0).unwrap(), 100.0);
}
