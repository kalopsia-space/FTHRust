//! Shared helpers for the media integration tests.
#![allow(dead_code)]

/// Returns the first of `names` that is an available encoder, or `None`
/// after printing a skip notice (the reference tests skip the same way).
pub fn skip_unless_encoder(names: &[&'static str]) -> Option<&'static str> {
    fthrust_media::init().expect("ffmpeg init");
    let found = names
        .iter()
        .copied()
        .find(|name| ffmpeg_next::encoder::find_by_name(name).is_some());
    if found.is_none() {
        eprintln!("skipping: none of {names:?} is available in this FFmpeg build");
    }
    found
}

/// A BGRA frame whose gradient moves with `frame_index`, so consecutive
/// frames differ and encoders produce real motion.
pub fn synthetic_bgra(width: u32, height: u32, frame_index: u32) -> Vec<u8> {
    let mut out = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            let shift = frame_index.wrapping_mul(3);
            out[i] = (x.wrapping_add(shift) & 0xff) as u8; // B
            out[i + 1] = (y.wrapping_add(shift) & 0xff) as u8; // G
            out[i + 2] = ((x ^ y).wrapping_add(shift) & 0xff) as u8; // R
            out[i + 3] = 0xff;
        }
    }
    out
}

/// Encodes `seconds` of synthetic 160×90 H.264 at 30 fps (300 kbps) and
/// writes it through the clip writer, optionally with a 440 Hz AAC track.
/// Returns `None` (after printing a skip note) when no H.264 software
/// encoder exists.
#[allow(dead_code)]
pub fn write_test_clip(path: &std::path::Path, seconds: u32, audio: bool) -> Option<()> {
    use fthrust_core::capture_config::{Codec, EncoderPref};
    use fthrust_media::clip_writer::{ClipAudio, ClipVideo, write_clip};
    use fthrust_media::encoder::{Encoder, EncoderConfig};
    use fthrust_media::ring::EncodedRing;

    skip_unless_encoder(&["libopenh264", "libx264"])?;
    const FPS: u32 = 30;
    const SECOND: i64 = 1_000_000_000;
    let cfg = EncoderConfig {
        src_width: 160,
        src_height: 90,
        enc_width: 160,
        enc_height: 90,
        fps: FPS,
        bitrate_kbps: 300,
        codec: Codec::H264,
        encoder: EncoderPref::Software,
        preset: 1,
    };
    let mut encoder = Encoder::open(&cfg).unwrap();
    let ring = EncodedRing::new(u64::from(seconds) * 1000 + 5000, FPS, 0);
    let mut out = Vec::new();
    for i in 0..seconds * FPS {
        let bgra = synthetic_bgra(160, 90, i);
        encoder
            .encode(
                &bgra,
                160 * 4,
                160,
                90,
                i64::from(i) * SECOND / i64::from(FPS),
                &mut out,
            )
            .unwrap();
    }
    encoder.flush(&mut out).unwrap();
    for p in out {
        ring.push(p);
    }
    let snapshot = ring
        .snapshot(seconds * 1000, i64::from(seconds) * SECOND, 0)
        .unwrap();
    let video = ClipVideo {
        snapshot: &snapshot,
        codec_id: encoder.codec_id(),
        extradata: &encoder.extradata(),
        width: 160,
        height: 90,
        bitrate_kbps: 300,
        separate_audio: false,
    };
    let samples: Vec<f32> = (0..u64::from(seconds) * 48_000)
        .flat_map(|i| {
            let v = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
            [v, v]
        })
        .collect();
    let track = ClipAudio {
        samples: &samples,
        sample_rate: 48_000,
        channels: 2,
    };
    write_clip(path, &video, audio.then_some(&track), None).unwrap();
    Some(())
}
