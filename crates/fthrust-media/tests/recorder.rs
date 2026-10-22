mod support;

use std::path::Path;

use fthrust_core::capture_config::{Codec, EncoderPref};
use fthrust_media::encoder::{Encoder, EncoderConfig};
use fthrust_media::recorder::{Recorder, RecorderAudio, RecorderConfig};
use fthrust_media::ring::EncodedPacket;

const SECOND: i64 = 1_000_000_000;
const FPS: u32 = 30;

struct Source {
    packets: Vec<EncodedPacket>,
    config: RecorderConfig,
}

/// Encodes `seconds` of synthetic 160×90 video (keyframe every 2 s).
fn encode(seconds: u32, audio: Option<RecorderAudio>) -> Option<Source> {
    support::skip_unless_encoder(&["libopenh264", "libx264"])?;
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
    let mut packets = Vec::new();
    for i in 0..seconds * FPS {
        let bgra = support::synthetic_bgra(160, 90, i);
        encoder
            .encode(
                &bgra,
                160 * 4,
                160,
                90,
                i as i64 * SECOND / FPS as i64,
                &mut packets,
            )
            .unwrap();
    }
    encoder.flush(&mut packets).unwrap();
    Some(Source {
        packets,
        config: RecorderConfig {
            codec_id: encoder.codec_id(),
            extradata: encoder.extradata(),
            width: 160,
            height: 90,
            fps: FPS,
            bitrate_kbps: 300,
            audio,
        },
    })
}

struct Probe {
    duration_s: f64,
    video_packets: usize,
    first_video_pts: i64,
    streams: usize,
    audio_codec: Option<ffmpeg_next::codec::Id>,
}

fn probe(path: &Path) -> Probe {
    let mut ictx = ffmpeg_next::format::input(path).unwrap();
    let video = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .unwrap();
    let index = video.index();
    let tb = video.time_base();
    let duration_s = video.duration() as f64 * tb.numerator() as f64 / tb.denominator() as f64;
    let streams = ictx.nb_streams() as usize;
    let audio_codec = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Audio)
        .map(|s| s.parameters().id());
    let mut video_packets = 0;
    let mut first_video_pts = None;
    for (stream, packet) in ictx.packets() {
        if stream.index() == index {
            video_packets += 1;
            if first_video_pts.is_none() {
                first_video_pts = packet.pts();
            }
        }
    }
    Probe {
        duration_s,
        video_packets,
        first_video_pts: first_video_pts.unwrap_or(i64::MIN),
        streams,
        audio_codec,
    }
}

#[test]
fn fragments_survive_abrupt_drop() {
    let Some(src) = encode(5, None) else { return };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("rec.mp4");
    let mut rec = Recorder::start(&out, &src.config).unwrap();
    for p in &src.packets {
        rec.write_video(p).unwrap();
    }
    // Simulate a crash: never finish, never run Drop.
    std::mem::forget(rec);
    let p = probe(&out);
    // Keyframes every 2 s → fragments committed at 2 s and 4 s.
    assert!(
        p.duration_s >= 3.9,
        "committed fragments give {} s",
        p.duration_s
    );
    assert!(
        p.video_packets >= 4 * FPS as usize,
        "{} packets",
        p.video_packets
    );
}

#[test]
fn finish_writes_trailer_and_duration() {
    let audio = RecorderAudio {
        sample_rate: 48_000,
        channels: 2,
    };
    let Some(src) = encode(5, Some(audio)) else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("rec.mp4");
    let mut rec = Recorder::start(&out, &src.config).unwrap();
    let mut audio_cursor = 0usize;
    let tone: Vec<f32> = (0..5 * 48_000)
        .flat_map(|i| {
            let v = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
            [v, v]
        })
        .collect();
    for p in &src.packets {
        rec.write_video(p).unwrap();
        // Feed one frame's worth of audio after each video packet.
        let next = (audio_cursor + 2 * 1600).min(tone.len());
        rec.write_audio(&tone[audio_cursor..next]).unwrap();
        audio_cursor = next;
    }
    let bytes = rec.finish().unwrap();
    assert!(bytes > 0);
    assert_eq!(bytes, std::fs::metadata(&out).unwrap().len());
    let p = probe(&out);
    assert!(
        (p.duration_s - 5.0).abs() <= 0.1,
        "duration {}",
        p.duration_s
    );
    assert_eq!(p.video_packets, 5 * FPS as usize);
    assert_eq!(p.first_video_pts, 0);
    assert_eq!(p.streams, 2);
    assert_eq!(p.audio_codec, Some(ffmpeg_next::codec::Id::AAC));
}

#[test]
fn skips_until_first_keyframe_and_rebases_pts() {
    let Some(src) = encode(5, None) else { return };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("rec.mp4");
    let mut rec = Recorder::start(&out, &src.config).unwrap();
    // Drop the first keyframe so the recorder sees 59 non-key packets first.
    let second_key = src.packets.iter().skip(1).position(|p| p.keyframe).unwrap() + 1;
    assert_eq!(second_key, 60, "GOP is 2 s");
    for p in &src.packets[1..] {
        rec.write_video(p).unwrap();
    }
    rec.finish().unwrap();
    let p = probe(&out);
    assert_eq!(
        p.first_video_pts, 0,
        "presentation rebased to the first keyframe"
    );
    assert_eq!(p.video_packets, src.packets.len() - second_key);
}

#[test]
fn audio_before_video_is_ignored_and_empty_extradata_rejected() {
    let audio = RecorderAudio {
        sample_rate: 48_000,
        channels: 2,
    };
    let Some(src) = encode(3, Some(audio)) else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("rec.mp4");
    let mut rec = Recorder::start(&out, &src.config).unwrap();
    rec.write_audio(&vec![0.0f32; 9600]).unwrap();
    for p in &src.packets {
        rec.write_video(p).unwrap();
    }
    rec.finish().unwrap();
    assert_eq!(probe(&out).first_video_pts, 0);

    let bad = RecorderConfig {
        extradata: Vec::new(),
        ..src.config.clone()
    };
    assert!(Recorder::start(&tmp.path().join("bad.mp4"), &bad).is_err());
}

#[test]
fn start_refuses_existing_file() {
    let Some(src) = encode(1, None) else { return };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("precious.mp4");
    std::fs::write(&out, b"someone else's recording").unwrap();
    let err = Recorder::start(&out, &src.config).unwrap_err();
    assert!(err.to_string().contains("exists"), "{err}");
    assert_eq!(
        std::fs::read(&out).unwrap(),
        b"someone else's recording",
        "untouched"
    );
}
