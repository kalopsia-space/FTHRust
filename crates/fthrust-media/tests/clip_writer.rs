mod support;

use std::path::Path;

use fthrust_core::capture_config::{Codec, EncoderPref};
use fthrust_media::clip_writer::{ClipAudio, ClipVideo, write_clip};
use fthrust_media::encoder::{Encoder, EncoderConfig};
use fthrust_media::ring::{EncodedRing, Snapshot};
use fthrust_media::transactional_save::{Failure, temporary_path_for};

const SECOND: i64 = 1_000_000_000;
const FPS: u32 = 30;

struct Encoded {
    snapshot: Snapshot,
    extradata: Vec<u8>,
    codec_id: ffmpeg_next::codec::Id,
}

/// Encodes 12 s of synthetic video at 30 fps and snapshots the last 5 s.
fn encode_twelve_seconds() -> Option<Encoded> {
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
    let ring = EncodedRing::new(17_000, FPS, 0);
    let mut out = Vec::new();
    for i in 0..12 * FPS {
        let bgra = support::synthetic_bgra(160, 90, i);
        encoder
            .encode(
                &bgra,
                160 * 4,
                160,
                90,
                i as i64 * SECOND / FPS as i64,
                &mut out,
            )
            .unwrap();
    }
    encoder.flush(&mut out).unwrap();
    for p in out {
        ring.push(p);
    }
    let snapshot = ring.snapshot(5_000, 12 * SECOND, 0).unwrap();
    assert!(snapshot.full_history);
    assert!(snapshot.packets[0].keyframe);
    assert!(
        snapshot.packets[0].pts < snapshot.presentation_start_pts,
        "physical pre-roll retained"
    );
    assert_eq!(snapshot.presentation_start_pts, 7 * FPS as i64);
    Some(Encoded {
        snapshot,
        extradata: encoder.extradata(),
        codec_id: encoder.codec_id(),
    })
}

fn video<'a>(e: &'a Encoded, separate_audio: bool) -> ClipVideo<'a> {
    ClipVideo {
        snapshot: &e.snapshot,
        codec_id: e.codec_id,
        extradata: &e.extradata,
        width: 160,
        height: 90,
        bitrate_kbps: 300,
        separate_audio,
    }
}

struct Probe {
    start_time: i64,
    duration_s: f64,
    first_pts: i64,
    streams: usize,
    metadata: Vec<(String, String)>,
}

fn probe(path: &Path) -> Probe {
    let mut ictx = ffmpeg_next::format::input(path).unwrap();
    let video = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .unwrap();
    let index = video.index();
    let tb = video.time_base();
    let start_time = video.start_time();
    let duration_s = video.duration() as f64 * tb.numerator() as f64 / tb.denominator() as f64;
    let streams = ictx.nb_streams() as usize;
    let metadata: Vec<(String, String)> = ictx
        .metadata()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let mut first_pts = None;
    for (stream, packet) in ictx.packets() {
        if stream.index() == index && first_pts.is_none() {
            first_pts = packet.pts();
        }
    }
    Probe {
        start_time,
        duration_s,
        first_pts: first_pts.unwrap(),
        streams,
        metadata,
    }
}

#[test]
fn short_save_actual_mp4() {
    let Some(e) = encode_twelve_seconds() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("clip.mp4");
    let result = write_clip(&out, &video(&e, false), &[], None).unwrap();
    assert_eq!(result.path, out);
    assert!(result.bytes_written > 0);
    assert!(!result.audio_included);
    assert!(result.audio_error.is_none());
    assert!(!temporary_path_for(&out).exists());
    let p = probe(&out);
    assert_eq!(p.start_time, 0, "the clip presents from t=0");
    assert!(
        p.first_pts < 0,
        "pre-roll packets carry negative pts: {}",
        p.first_pts
    );
    assert!(
        (p.duration_s - 5.0).abs() <= 1.0 / FPS as f64,
        "duration {}",
        p.duration_s
    );
    assert_eq!(p.streams, 1);
}

#[test]
fn metadata_tags_present() {
    let Some(e) = encode_twelve_seconds() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("clip.mp4");
    write_clip(&out, &video(&e, true), &[], None).unwrap();
    let p = probe(&out);
    let get = |k: &str| {
        p.metadata
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(get("comment").as_deref(), Some("fthr-audio-mode=separated"));
    assert_eq!(get("fthr_frame_rate").as_deref(), Some("30/1"));
    assert_eq!(get("fthr_video_bitrate_bps").as_deref(), Some("300000"));
}

#[test]
fn audio_is_muxed_as_aac_and_progress_is_reported() {
    let Some(e) = encode_twelve_seconds() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("clip.mp4");
    let samples: Vec<f32> = (0..5 * 48_000)
        .flat_map(|i| {
            let v = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
            [v, v]
        })
        .collect();
    let audio = ClipAudio {
        samples: &samples,
        sample_rate: 48_000,
        channels: 2,
        title: "Default Mix",
    };
    let progress = std::cell::Cell::new(0u64);
    let result = write_clip(
        &out,
        &video(&e, false),
        &[audio],
        Some(&|bytes| progress.set(bytes)),
    )
    .unwrap();
    assert!(result.audio_included);
    assert!(result.audio_error.is_none());
    assert_eq!(
        progress.get(),
        result.bytes_written,
        "progress callback saw the final byte count"
    );
    let p = probe(&out);
    assert_eq!(p.streams, 2);
    let ictx = ffmpeg_next::format::input(&out).unwrap();
    let audio_stream = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Audio)
        .unwrap();
    assert_eq!(audio_stream.parameters().id(), ffmpeg_next::codec::Id::AAC);
}

#[test]
fn audio_failure_degrades_to_video_only_and_is_reported() {
    let Some(e) = encode_twelve_seconds() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("clip.mp4");
    let samples = vec![0.0f32; 4800];
    let bad = ClipAudio {
        samples: &samples,
        sample_rate: 0,
        channels: 2,
        title: "Default Mix",
    };
    let result = write_clip(&out, &video(&e, false), &[bad], None).unwrap();
    assert!(!result.audio_included);
    assert!(
        result.audio_error.is_some(),
        "the caller is told the clip is video-only"
    );
    assert_eq!(probe(&out).streams, 1);
}

#[test]
fn refuses_existing_partial_and_empty_snapshot() {
    let Some(e) = encode_twelve_seconds() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("clip.mp4");
    std::fs::write(temporary_path_for(&out), b"leftover").unwrap();
    let err = write_clip(&out, &video(&e, false), &[], None).unwrap_err();
    assert_eq!(err.failure, Failure::TemporaryExists);
    assert!(!out.exists());

    let empty = Snapshot {
        packets: Vec::new(),
        ..e.snapshot.clone()
    };
    let v = ClipVideo {
        snapshot: &empty,
        ..video(&e, false)
    };
    let out2 = tmp.path().join("empty.mp4");
    let err = write_clip(&out2, &v, &[], None).unwrap_err();
    assert_eq!(err.failure, Failure::Writer);
    assert!(err.to_string().contains("no video packets"), "{err}");
    assert!(!out2.exists() && !temporary_path_for(&out2).exists());
}

fn tone(seconds: u32, hz: f32) -> Vec<f32> {
    (0..seconds * 48_000)
        .flat_map(|i| {
            let v = (i as f32 * hz * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
            [v, v]
        })
        .collect()
}

#[test]
fn two_tracks_are_written_with_titles() {
    let Some(e) = encode_twelve_seconds() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("clip.mp4");
    let (a, b) = (tone(5, 440.0), tone(5, 660.0));
    let tracks = [
        ClipAudio {
            samples: &a,
            sample_rate: 48_000,
            channels: 2,
            title: "Default Mix",
        },
        ClipAudio {
            samples: &b,
            sample_rate: 48_000,
            channels: 2,
            title: "Microphone",
        },
    ];
    let result = write_clip(&out, &video(&e, true), &tracks, None).unwrap();
    assert!(result.audio_included);
    assert_eq!(result.audio_error, None);
    assert_eq!(result.audio_tracks, ["Default Mix", "Microphone"]);
    assert_eq!(probe(&out).streams, 3);
    let desc = fthrust_media::combined_audio::descriptors_from_file(&out).unwrap();
    assert_eq!(desc.len(), 2);
    // The MP4 muxer carries the label as the track handler name (the
    // combined-audio policy matches on either tag).
    assert_eq!(
        desc[0].handler_name.as_deref(),
        Some("Default Mix"),
        "{desc:?}"
    );
    assert_eq!(desc[1].handler_name.as_deref(), Some("Microphone"));
    assert_eq!((desc[0].container_index, desc[1].container_index), (1, 2));
}

#[test]
fn one_failing_track_is_skipped_and_named() {
    let Some(e) = encode_twelve_seconds() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("clip.mp4");
    let good = tone(2, 440.0);
    let tracks = [
        ClipAudio {
            samples: &good,
            sample_rate: 0,
            channels: 2,
            title: "Default Mix",
        },
        ClipAudio {
            samples: &good,
            sample_rate: 48_000,
            channels: 2,
            title: "Microphone",
        },
    ];
    let result = write_clip(&out, &video(&e, false), &tracks, None).unwrap();
    assert!(result.audio_included, "the good track was written");
    assert_eq!(result.audio_tracks, ["Microphone"]);
    let err = result.audio_error.expect("the bad track is reported");
    assert!(err.contains("Default Mix"), "{err}");
    assert_eq!(probe(&out).streams, 2);
    // An empty track is simply skipped, silently.
    let tracks = [ClipAudio {
        samples: &[],
        sample_rate: 48_000,
        channels: 2,
        title: "Microphone",
    }];
    let out2 = tmp.path().join("clip2.mp4");
    let result = write_clip(&out2, &video(&e, false), &tracks, None).unwrap();
    assert!(
        !result.audio_included && result.audio_error.is_none() && result.audio_tracks.is_empty()
    );
}
