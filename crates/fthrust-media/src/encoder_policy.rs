//! Which FFmpeg encoders the engine may try, in order, and the per-encoder
//! preset options. Pure and FFmpeg-free so the policy is pinned by tests
//! without a GPU. Ports `encoder_policy.cpp` and `ApplyPreset` from
//! `encoder.cpp` in the reference.

use fthrust_core::capture_config::{Codec, EncoderPref};

/// Candidate encoder names for a codec and backend preference.
///
/// `Auto` tries the hardware backends only (NVENC, then VA-API/AMF, then
/// QSV) and never falls back to software on its own, so a missing GPU
/// encoder surfaces as an error instead of a silent CPU replay. Software
/// encoders are tried only for `Software`: the reference's LGPL names first,
/// then x264/x265 for distro FFmpeg builds (absent encoders are skipped).
pub fn encoder_candidates(codec: Codec, encoder: EncoderPref) -> Vec<&'static str> {
    let allowed = |c: Codec| codec == Codec::Auto || codec == c;
    let mut out = Vec::new();
    let add = |backend: EncoderPref, out: &mut Vec<&'static str>| match backend {
        EncoderPref::Nvenc => {
            if allowed(Codec::H264) {
                out.push("h264_nvenc");
            }
            if allowed(Codec::Hevc) {
                out.push("hevc_nvenc");
            }
            if allowed(Codec::Av1) {
                out.push("av1_nvenc");
            }
        }
        EncoderPref::Vaapi => {
            if allowed(Codec::H264) {
                out.push("h264_vaapi");
            }
            if allowed(Codec::Hevc) {
                out.push("hevc_vaapi");
            }
            if allowed(Codec::Av1) {
                out.push("av1_vaapi");
            }
            if allowed(Codec::H264) {
                out.push("h264_amf");
            }
            if allowed(Codec::Hevc) {
                out.push("hevc_amf");
            }
            if allowed(Codec::Av1) {
                out.push("av1_amf");
            }
        }
        EncoderPref::Qsv => {
            if allowed(Codec::H264) {
                out.push("h264_qsv");
            }
            if allowed(Codec::Hevc) {
                out.push("hevc_qsv");
            }
            if allowed(Codec::Av1) {
                out.push("av1_qsv");
            }
        }
        EncoderPref::Software => {
            if allowed(Codec::H264) {
                out.push("libopenh264");
                out.push("libx264");
            }
            if allowed(Codec::Hevc) {
                out.push("libkvazaar");
                out.push("libx265");
            }
            if allowed(Codec::Av1) {
                out.push("libsvtav1");
                out.push("libaom-av1");
                out.push("librav1e");
            }
        }
        EncoderPref::Auto => {}
    };
    match encoder {
        EncoderPref::Auto => {
            add(EncoderPref::Nvenc, &mut out);
            add(EncoderPref::Vaapi, &mut out);
            add(EncoderPref::Qsv, &mut out);
        }
        other => add(other, &mut out),
    }
    out
}

/// Whether `codec_name` is a hardware encoder (NVENC, VA-API, AMF or QSV).
pub fn is_hardware(codec_name: &str) -> bool {
    ["nvenc", "vaapi", "amf", "qsv"]
        .iter()
        .any(|h| codec_name.contains(h))
}

/// The `thread_count` an encoder opens with: `Some(0)`, FFmpeg's automatic
/// count, for software encoders (most of their wrappers otherwise run the
/// library on one thread), and `None` to leave hardware encoders as they
/// are.
pub fn thread_count(codec_name: &str) -> Option<i32> {
    (!is_hardware(codec_name)).then_some(0)
}

/// The `(option, value)` pairs that map preset `1..=7` (clamped) onto an
/// encoder's private options, or `None` for an encoder the policy does not
/// know. VA-API deliberately gets no quality option: its meaning differs
/// between drivers. `libopenh264` additionally needs `max_b_frames = 0`,
/// which the encoder applies itself.
///
/// Every pair is one the encoder accepts (tested against the FFmpeg the
/// tests run with): QSV's slowest preset is `veryslow`, only `h264_qsv`
/// has `look_ahead`, NVENC's `cbr` (default off) is gone in FFmpeg 9, and
/// libkvazaar takes its preset inside `kvazaar-params`.
pub fn preset_options(codec_name: &str, preset: u8) -> Option<Vec<(&'static str, String)>> {
    let p = preset.clamp(1, 7) as usize;
    let s = |v: &str| v.to_string();
    const X26X: [&str; 7] = [
        "ultrafast",
        "superfast",
        "veryfast",
        "fast",
        "medium",
        "slow",
        "veryslow",
    ];
    Some(if codec_name.contains("nvenc") {
        vec![
            ("preset", format!("p{p}")),
            ("tune", s("ll")),
            ("rc", s("vbr")),
            ("forced-idr", s("1")),
        ]
    } else if codec_name.contains("amf") {
        const AMF: [&str; 7] = [
            "speed", "speed", "balanced", "balanced", "balanced", "quality", "quality",
        ];
        vec![("quality", s(AMF[p - 1])), ("rc", s("vbr_latency"))]
    } else if codec_name.contains("_vaapi") {
        Vec::new()
    } else if codec_name.contains("qsv") {
        const QSV: [&str; 7] = [
            "veryfast", "fast", "medium", "medium", "slow", "slower", "veryslow",
        ];
        let mut v = vec![("preset", s(QSV[p - 1]))];
        if codec_name.starts_with("h264") {
            v.push(("look_ahead", s("0")));
        }
        v
    } else if codec_name.contains("openh264") {
        vec![
            ("rc_mode", s("bitrate")),
            ("allow_skip_frames", s("0")),
            ("profile", s("high")),
        ]
    } else if codec_name.contains("kvazaar") {
        vec![("kvazaar-params", format!("preset={}", X26X[p - 1]))]
    } else if codec_name.contains("libx26") {
        vec![("preset", s(X26X[p - 1])), ("tune", s("zerolatency"))]
    } else if codec_name.contains("svtav1") {
        const SVT: [u8; 7] = [12, 9, 7, 6, 4, 1, 0];
        vec![("preset", SVT[p - 1].to_string())]
    } else if codec_name.contains("libaom") {
        Vec::new()
    } else {
        return None;
    })
}
