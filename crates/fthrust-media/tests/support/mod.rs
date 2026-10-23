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
        title: "Default Mix",
    };
    let tracks: &[ClipAudio<'_>] = if audio {
        std::slice::from_ref(&track)
    } else {
        &[]
    };
    write_clip(path, &video, tracks, None).unwrap();
    Some(())
}

/// A clip with one AAC track per tone, `(label, hz)`, 320×180 at 30 fps
/// whose frame `n` is `synthetic_bgra(320, 180, n)`. Returns the tracks'
/// container stream indexes (video is stream 0).
#[allow(dead_code)]
pub fn write_tone_clip(
    path: &std::path::Path,
    seconds: u32,
    tones: &[(&str, f32)],
) -> Option<Vec<usize>> {
    use fthrust_core::capture_config::{Codec, EncoderPref};
    use fthrust_media::clip_writer::{ClipAudio, ClipVideo, write_clip};
    use fthrust_media::encoder::{Encoder, EncoderConfig};
    use fthrust_media::ring::EncodedRing;

    skip_unless_encoder(&["libopenh264", "libx264"])?;
    const FPS: u32 = 30;
    const SECOND: i64 = 1_000_000_000;
    let (w, h) = (320u32, 180u32);
    let cfg = EncoderConfig {
        src_width: w,
        src_height: h,
        enc_width: w,
        enc_height: h,
        fps: FPS,
        bitrate_kbps: 600,
        codec: Codec::H264,
        encoder: EncoderPref::Software,
        preset: 1,
    };
    let mut encoder = Encoder::open(&cfg).unwrap();
    let ring = EncodedRing::new(u64::from(seconds) * 1000 + 5000, FPS, 0);
    let mut out = Vec::new();
    for i in 0..seconds * FPS {
        let bgra = synthetic_bgra(w, h, i);
        encoder
            .encode(
                &bgra,
                (w * 4) as usize,
                w,
                h,
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
        width: w,
        height: h,
        bitrate_kbps: 600,
        separate_audio: tones.len() > 1,
    };
    let pcm: Vec<Vec<f32>> = tones
        .iter()
        .map(|(_, hz)| {
            (0..u64::from(seconds) * 48_000)
                .flat_map(|i| {
                    let v = (i as f32 * hz * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
                    [v, v]
                })
                .collect()
        })
        .collect();
    let tracks: Vec<ClipAudio<'_>> = tones
        .iter()
        .zip(pcm.iter())
        .map(|((label, _), samples)| ClipAudio {
            samples,
            sample_rate: 48_000,
            channels: 2,
            title: label,
        })
        .collect();
    let result = write_clip(path, &video, &tracks, None).unwrap();
    assert_eq!(result.audio_tracks.len(), tones.len(), "{result:?}");
    Some((1..=tones.len()).collect())
}

/// Goertzel power of `hz` in the left channel of interleaved stereo `pcm`.
#[allow(dead_code)]
pub fn tone_power(pcm: &[f32], hz: f32, rate: f32) -> f32 {
    let n = pcm.len() / 2;
    let k = (0.5 + n as f32 * hz / rate).floor();
    let w = std::f32::consts::TAU * k / n as f32;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for i in 0..n {
        let s = pcm[i * 2] + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2) / n as f32
}

/// A silent-audio clip whose encoder is asked for a keyframe every
/// `gop_frames` frames (`None` = the encoder's default, a long GOP).
#[allow(dead_code)]
pub fn write_gop_clip(path: &std::path::Path, seconds: u32, gop_frames: Option<u32>) -> Option<()> {
    use fthrust_core::capture_config::{Codec, EncoderPref};
    use fthrust_media::clip_writer::{ClipAudio, ClipVideo, write_clip};
    use fthrust_media::encoder::{Encoder, EncoderConfig};
    use fthrust_media::ring::EncodedRing;

    skip_unless_encoder(&["libopenh264", "libx264"])?;
    const FPS: u32 = 30;
    const SECOND: i64 = 1_000_000_000;
    let (w, h) = (320u32, 180u32);
    let cfg = EncoderConfig {
        src_width: w,
        src_height: h,
        enc_width: w,
        enc_height: h,
        fps: FPS,
        bitrate_kbps: 600,
        codec: Codec::H264,
        encoder: EncoderPref::Software,
        preset: 1,
    };
    // `None` keeps one keyframe for the whole clip (a long GOP).
    let mut encoder = Encoder::open_with_gop(&cfg, gop_frames.unwrap_or(10_000)).unwrap();
    let ring = EncodedRing::new(u64::from(seconds) * 1000 + 5000, FPS, 0);
    let mut out = Vec::new();
    for i in 0..seconds * FPS {
        let bgra = synthetic_bgra(w, h, i);
        encoder
            .encode(
                &bgra,
                (w * 4) as usize,
                w,
                h,
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
        width: w,
        height: h,
        bitrate_kbps: 600,
        separate_audio: false,
    };
    let samples: Vec<f32> = vec![0.0; (u64::from(seconds) * 48_000 * 2) as usize];
    let audio = ClipAudio {
        samples: &samples,
        sample_rate: 48_000,
        channels: 2,
        title: "Default Mix",
    };
    write_clip(path, &video, &[audio], None).unwrap();
    Some(())
}
