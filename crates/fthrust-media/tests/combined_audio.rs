mod support;

use fthrust_media::combined_audio::{
    AudioStreamDescriptor, build_combined_audio_filter, descriptors_from_ffprobe_streams,
    descriptors_from_file, select_combined_audio_streams,
};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/combined-audio-policy.json")).unwrap()
}

fn descriptor(v: &Value) -> AudioStreamDescriptor {
    AudioStreamDescriptor {
        container_index: v["container_index"].as_u64().unwrap() as usize,
        audio_index: v["audio_index"].as_u64().unwrap() as usize,
        title: v["title"].as_str().map(str::to_string),
        handler_name: v["handler_name"].as_str().map(str::to_string),
    }
}

#[test]
fn combined_policy_cases_match_fixture() {
    let fx = fixture();
    for case in fx["selections"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let streams: Vec<AudioStreamDescriptor> = case["streams"]
            .as_array()
            .unwrap()
            .iter()
            .map(descriptor)
            .collect();
        let manifest = (!case["manifest"].is_null()).then_some(&case["manifest"]);
        let got = select_combined_audio_streams(manifest, &streams);
        let want: Vec<usize> = case["expected"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as usize)
            .collect();
        assert_eq!(got, want, "{name}");
    }
    for (key, want) in fx["filters"].as_object().unwrap() {
        let indexes: Vec<usize> = serde_json::from_str(key).unwrap();
        assert_eq!(
            build_combined_audio_filter(&indexes).unwrap(),
            want.as_str().unwrap(),
            "{key}"
        );
    }
    let streams = fx["probe_streams"].as_array().unwrap();
    let got = descriptors_from_ffprobe_streams(streams);
    let want: Vec<AudioStreamDescriptor> = fx["descriptors"]
        .as_array()
        .unwrap()
        .iter()
        .map(descriptor)
        .collect();
    assert_eq!(got, want);
}

#[test]
fn filter_rejects_empty_and_manifest_edge_cases() {
    assert!(build_combined_audio_filter(&[]).is_err());
    let streams = vec![
        AudioStreamDescriptor {
            container_index: 1,
            audio_index: 0,
            title: Some("System Audio".into()),
            handler_name: None,
        },
        AudioStreamDescriptor {
            container_index: 2,
            audio_index: 1,
            title: None,
            handler_name: Some("MICROPHONE".into()),
        },
    ];
    // A manifest without usable sources selects nothing (Default Mix covers it).
    let manifest =
        serde_json::json!({"sources": [{"source_type": "system", "stream_index": "1"}, 7, null]});
    assert_eq!(
        select_combined_audio_streams(Some(&manifest), &streams),
        Vec::<usize>::new()
    );
    let manifest = serde_json::json!({});
    assert_eq!(
        select_combined_audio_streams(Some(&manifest), &streams),
        Vec::<usize>::new()
    );
    // Without a manifest, handler_name labels count like titles.
    assert_eq!(select_combined_audio_streams(None, &streams), [0, 1]);
}

#[test]
fn descriptors_from_written_clip() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("clip.mp4");
    if support::write_test_clip(&path, 2, true).is_none() {
        return;
    }
    let got = descriptors_from_file(&path).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].container_index, got[0].audio_index), (1, 0));
    let silent = tmp.path().join("silent.mp4");
    support::write_test_clip(&silent, 2, false).unwrap();
    assert!(descriptors_from_file(&silent).unwrap().is_empty());
    assert!(descriptors_from_file(&tmp.path().join("missing.mp4")).is_err());
}
