use fthrust_media::capabilities::{
    CACHE_VERSION, CacheSignature, EncoderCapability, available_codecs, backend_keys,
    backend_table, load_cached, probe_capabilities, probe_capabilities_with, store_cached,
};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/encoder-capabilities.json")).unwrap()
}

#[test]
fn capability_table_matches_fixture() {
    let fx = fixture();
    assert_eq!(fx["cache_version"].as_u64().unwrap() as u32, CACHE_VERSION);
    assert_eq!(
        fx["vaapi_device_default"],
        fthrust_media::ffi::hw::DEFAULT_DEVICE
    );
    let keys: Vec<&str> = fx["backend_keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(backend_keys().to_vec(), keys);
    let table = backend_table();
    let backends = fx["backends"].as_array().unwrap();
    assert_eq!(table.len(), backends.len());
    for (spec, want) in table.iter().zip(backends) {
        assert_eq!(spec.key, want["key"].as_str().unwrap());
        assert_eq!(spec.label, want["label"].as_str().unwrap());
        assert_eq!(
            spec.supports_presets,
            want["supports_presets"].as_bool().unwrap()
        );
        let want_names = want["codec_names"].as_array().unwrap();
        assert_eq!(spec.codecs.len(), want_names.len());
        for ((codec, names), w) in spec.codecs.iter().zip(want_names) {
            assert_eq!(*codec, w[0].as_str().unwrap());
            let wn: Vec<&str> = w[1]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            if spec.key == "software" {
                // The reference's LGPL names lead; x264/x265 are appended for
                // distro FFmpeg builds (same ruling as the encoder policy).
                assert!(
                    names.starts_with(&wn),
                    "{codec}: {names:?} should start with {wn:?}"
                );
            } else {
                assert_eq!(*names, wn, "{}/{codec}", spec.key);
            }
        }
    }
}

#[test]
fn injected_probe_filters_and_orders() {
    let caps =
        probe_capabilities_with(|name| matches!(name, "h264_vaapi" | "libsvtav1" | "hevc_amf"));
    assert_eq!(caps.len(), 2);
    assert_eq!(caps[0].key, "amf");
    assert_eq!(caps[0].label, "VA-API (AMD / Intel)");
    assert_eq!(caps[0].codecs, ["h264", "hevc"]);
    assert_eq!(
        caps[0].codec_names,
        [
            ("h264".to_string(), "h264_vaapi".to_string()),
            ("hevc".to_string(), "hevc_amf".to_string())
        ]
    );
    assert!(!caps[0].supports_presets);
    assert_eq!(caps[0].encoder_name_for("h264"), "h264_vaapi");
    assert_eq!(caps[0].encoder_name_for("av1"), "");
    assert_eq!(caps[1].key, "software");
    assert_eq!(caps[1].codecs, ["av1"]);
    assert_eq!(available_codecs(&caps, "auto"), ["h264", "hevc", "av1"]);
    assert_eq!(available_codecs(&caps, "amf"), ["h264", "hevc"]);
    assert_eq!(available_codecs(&caps, "nvenc"), Vec::<String>::new());
    assert!(probe_capabilities_with(|_| false).is_empty());
}

#[test]
fn cache_round_trip_and_signature_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("encoder_capabilities.json");
    let sig = CacheSignature {
        platform: "linux".into(),
        ffmpeg: "/usr/lib/libavcodec.so.62".into(),
        ffmpeg_size: 123,
        ffmpeg_mtime_ns: 456,
    };
    let caps = vec![EncoderCapability {
        key: "software".into(),
        label: "Software".into(),
        codecs: vec!["h264".into()],
        codec_names: vec![("h264".into(), "libx264".into())],
        supports_presets: false,
    }];
    assert_eq!(load_cached(&file, &sig), None, "missing cache is a miss");
    store_cached(&file, &sig, &caps).unwrap();
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(raw["version"], CACHE_VERSION);
    assert_eq!(raw["platform"], "linux");
    assert_eq!(raw["ffmpeg"], "/usr/lib/libavcodec.so.62");
    assert_eq!(raw["ffmpeg_size"], 123);
    assert_eq!(raw["ffmpeg_mtime_ns"], 456);
    assert_eq!(
        raw["capabilities"][0]["codec_names"][0],
        serde_json::json!(["h264", "libx264"])
    );
    assert!(!tmp.path().join("encoder_capabilities.json.tmp").exists());
    assert_eq!(load_cached(&file, &sig), Some(caps.clone()));
    let other = CacheSignature {
        ffmpeg_size: 124,
        ..sig.clone()
    };
    assert_eq!(
        load_cached(&file, &other),
        None,
        "a different build is a miss"
    );
    std::fs::write(&file, "{not json").unwrap();
    assert_eq!(load_cached(&file, &sig), None, "corrupt cache is a miss");
    std::fs::write(&file, "[1,2]").unwrap();
    assert_eq!(load_cached(&file, &sig), None);
}

#[test]
fn real_probe_reports_a_working_software_h264() {
    fthrust_media::init().unwrap();
    let have_sw = ["libopenh264", "libx264"]
        .iter()
        .any(|n| ffmpeg_next::encoder::find_by_name(n).is_some());
    let caps = probe_capabilities();
    let software = caps.iter().find(|c| c.key == "software");
    if have_sw {
        let software = software.expect("software backend present");
        assert!(software.codecs.iter().any(|c| c == "h264"), "{software:?}");
    }
    for cap in &caps {
        assert!(backend_keys().contains(&cap.key.as_str()));
        assert_eq!(cap.codecs.len(), cap.codec_names.len());
    }
    let sig = CacheSignature::current().unwrap();
    assert_eq!(sig.platform, "linux");
    assert!(sig.ffmpeg_size > 0);
}
