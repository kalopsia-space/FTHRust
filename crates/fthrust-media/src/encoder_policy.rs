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

/// The `(option, value)` pairs that map preset `1..=7` (clamped) onto an
/// encoder's private options, or `None` for an encoder the policy does not
/// know. VA-API deliberately gets no quality option: its meaning differs
/// between drivers. `libopenh264` additionally needs `max_b_frames = 0`,
/// which the encoder applies itself.
pub fn preset_options(codec_name: &str, preset: u8) -> Option<Vec<(&'static str, String)>> {
    let p = preset.clamp(1, 7) as usize;
    let s = |v: &str| v.to_string();
    Some(if codec_name.contains("nvenc") {
        vec![
            ("preset", format!("p{p}")),
            ("tune", s("ll")),
            ("rc", s("vbr")),
            ("cbr", s("0")),
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
            "veryfast", "fast", "medium", "medium", "slow", "slower", "slowest",
        ];
        vec![("preset", s(QSV[p - 1])), ("look_ahead", s("0"))]
    } else if codec_name.contains("openh264") {
        vec![
            ("rc_mode", s("bitrate")),
            ("allow_skip_frames", s("0")),
            ("profile", s("high")),
        ]
    } else if codec_name.contains("kvazaar") || codec_name.contains("libx26") {
        const NAMES: [&str; 7] = [
            "ultrafast",
            "superfast",
            "veryfast",
            "fast",
            "medium",
            "slow",
            "veryslow",
        ];
        let mut v = vec![("preset", s(NAMES[p - 1]))];
        if codec_name.contains("libx26") {
            v.push(("tune", s("zerolatency")));
        }
        v
    } else if codec_name.contains("svtav1") {
        const SVT: [u8; 7] = [12, 9, 7, 6, 4, 1, 0];
        vec![("preset", SVT[p - 1].to_string())]
    } else if codec_name.contains("libaom") {
        Vec::new()
    } else {
        return None;
    })
}
