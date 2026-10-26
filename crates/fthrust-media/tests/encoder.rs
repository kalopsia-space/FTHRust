#[path = "support/decode.rs"]
mod decode;
mod support;

use fthrust_core::capture_config::{Codec, EncoderPref};
use fthrust_media::MediaError;
use fthrust_media::encoder::{Conversion, Encoder, EncoderConfig, GpuFailure};
use fthrust_media::ring::EncodedPacket;
use fthrust_media::vaapi::vaapi_available;

const SECOND: i64 = 1_000_000_000;

fn software_config(width: u32, height: u32, fps: u32) -> EncoderConfig {
    EncoderConfig {
        src_width: width,
        src_height: height,
        enc_width: width,
        enc_height: height,
        fps,
        bitrate_kbps: 300,
        codec: Codec::H264,
        encoder: EncoderPref::Software,
        preset: 1,
    }
}

#[test]
fn keeps_output_size_when_source_changes() {
    if support::skip_unless_encoder(&["libopenh264", "libx264"]).is_none() {
        return;
    }
    let (width, height, fps) = (160u32, 90u32, 30u32);
    let frame_ns = SECOND / fps as i64;
    let mut encoder = Encoder::open(&software_config(width, height, fps)).unwrap();
    let mut out: Vec<EncodedPacket> = Vec::new();
    let sizes = [
        (width, height, 0usize),
        (320, 180, 64),
        (96, 54, 0),
        (width, height, 0),
    ];
    let mut wall = SECOND;
    for (w, h, padding) in sizes {
        let stride = w as usize * 4 + padding;
        let bgra = vec![0x60u8; stride * h as usize];
        for _ in 0..15 {
            wall += frame_ns;
            encoder.encode(&bgra, stride, w, h, wall, &mut out).unwrap();
        }
    }
    encoder.flush(&mut out).unwrap();
    assert_eq!((encoder.width(), encoder.height()), (width, height));
    assert!(out.len() >= 30, "{} packets", out.len());
    assert!(out.iter().all(|p| !p.data.is_empty()));
}

#[test]
fn rejects_short_stride() {
    if support::skip_unless_encoder(&["libopenh264", "libx264"]).is_none() {
        return;
    }
    let mut encoder = Encoder::open(&software_config(64, 8, 30)).unwrap();
    let tiny = vec![0u8; 64 * 4 * 8];
    let mut out = Vec::new();
    let err = encoder
        .encode(&tiny, 64 * 4 - 4, 64, 8, SECOND, &mut out)
        .unwrap_err();
    assert!(matches!(err, MediaError::Invalid(_)), "{err}");
    let short_buffer = vec![0u8; 10];
    let err = encoder
        .encode(&short_buffer, 64 * 4, 64, 8, SECOND, &mut out)
        .unwrap_err();
    assert!(
        matches!(err, MediaError::Invalid(_)),
        "buffer shorter than stride*height: {err}"
    );
    assert!(
        encoder
            .encode(&tiny, 64 * 4, 0, 8, SECOND, &mut out)
            .is_err()
    );
}

#[test]
fn first_pts_zero_strictly_increasing_keyframe_every_gop() {
    if support::skip_unless_encoder(&["libopenh264", "libx264"]).is_none() {
        return;
    }
    let (width, height, fps) = (160u32, 90u32, 30u32);
    let mut encoder = Encoder::open(&software_config(width, height, fps)).unwrap();
    encoder.set_generation(3);
    let mut out = Vec::new();
    let mut wall = 100 * SECOND;
    for frame in 0..120u32 {
        wall += if frame % 5 == 0 {
            SECOND / 15
        } else {
            SECOND / 30
        };
        let bgra = support::synthetic_bgra(width, height, frame);
        encoder
            .encode(&bgra, width as usize * 4, width, height, wall, &mut out)
            .unwrap();
    }
    encoder.flush(&mut out).unwrap();
    assert!(out.len() >= 40, "{}", out.len());
    assert_eq!(out[0].pts, 0);
    assert!(out.iter().all(|p| p.generation == 3));
    let mut gap = false;
    let mut previous_key = if out[0].keyframe {
        Some(out[0].pts)
    } else {
        None
    };
    let mut keyframes = usize::from(out[0].keyframe);
    for i in 1..out.len() {
        assert!(out[i].pts > out[i - 1].pts, "pts not increasing at {i}");
        assert!(
            out[i].wall_time_ns > out[i - 1].wall_time_ns,
            "wall time not increasing at {i}"
        );
        gap |= out[i].pts - out[i - 1].pts > 1;
        if out[i].keyframe {
            if let Some(prev) = previous_key {
                assert!(out[i].pts - prev <= 2 * fps as i64 + 1);
            }
            previous_key = Some(out[i].pts);
            keyframes += 1;
        }
    }
    assert!(
        gap,
        "double-length capture gaps must stay visible in media time"
    );
    assert!(keyframes >= 2);
    let media_span = (out.last().unwrap().pts - out[0].pts) * SECOND / fps as i64;
    let capture_span = out.last().unwrap().wall_time_ns - out[0].wall_time_ns;
    assert!((media_span - capture_span).abs() <= SECOND / fps as i64);
    assert!(
        !encoder.extradata().is_empty(),
        "global header carries SPS/PPS"
    );
    assert_eq!(encoder.codec_id(), ffmpeg_next::codec::Id::H264);
}

#[test]
fn open_fails_without_any_candidate() {
    fthrust_media::init().unwrap();
    let cfg = EncoderConfig {
        encoder: EncoderPref::Nvenc,
        codec: Codec::Av1,
        ..software_config(64, 64, 30)
    };
    if ffmpeg_next::encoder::find_by_name("av1_nvenc").is_some() && vaapi_available() {
        return;
    }
    match Encoder::open(&cfg) {
        Err(MediaError::EncoderUnavailable(msg)) => assert!(msg.contains("av1_nvenc"), "{msg}"),
        other => panic!(
            "expected EncoderUnavailable, got {:?}",
            other.map(|e| e.active_codec().to_string())
        ),
    }
}

#[test]
fn vaapi_encodes_when_device_present() {
    fthrust_media::init().unwrap();
    if !vaapi_available() {
        eprintln!("skipping: no VA-API device");
        return;
    }
    let cfg = EncoderConfig {
        src_width: 320,
        src_height: 180,
        enc_width: 320,
        enc_height: 180,
        fps: 30,
        bitrate_kbps: 1500,
        codec: Codec::H264,
        encoder: EncoderPref::Vaapi,
        preset: 4,
    };
    let mut encoder = match Encoder::open(&cfg) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skipping: h264_vaapi cannot initialise: {e}");
            return;
        }
    };
    if encoder.active_codec() != "h264_vaapi" {
        eprintln!(
            "skipping: got {} instead of h264_vaapi",
            encoder.active_codec()
        );
        return;
    }
    let mut out = Vec::new();
    for frame in 0..60u32 {
        let bgra = support::synthetic_bgra(320, 180, frame);
        encoder
            .encode(
                &bgra,
                320 * 4,
                320,
                180,
                frame as i64 * SECOND / 30,
                &mut out,
            )
            .unwrap();
    }
    encoder.flush(&mut out).unwrap();
    assert!(!out.is_empty());
    assert!(out.iter().all(|p| !p.data.is_empty()));
    assert!(out[0].keyframe);
}

#[test]
fn vaapi_open_iterates_devices_until_one_encodes() {
    use fthrust_media::vaapi::{candidate_devices, vaapi_available};
    let cfg = EncoderConfig {
        src_width: 320,
        src_height: 180,
        enc_width: 320,
        enc_height: 180,
        fps: 30,
        bitrate_kbps: 500,
        codec: Codec::H264,
        encoder: EncoderPref::Vaapi,
        preset: 4,
    };
    // A node that cannot host an encoder must not stop the search.
    let bogus = vec!["/dev/null".to_string()];
    let err = Encoder::open_with_devices(&cfg, &bogus)
        .unwrap_err()
        .to_string();
    assert!(err.contains("/dev/null"), "{err}");
    if !vaapi_available() {
        eprintln!("skipping: no VA-API device");
        return;
    }
    let mut devices = bogus;
    devices.extend(candidate_devices());
    let enc = Encoder::open_with_devices(&cfg, &devices).unwrap();
    assert!(enc.is_vaapi());
    assert_ne!(enc.vaapi_device(), Some("/dev/null"));
}

fn vaapi_cfg(width: u32, height: u32) -> EncoderConfig {
    EncoderConfig {
        src_width: width,
        src_height: height,
        enc_width: width,
        enc_height: height,
        fps: 30,
        bitrate_kbps: 4000,
        codec: Codec::H264,
        encoder: EncoderPref::Vaapi,
        preset: 4,
    }
}

/// Opens an h264_vaapi encoder, or `None` (with a skip note) when the machine
/// has no VA-API device.
fn open_vaapi(src_w: u32, src_h: u32, enc_w: u32, enc_h: u32) -> Option<Encoder> {
    fthrust_media::init().unwrap();
    if !vaapi_available() {
        eprintln!("skipping: no VA-API device");
        return None;
    }
    let cfg = EncoderConfig {
        src_width: src_w,
        src_height: src_h,
        enc_width: enc_w,
        enc_height: enc_h,
        ..vaapi_cfg(enc_w, enc_h)
    };
    let enc = Encoder::open(&cfg).unwrap();
    assert_eq!(enc.active_codec(), "h264_vaapi");
    Some(enc)
}

fn encode_synthetic(
    enc: &mut Encoder,
    w: u32,
    h: u32,
    frames: u32,
    first: u32,
) -> Vec<EncodedPacket> {
    let mut out = Vec::new();
    for i in 0..frames {
        let bgra = support::synthetic_bgra(w, h, first + i);
        enc.encode(
            &bgra,
            w as usize * 4,
            w,
            h,
            (first + i) as i64 * SECOND / 30,
            &mut out,
        )
        .unwrap();
    }
    out
}

#[test]
fn vaapi_encoder_converts_on_the_gpu() {
    let Some(enc) = open_vaapi(1920, 1080, 1920, 1080) else {
        return;
    };
    assert_eq!(enc.conversion(), Conversion::Gpu);
}

#[test]
fn gpu_and_cpu_paths_decode_to_the_same_colours() {
    let Some(mut gpu) = open_vaapi(320, 180, 320, 180) else {
        return;
    };
    let mut cpu = Encoder::open_forcing_cpu_conversion(&vaapi_cfg(320, 180)).unwrap();
    assert_eq!(gpu.conversion(), Conversion::Gpu);
    assert_eq!(cpu.conversion(), Conversion::Cpu);
    let mut decoded = Vec::new();
    for enc in [&mut gpu, &mut cpu] {
        let mut packets = encode_synthetic(enc, 320, 180, 30, 0);
        enc.flush(&mut packets).unwrap();
        decoded.push(decode::decode_frame_rgb(&enc.extradata(), &packets, 15));
    }
    let (a, b) = (&decoded[0], &decoded[1]);
    assert_eq!((a.1, a.2), (320, 180));
    assert_eq!((b.1, b.2), (320, 180));
    for c in 0..3 {
        let sum: u64 =
            a.0.iter()
                .skip(c)
                .step_by(3)
                .zip(b.0.iter().skip(c).step_by(3))
                .map(|(x, y)| u64::from(x.abs_diff(*y)))
                .sum();
        let mean = sum as f64 / (320.0 * 180.0);
        assert!(mean <= 4.0, "channel {c} differs by {mean:.2} on average");
    }
}

#[test]
fn gpu_path_keeps_output_size_when_source_changes() {
    let Some(mut enc) = open_vaapi(1920, 1080, 1920, 1080) else {
        return;
    };
    let mut out = encode_synthetic(&mut enc, 1920, 1080, 10, 0);
    out.extend(encode_synthetic(&mut enc, 1280, 720, 10, 10));
    enc.flush(&mut out).unwrap();
    assert_eq!(enc.conversion(), Conversion::Gpu);
    let (_, w, h) = decode::decode_frame_rgb(&enc.extradata(), &out, 15);
    assert_eq!((w, h), (1920, 1080));
}

#[test]
fn falls_back_to_cpu_when_forced() {
    fthrust_media::init().unwrap();
    if !vaapi_available() {
        eprintln!("skipping: no VA-API device");
        return;
    }
    let enc = Encoder::open_forcing_cpu_conversion(&vaapi_cfg(320, 180)).unwrap();
    assert_eq!(enc.conversion(), Conversion::Cpu);
    assert!(Encoder::encoder_works("h264_vaapi"));
}

#[test]
fn gpu_failures_fall_back_to_cpu_conversion() {
    fthrust_media::init().unwrap();
    if !vaapi_available() {
        eprintln!("skipping: no VA-API device");
        return;
    }
    for failure in [GpuFailure::Open, GpuFailure::FirstConvert] {
        let mut enc =
            Encoder::open_with_injected_gpu_failure(&vaapi_cfg(320, 180), failure).unwrap();
        let mut out = encode_synthetic(&mut enc, 320, 180, 10, 0);
        enc.flush(&mut out).unwrap();
        assert_eq!(enc.conversion(), Conversion::Cpu, "{failure:?}");
        assert!(!out.is_empty() && out[0].keyframe, "{failure:?}");
        assert!(out.iter().all(|p| !p.data.is_empty()));
    }
}

#[test]
fn first_send_after_a_size_change_can_fall_back_to_cpu() {
    let Some(mut enc) = open_vaapi(320, 180, 320, 180) else {
        return;
    };
    let mut out = encode_synthetic(&mut enc, 320, 180, 5, 0);
    assert_eq!(enc.conversion(), Conversion::Gpu);
    enc.inject_gpu_convert_failure();
    out.extend(encode_synthetic(&mut enc, 160, 90, 5, 5));
    enc.flush(&mut out).unwrap();
    assert_eq!(enc.conversion(), Conversion::Cpu);
    let (_, w, h) = decode::decode_frame_rgb(&enc.extradata(), &out, 7);
    assert_eq!((w, h), (320, 180));
}

#[test]
fn repeats_keep_pts_monotonic_and_keyframes() {
    let Some(mut enc) = open_vaapi(320, 180, 320, 180) else {
        return;
    };
    let mut out = encode_synthetic(&mut enc, 320, 180, 5, 0);
    for i in 5..10 {
        assert!(
            enc.encode_repeat(i64::from(i) * SECOND / 30, &mut out)
                .unwrap()
        );
    }
    enc.request_keyframe();
    assert!(enc.encode_repeat(10 * SECOND / 30, &mut out).unwrap());
    out.extend(encode_synthetic(&mut enc, 320, 180, 5, 11));
    enc.flush(&mut out).unwrap();
    assert_eq!(out.len(), 16);
    assert!(
        out.windows(2).all(|w| w[0].pts < w[1].pts),
        "pts not increasing"
    );
    let after_request = out.iter().find(|p| p.pts == 10).unwrap();
    assert!(after_request.keyframe, "repeat after a keyframe request");
    let (a, ..) = decode::decode_frame_rgb(&enc.extradata(), &out, 4);
    let (b, ..) = decode::decode_frame_rgb(&enc.extradata(), &out, 7);
    assert_eq!(a.len(), b.len());
    let worst = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
    let mean = a
        .iter()
        .zip(&b)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum::<u64>() as f64
        / a.len() as f64;
    // The codec refines a still image over the repeats, so a few edge pixels
    // move by more than 2; the picture as a whole must stay put.
    assert!(mean <= 0.5 && worst <= 8, "mean {mean:.2}, worst {worst}");
}

#[test]
fn repeat_without_previous_frame_returns_false() {
    let Some(mut enc) = open_vaapi(320, 180, 320, 180) else {
        return;
    };
    let mut out = Vec::new();
    assert!(!enc.encode_repeat(0, &mut out).unwrap());
    assert!(out.is_empty());
    assert_eq!(enc.conversions(), 0);
}

#[test]
fn repeats_release_old_surfaces_on_gpu_and_cpu_paths() {
    let Some(gpu) = open_vaapi(320, 180, 320, 180) else {
        return;
    };
    let cpu = Encoder::open_forcing_cpu_conversion(&vaapi_cfg(320, 180)).unwrap();
    for mut enc in [gpu, cpu] {
        let mut out = encode_synthetic(&mut enc, 320, 180, 3, 0);
        assert!(enc.encode_repeat(3 * SECOND / 30, &mut out).unwrap());
        assert!(enc.holds_repeat_reference());
        // Far more new frames than the surface pool is deep.
        out.extend(encode_synthetic(&mut enc, 320, 180, 40, 4));
        assert!(!enc.holds_repeat_reference());
        assert!(enc.encode_repeat(44 * SECOND / 30, &mut out).unwrap());
        // A size change rebuilds the converter or scaler and drops it too.
        out.extend(encode_synthetic(&mut enc, 160, 90, 3, 45));
        assert!(!enc.holds_repeat_reference());
        enc.flush(&mut out).unwrap();
        assert!(out.windows(2).all(|w| w[0].pts < w[1].pts));
    }
}
