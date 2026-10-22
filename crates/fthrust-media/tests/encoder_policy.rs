use fthrust_core::capture_config::{Codec, EncoderPref};
use fthrust_media::encoder_policy::{encoder_candidates, preset_options};

fn names(codec: Codec, encoder: EncoderPref) -> Vec<&'static str> {
    encoder_candidates(codec, encoder)
}

fn is_software(name: &str) -> bool {
    name.starts_with("lib")
}

#[test]
fn auto_is_hardware_only_in_backend_order() {
    assert_eq!(
        names(Codec::Auto, EncoderPref::Auto),
        [
            "h264_nvenc",
            "hevc_nvenc",
            "av1_nvenc",
            "h264_vaapi",
            "hevc_vaapi",
            "av1_vaapi",
            "h264_amf",
            "hevc_amf",
            "av1_amf",
            "h264_qsv",
            "hevc_qsv",
            "av1_qsv",
        ]
    );
    assert_eq!(
        names(Codec::H264, EncoderPref::Auto),
        ["h264_nvenc", "h264_vaapi", "h264_amf", "h264_qsv"]
    );
    for codec in [Codec::Auto, Codec::H264, Codec::Hevc, Codec::Av1] {
        let list = names(codec, EncoderPref::Auto);
        assert!(!list.is_empty());
        assert!(list.iter().all(|n| !is_software(n)), "{list:?}");
    }
}

#[test]
fn codec_filtering() {
    assert_eq!(names(Codec::H264, EncoderPref::Nvenc), ["h264_nvenc"]);
    assert_eq!(
        names(Codec::Hevc, EncoderPref::Vaapi),
        ["hevc_vaapi", "hevc_amf"]
    );
    assert_eq!(names(Codec::Av1, EncoderPref::Qsv), ["av1_qsv"]);
    for backend in [EncoderPref::Nvenc, EncoderPref::Vaapi, EncoderPref::Qsv] {
        assert!(names(Codec::Auto, backend).iter().all(|n| !is_software(n)));
    }
}

#[test]
fn software_pref_lists_only_software() {
    // The reference's LGPL names come first; x264/x265 are appended for
    // distro builds and are skipped when the FFmpeg build lacks them.
    assert_eq!(
        names(Codec::H264, EncoderPref::Software),
        ["libopenh264", "libx264"]
    );
    assert_eq!(
        names(Codec::Hevc, EncoderPref::Software),
        ["libkvazaar", "libx265"]
    );
    assert_eq!(
        names(Codec::Av1, EncoderPref::Software),
        ["libsvtav1", "libaom-av1", "librav1e"]
    );
    assert!(
        names(Codec::Auto, EncoderPref::Software)
            .iter()
            .all(|n| is_software(n))
    );
}

#[test]
fn unknown_prefs_never_add_software() {
    for backend in [
        EncoderPref::Auto,
        EncoderPref::Nvenc,
        EncoderPref::Vaapi,
        EncoderPref::Qsv,
    ] {
        assert!(names(Codec::Auto, backend).iter().all(|n| !is_software(n)));
    }
}

#[test]
fn preset_options_match_reference_tables() {
    let opts = |name: &str, p: u8| preset_options(name, p).unwrap();
    assert_eq!(
        opts("h264_nvenc", 3),
        [
            ("preset", "p3"),
            ("tune", "ll"),
            ("rc", "vbr"),
            ("cbr", "0"),
            ("forced-idr", "1")
        ]
        .map(|(k, v)| (k, v.to_string()))
    );
    assert_eq!(opts("h264_amf", 1)[0], ("quality", "speed".to_string()));
    assert_eq!(
        opts("hevc_amf", 7),
        [
            ("quality", "quality".to_string()),
            ("rc", "vbr_latency".to_string())
        ]
    );
    assert!(
        opts("h264_vaapi", 1).is_empty(),
        "VA-API quality level is deliberately left unset"
    );
    assert!(opts("av1_vaapi", 7).is_empty());
    assert_eq!(
        opts("h264_qsv", 1),
        [
            ("preset", "veryfast".to_string()),
            ("look_ahead", "0".to_string())
        ]
    );
    assert_eq!(opts("h264_qsv", 7)[0], ("preset", "slowest".to_string()));
    assert_eq!(
        opts("libopenh264", 4),
        [
            ("rc_mode", "bitrate".to_string()),
            ("allow_skip_frames", "0".to_string()),
            ("profile", "high".to_string())
        ]
    );
    assert_eq!(opts("libkvazaar", 5), [("preset", "medium".to_string())]);
    assert_eq!(
        opts("libx264", 1),
        [
            ("preset", "ultrafast".to_string()),
            ("tune", "zerolatency".to_string())
        ]
    );
    assert_eq!(opts("libsvtav1", 2), [("preset", "9".to_string())]);
    assert!(opts("libaom-av1", 3).is_empty());
    assert_eq!(
        opts("h264_nvenc", 0)[0],
        ("preset", "p1".to_string()),
        "clamped below 1"
    );
    assert_eq!(
        opts("h264_nvenc", 9)[0],
        ("preset", "p7".to_string()),
        "clamped above 7"
    );
    assert!(preset_options("mpeg4", 3).is_none());
}
