use fthrust_core::audio_manifest::*;
use std::fs;
use std::path::Path;

const TX: &str = "11111111-2222-4333-8444-555555555555";
const SRC: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

fn source(uuid: &str, idx: u32) -> AudioSource {
    AudioSource {
        source_uuid: uuid.into(),
        stream_index: idx,
        source_type: SourceType::Application,
        display_name: "Game (1)".into(),
        persistent_identity: "game.exe".into(),
        icon_reference: None,
        first_active_100ns: 0,
        last_active_100ns: 10,
        sample_rate: 48000,
        channels: 2,
        sample_format: "fltp".into(),
    }
}

fn manifest_for(media: &Path) -> AudioManifest {
    AudioManifest {
        manifest_version: 1,
        transaction_id: TX.into(),
        media_file: media.file_name().unwrap().to_str().unwrap().into(),
        media_sha256: sha256_file(media).unwrap(),
        sources: vec![source(SRC, 1)],
    }
}

fn media_in(dir: &Path, name: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    fs::write(&p, b"mp4 bytes").unwrap();
    p
}

#[test]
fn round_trip_is_compact_sorted_ascii() {
    let d = tempfile::tempdir().unwrap();
    let media = media_in(d.path(), "clip.mp4");
    let mut m = manifest_for(&media);
    m.sources[0].display_name = "Game (1)".into();
    m.write(&media).unwrap();
    let text = fs::read_to_string(manifest_path_for(&media)).unwrap();
    assert!(text.is_ascii());
    assert!(!text.contains(": ") && !text.contains(", ") && !text.contains('\n'));
    let expected = format!(
        "{{\"manifest_version\":1,\"media_file\":\"clip.mp4\",\"media_sha256\":\"{}\",\"sources\":[{{\"channels\":2,\"display_name\":\"Game (1)\",\"first_active_100ns\":0,\"icon_reference\":null,\"last_active_100ns\":10,\"persistent_identity\":\"game.exe\",\"sample_format\":\"fltp\",\"sample_rate\":48000,\"source_type\":\"application\",\"source_uuid\":\"{SRC}\",\"stream_index\":1}}],\"transaction_id\":\"{TX}\"}}",
        m.media_sha256
    );
    assert_eq!(text, expected);
    assert_eq!(AudioManifest::read_verified(&media).unwrap(), Some(m));
}

#[test]
fn mismatched_hash_reads_as_none() {
    let d = tempfile::tempdir().unwrap();
    let media = media_in(d.path(), "clip.mp4");
    manifest_for(&media).write(&media).unwrap();
    fs::write(&media, b"changed").unwrap();
    assert_eq!(AudioManifest::read_verified(&media).unwrap(), None);
    let other = d.path().join("none.mp4");
    fs::write(&other, b"x").unwrap();
    assert_eq!(AudioManifest::read_verified(&other).unwrap(), None);
}

#[test]
fn path_like_identity_rejected() {
    let d = tempfile::tempdir().unwrap();
    let media = media_in(d.path(), "clip.mp4");
    for bad in ["C:\\Users\\bob\\game.exe", "/usr/bin/game", "a\"b", ""] {
        let mut m = manifest_for(&media);
        m.sources[0].persistent_identity = bad.into();
        assert!(m.validate().is_err(), "{bad}");
    }
    let mut m = manifest_for(&media);
    m.media_file = "sub/clip.mp4".into();
    assert!(m.validate().is_err());
    m = manifest_for(&media);
    m.sources[0].sample_rate = 7999;
    assert!(m.validate().is_err());
    m = manifest_for(&media);
    m.sources[0].channels = 9;
    assert!(m.validate().is_err());
    m = manifest_for(&media);
    m.sources.clear();
    assert!(m.validate().is_err());
}

#[test]
fn duplicate_stream_index_rejected() {
    let d = tempfile::tempdir().unwrap();
    let media = media_in(d.path(), "clip.mp4");
    let mut m = manifest_for(&media);
    m.sources
        .push(source("bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee", 1));
    assert!(m.validate().is_err());
    m.sources[1].stream_index = 2;
    assert!(m.validate().is_ok());
    m.sources[0].stream_index = 0;
    assert!(m.validate().is_err());
}

#[test]
fn rebind_updates_hash() {
    let d = tempfile::tempdir().unwrap();
    let media = media_in(d.path(), "clip.mp4");
    manifest_for(&media).write(&media).unwrap();
    fs::write(&media, b"remuxed").unwrap();
    assert_eq!(AudioManifest::read_verified(&media).unwrap(), None);
    rebind_after_media_replace(&media).unwrap();
    let m = AudioManifest::read_verified(&media).unwrap().unwrap();
    assert_eq!(m.media_sha256, sha256_file(&media).unwrap());
}

#[test]
fn source_type_serialises_lowercase() {
    assert_eq!(
        serde_json::to_string(&SourceType::Application).unwrap(),
        "\"application\""
    );
    assert_eq!(
        serde_json::to_string(&SourceType::Microphone).unwrap(),
        "\"microphone\""
    );
    assert_eq!(
        serde_json::to_string(&SourceType::System).unwrap(),
        "\"system\""
    );
}

#[test]
fn rename_moves_media_and_manifest_and_rewrites_media_file() {
    let d = tempfile::tempdir().unwrap();
    let media = media_in(d.path(), "old.mp4");
    manifest_for(&media).write(&media).unwrap();
    let new = rename_clip(&media, "new name").unwrap();
    assert_eq!(new, d.path().join("new name.mp4"));
    assert!(!media.exists() && !manifest_path_for(&media).exists());
    let m = AudioManifest::read_verified(&new).unwrap().unwrap();
    assert_eq!(m.media_file, "new name.mp4");
    // Clip without a sidecar renames cleanly.
    let bare = media_in(d.path(), "bare.mp4");
    assert_eq!(
        rename_clip(&bare, "bare2").unwrap(),
        d.path().join("bare2.mp4")
    );
    // Same-name rename keeps the sidecar.
    let same = rename_clip(&new, "new name").unwrap();
    assert!(AudioManifest::read_verified(&same).unwrap().is_some());
}

#[test]
fn rename_rolls_back_when_sidecar_fails() {
    let d = tempfile::tempdir().unwrap();
    let media = media_in(d.path(), "old.mp4");
    // A sidecar that cannot be rewritten (not valid JSON).
    fs::write(manifest_path_for(&media), b"not json").unwrap();
    assert!(rename_clip(&media, "new").is_err());
    assert!(media.exists());
    assert!(!d.path().join("new.mp4").exists());
    assert_eq!(fs::read(manifest_path_for(&media)).unwrap(), b"not json");
}

#[test]
fn rename_refuses_collision_and_reserved_names() {
    let d = tempfile::tempdir().unwrap();
    let media = media_in(d.path(), "a.mp4");
    let other = media_in(d.path(), "b.mp4");
    assert!(rename_clip(&media, "b").is_err());
    assert_eq!(fs::read(&other).unwrap(), b"mp4 bytes");
    for bad in [
        "CON", "nul.txt", "a/b", "a:b", "", "..", "x.", "q?", "ta\tb",
    ] {
        assert!(rename_clip(&media, bad).is_err(), "{bad}");
    }
    assert!(rename_clip(&media, &"x".repeat(252)).is_err());
    assert!(media.exists());
}

#[test]
fn rename_rolls_back_when_sidecar_write_fails() {
    let d = tempfile::tempdir().unwrap();
    let media = media_in(d.path(), "clip.mp4");
    manifest_for(&media).write(&media).unwrap();
    let side = manifest_path_for(&media);
    let before = fs::read(&side).unwrap();
    let stem = "a".repeat(240);
    assert!(rename_clip(&media, &stem).is_err());
    assert!(media.exists());
    assert_eq!(fs::read(&side).unwrap(), before);
    assert!(!d.path().join(format!("{stem}.mp4")).exists());
    let names: Vec<String> = fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}
