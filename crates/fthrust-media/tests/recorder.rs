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

fn track(title: &str) -> RecorderAudio {
    RecorderAudio {
        sample_rate: 48_000,
        channels: 2,
        title: title.into(),
    }
}

/// Encodes `seconds` of synthetic 160×90 video (keyframe every 2 s).
fn encode(seconds: u32, audio: Vec<RecorderAudio>) -> Option<Source> {
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
    let Some(src) = encode(5, Vec::new()) else {
        return;
    };
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
    let Some(src) = encode(5, vec![track("Default Mix")]) else {
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
        rec.write_audio(0, &tone[audio_cursor..next]).unwrap();
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
    let Some(src) = encode(5, Vec::new()) else {
        return;
    };
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
    let Some(src) = encode(3, vec![track("Default Mix")]) else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("rec.mp4");
    let mut rec = Recorder::start(&out, &src.config).unwrap();
    rec.write_audio(0, &vec![0.0f32; 9600]).unwrap();
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
    let Some(src) = encode(1, Vec::new()) else {
        return;
    };
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

#[test]
fn a_failed_start_leaves_no_file() {
    // The name is claimed before the muxer opens; when a later step failed,
    // an empty FTHR_recording_*.mp4 stayed behind in the user's folder.
    let Some(src) = encode(1, Vec::new()) else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("FTHR_recording_test.mp4");
    let bad = RecorderConfig {
        audio: vec![RecorderAudio {
            sample_rate: 0,
            ..track("Default Mix")
        }],
        ..src.config.clone()
    };
    assert!(Recorder::start(&out, &bad).is_err());
    assert!(!out.exists(), "the claimed file was removed");
    assert!(
        Recorder::start(&out, &src.config).is_ok(),
        "the name is free again"
    );
}

#[test]
fn decode_timestamps_are_written_as_given() {
    // The recorder wrote dts = pts. Give the packets a reordered pts pattern
    // with a monotonic dts, as an encoder with B-frames would: dts = pts
    // made the muxer reject them, and a correct stream must keep both.
    let Some(src) = encode(2, Vec::new()) else {
        return;
    };
    let mut packets = src.packets.clone();
    // Decode order I P B P B ... with one frame of reorder delay: dts
    // counts packets, the keyframe presents at 1, each P two ahead of its
    // dts and each B at its dts.
    for (i, p) in packets.iter_mut().enumerate() {
        let i = i as i64;
        p.dts = i;
        p.pts = if i == 0 {
            1
        } else if i % 2 == 1 {
            i + 2
        } else {
            i
        };
        p.keyframe = i == 0;
    }
    let last = packets.len() - 1;
    if last % 2 == 1 {
        // An odd last packet would present after a missing frame.
        packets.pop();
    }
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("bframes.mp4");
    let mut rec = Recorder::start(&out, &src.config).unwrap();
    for p in &packets {
        rec.write_video(p).unwrap();
    }
    rec.finish().unwrap();
    let mut ictx = ffmpeg_next::format::input(&out).unwrap();
    let video = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .unwrap();
    let (index, tb) = (video.index(), video.time_base());
    let ticks = |t: i64| t * i64::from(tb.denominator()) / (i64::from(tb.numerator()) * 30);
    let mut read = Vec::new();
    for (stream, packet) in ictx.packets() {
        if stream.index() == index {
            read.push((packet.pts().unwrap(), packet.dts().unwrap()));
        }
    }
    assert_eq!(read.len(), packets.len());
    let first_dts = read[0].1;
    for (got, want) in read.iter().zip(&packets) {
        assert_eq!(
            (got.0 - first_dts, got.1 - first_dts),
            (ticks(want.pts), ticks(want.dts)),
            "pts/dts as written"
        );
    }
}

#[test]
fn tracks_are_written_with_their_titles() {
    let Some(src) = encode(3, vec![track("Default Mix"), track("Microphone")]) else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("two.mp4");
    let mut rec = Recorder::start(&out, &src.config).unwrap();
    assert_eq!(rec.audio_tracks(), 2);
    let second = vec![0.25f32; 2 * 1600];
    for p in &src.packets {
        rec.write_video(p).unwrap();
        rec.write_audio(0, &second).unwrap();
        rec.write_audio(1, &second).unwrap();
    }
    assert!(rec.write_audio(2, &second).is_err(), "no third track");
    rec.finish().unwrap();
    let descriptors = fthrust_media::combined_audio::descriptors_from_file(&out).unwrap();
    // The fragmented MP4 carries the title as the handler name.
    let titles: Vec<&str> = descriptors
        .iter()
        .map(|d| d.handler_name.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(titles, ["Default Mix", "Microphone"]);
}

#[test]
fn the_first_audio_sample_plays_after_the_encoder_priming() {
    // The fragmented MP4 cannot carry an edit list, so the AAC priming plays
    // at the start of every track: a sample handed to the encoder first
    // plays `audio_priming_frames()` into the recording. The engine starts
    // each track's audio that long after the first keyframe.
    let Some(src) = encode(3, vec![track("Default Mix")]) else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("onset.mp4");
    let mut rec = Recorder::start(&out, &src.config).unwrap();
    let priming = rec.audio_priming_frames() as usize;
    assert!(priming > 0, "the AAC encoder primes");
    // Half a second of silence, then a loud tone.
    let onset = 24_000usize;
    let pcm: Vec<f32> = (0..3 * 48_000usize)
        .flat_map(|i| {
            let v = if i < onset {
                0.0
            } else {
                (i as f32 * 1000.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.8
            };
            [v, v]
        })
        .collect();
    let per_packet = pcm.len() / src.packets.len() / 2 * 2;
    for (i, p) in src.packets.iter().enumerate() {
        rec.write_video(p).unwrap();
        let from = (i * per_packet).min(pcm.len());
        let to = ((i + 1) * per_packet).min(pcm.len());
        rec.write_audio(0, &pcm[from..to]).unwrap();
    }
    rec.finish().unwrap();
    let decoded = fthrust_media::audio_decode::decode_to_pcm(&out).unwrap();
    let heard = decoded
        .samples
        .as_chunks::<2>()
        .0
        .iter()
        .position(|f| f[0].abs() > 0.2)
        .expect("the tone is in the recording");
    let expected = onset + priming;
    assert!(
        heard.abs_diff(expected) <= 256,
        "the tone starts at sample {heard}, expected about {expected} (onset {onset} + priming {priming})"
    );
}
