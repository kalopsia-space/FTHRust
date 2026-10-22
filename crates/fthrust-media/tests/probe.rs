mod support;

use fthrust_media::probe::{
    CfrEvidence, FpsSource, VideoMetadata, evaluate_physical_video_timeline, format_bitrate,
    format_fps, parse_ffprobe_video_metadata, parse_frame_rate, probe_video_cfr_evidence,
    probe_video_metadata,
};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/media-metadata.json")).unwrap()
}

fn as_meta(v: &Value) -> Option<VideoMetadata> {
    if v.is_null() {
        return None;
    }
    let f = |k: &str| v[k].as_f64();
    let u = |k: &str| v[k].as_u64();
    Some(VideoMetadata {
        duration_seconds: f("duration_seconds"),
        width: u("width").map(|x| x as u32),
        height: u("height").map(|x| x as u32),
        average_fps: f("average_fps"),
        real_fps: f("real_fps"),
        video_bitrate_bps: u("video_bitrate_bps"),
        total_bitrate_bps: u("total_bitrate_bps"),
        fps_source: match v["fps_source"].as_str() {
            Some("fthr_frame_rate") => Some(FpsSource::Configured),
            Some("avg_frame_rate") => Some(FpsSource::Average),
            Some("r_frame_rate") => Some(FpsSource::Real),
            None => None,
            Some(other) => panic!("unknown fps_source {other}"),
        },
    })
}

#[test]
fn fixture_cases_match() {
    let fx = fixture();
    for case in fx["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let got = parse_ffprobe_video_metadata(&case["ffprobe"], None);
        let want = as_meta(&case["expected"]);
        assert_eq!(got, want, "case {name}");
        if let Some(m) = &got {
            assert_eq!(
                format_fps(m.average_fps),
                case["expected"]["fps_text"],
                "{name}"
            );
            assert_eq!(
                format_bitrate(m.video_bitrate_bps),
                case["expected"]["video_bitrate_text"],
                "{name}"
            );
        }
    }
    for pair in fx["frame_rate_cases"].as_array().unwrap() {
        assert_eq!(
            parse_frame_rate(&pair[0]),
            pair[1].as_f64(),
            "frame rate {}",
            pair[0]
        );
    }
    for pair in fx["bitrate_text_cases"].as_array().unwrap() {
        assert_eq!(
            format_bitrate(pair[0].as_u64()),
            pair[1],
            "bitrate {}",
            pair[0]
        );
    }
    for pair in fx["fps_text_cases"].as_array().unwrap() {
        assert_eq!(format_fps(pair[0].as_f64()), pair[1], "fps {}", pair[0]);
    }
}

#[test]
fn absurd_bitrate_and_missing_duration_fall_back() {
    // No duration anywhere: the size fallback cannot run, and nothing is
    // invented. A negative or non-numeric bitrate is unknown, not zero.
    let doc = json!({
        "format": {"bit_rate": "-5", "size": "1000000"},
        "streams": [{"codec_type": "video", "width": 1920, "height": 1080,
                     "avg_frame_rate": "60/1", "r_frame_rate": "60/1",
                     "bit_rate": "garbage"}]
    });
    let m = parse_ffprobe_video_metadata(&doc, Some(1_000_000)).unwrap();
    assert_eq!(m.duration_seconds, None);
    assert_eq!(m.total_bitrate_bps, None);
    assert_eq!(m.video_bitrate_bps, None);
    assert_eq!(format_bitrate(m.video_bitrate_bps), "Unavailable");
    // Beyond u64 the value is absurd; it is unknown rather than wrapped.
    let doc = json!({
        "format": {"duration": "10"},
        "streams": [{"codec_type": "video", "bit_rate": "1e30"}]
    });
    let m = parse_ffprobe_video_metadata(&doc, None).unwrap();
    assert_eq!(m.video_bitrate_bps, None);
    // The file-size argument feeds the total-bitrate fallback only.
    let doc = json!({
        "format": {"duration": "10", "bit_rate": "N/A"},
        "streams": [{"codec_type": "video", "bit_rate": "N/A"}]
    });
    let m = parse_ffprobe_video_metadata(&doc, Some(2_500_000)).unwrap();
    assert_eq!(m.total_bitrate_bps, Some(2_000_000));
    assert_eq!(
        m.video_bitrate_bps, None,
        "never derived from the container"
    );
    // Corrupt documents yield nothing.
    assert_eq!(parse_ffprobe_video_metadata(&json!([1, 2]), None), None);
    assert_eq!(
        parse_ffprobe_video_metadata(&json!({"streams": "x"}), None),
        None
    );
}

#[test]
fn physical_timeline_rules() {
    let pts = |v: &[f64]| v.to_vec();
    assert_eq!(evaluate_physical_video_timeline(&[], 30.0), None);
    assert_eq!(evaluate_physical_video_timeline(&pts(&[0.0]), 30.0), None);
    assert_eq!(
        evaluate_physical_video_timeline(&pts(&[0.0, 0.033]), 0.0),
        None
    );
    assert_eq!(
        evaluate_physical_video_timeline(&pts(&[0.0, 0.033]), 2000.0),
        None
    );
    assert_eq!(
        evaluate_physical_video_timeline(&pts(&[0.0, 0.033, 0.066]), 30.0),
        Some(true)
    );
    assert_eq!(
        evaluate_physical_video_timeline(&pts(&[-0.1, 0.0, 0.033]), 30.0),
        Some(true),
        "negative pre-roll is fine"
    );
    assert_eq!(
        evaluate_physical_video_timeline(&pts(&[0.0, 0.033, 0.033]), 30.0),
        Some(false),
        "non-increasing"
    );
    // Bound is max(0.25 s, 4 frames): a 0.2 s hole at 30 fps passes, 0.3 s fails.
    assert_eq!(
        evaluate_physical_video_timeline(&pts(&[0.0, 0.2]), 30.0),
        Some(true)
    );
    assert_eq!(
        evaluate_physical_video_timeline(&pts(&[0.0, 0.3]), 30.0),
        Some(false)
    );
    // At 10 fps four frames is 0.4 s, so 0.3 s passes.
    assert_eq!(
        evaluate_physical_video_timeline(&pts(&[0.0, 0.3]), 10.0),
        Some(true)
    );
    assert_eq!(
        evaluate_physical_video_timeline(&pts(&[0.0, f64::NAN]), 30.0),
        None
    );
}

#[test]
fn probe_real_file_matches_writer() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("clip.mp4");
    if support::write_test_clip(&path, 5, true).is_none() {
        return;
    }
    let m = probe_video_metadata(&path).unwrap();
    assert_eq!((m.width, m.height), (Some(160), Some(90)));
    assert_eq!(m.average_fps, Some(30.0));
    assert_eq!(
        m.fps_source,
        Some(FpsSource::Configured),
        "fthr_frame_rate tag wins"
    );
    assert_eq!(
        m.video_bitrate_bps,
        Some(300_000),
        "fthr_video_bitrate_bps tag wins"
    );
    let d = m.duration_seconds.unwrap();
    assert!((d - 5.0).abs() < 0.1, "duration {d}");
    assert!(m.total_bitrate_bps.unwrap() > 0);

    let ev = probe_video_cfr_evidence(&path, 30.0);
    assert_eq!(
        ev,
        CfrEvidence {
            cfr: Some(true),
            physical_timeline_bounded: Some(true)
        }
    );
    let ev = probe_video_cfr_evidence(&path, 60.0);
    assert_eq!(ev.cfr, Some(false), "sample durations are 1/30, not 1/60");
    assert_eq!(
        probe_video_cfr_evidence(&path, 0.0),
        CfrEvidence {
            cfr: None,
            physical_timeline_bounded: None
        }
    );
    assert_eq!(probe_video_metadata(&tmp.path().join("missing.mp4")), None);
}
