mod support;

use fthrust_core::capture_config::{Codec, EncoderPref};
use fthrust_media::MediaError;
use fthrust_media::encoder::{Encoder, EncoderConfig};
use fthrust_media::ffi::hw::vaapi_available;
use fthrust_media::ring::EncodedPacket;

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
