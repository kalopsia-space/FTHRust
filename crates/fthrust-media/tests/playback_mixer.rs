//! Plan 6a Task 1: the playback audio mixer.

mod support;

use fthrust_media::playback::mixer::Mixer;
use fthrust_media::playback::{CANONICAL_SAMPLE_RATE, SourceSpec};
use support::{tone_power, write_tone_clip};

fn specs(indexes: &[usize]) -> Vec<SourceSpec> {
    indexes
        .iter()
        .enumerate()
        .map(|(i, idx)| SourceSpec {
            source_id: format!("track{i}"),
            container_index: *idx,
        })
        .collect()
}

#[test]
fn mixer_mixes_tracks_with_gains() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("tones.mp4");
    let Some(idx) = write_tone_clip(&clip, 3, &[("a", 440.0), ("b", 880.0)]) else {
        return;
    };
    let mut m = Mixer::open(&clip, &specs(&idx)).unwrap();
    assert_eq!(m.source_count(), 2);
    assert!(m.failed_sources().is_empty());
    let only_a = m.pull(4800, &[1.0, 0.0], 1.0).unwrap().unwrap();
    assert_eq!(only_a.len(), 4800 * 2);
    let rate = CANONICAL_SAMPLE_RATE as f32;
    assert!(tone_power(&only_a, 440.0, rate) > 50.0 * tone_power(&only_a, 880.0, rate));
    let full_b = m.pull(4800, &[0.0, 1.0], 1.0).unwrap().unwrap();
    let half_b = m.pull(4800, &[0.0, 1.0], 0.5).unwrap().unwrap();
    let pb = tone_power(&full_b, 880.0, rate);
    let ph = tone_power(&half_b, 880.0, rate);
    assert!(pb > 50.0 * tone_power(&full_b, 440.0, rate));
    assert!(
        (ph / pb - 0.25).abs() < 0.08,
        "half gain is a quarter of the power: {}",
        ph / pb
    );
}

#[test]
fn mixer_reports_missing_track_and_plays_the_rest() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("one.mp4");
    let Some(idx) = write_tone_clip(&clip, 2, &[("a", 440.0)]) else {
        return;
    };
    let mut s = specs(&[7, idx[0]]);
    s[0].source_id = "ghost".into();
    let mut m = Mixer::open(&clip, &s).unwrap();
    assert_eq!(m.failed_sources(), vec![0]);
    let block = m.pull(2048, &[1.0, 1.0], 1.0).unwrap().unwrap();
    assert!(block.iter().any(|v| v.abs() > 0.01));
}

#[test]
fn mixer_no_tracks_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("one.mp4");
    let Some(_) = write_tone_clip(&clip, 1, &[("a", 440.0)]) else {
        return;
    };
    let err = match Mixer::open(&clip, &specs(&[5, 9])) {
        Ok(_) => panic!("opened without any valid track"),
        Err(e) => e,
    };
    assert_eq!(err.to_string(), "This clip has no decodable audio stream.");
}

#[test]
fn mixer_seek_moves_position() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("seek.mp4");
    let Some(idx) = write_tone_clip(&clip, 4, &[("a", 440.0)]) else {
        return;
    };
    let mut m = Mixer::open(&clip, &specs(&idx)).unwrap();
    m.seek(2500).unwrap();
    assert!(
        (2400..=2600).contains(&m.position_ms()),
        "{}",
        m.position_ms()
    );
    let after_seek = m.pull(480, &[1.0], 1.0).unwrap().unwrap();
    let mut fresh = Mixer::open(&clip, &specs(&idx)).unwrap();
    fresh.seek(2500).unwrap();
    let again = fresh.pull(480, &[1.0], 1.0).unwrap().unwrap();
    assert_eq!(after_seek.len(), again.len());
    assert!(
        after_seek
            .iter()
            .zip(&again)
            .all(|(a, b)| (a - b).abs() < 1e-3)
    );
    assert!(
        (2500..=2520).contains(&m.position_ms()),
        "{}",
        m.position_ms()
    );
}

#[test]
fn mixer_rate_with_pitch_preserved_keeps_frequency() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("rate.mp4");
    let Some(idx) = write_tone_clip(&clip, 6, &[("a", 440.0)]) else {
        return;
    };
    let rate = CANONICAL_SAMPLE_RATE as f32;
    let mut m = Mixer::open(&clip, &specs(&idx)).unwrap();
    assert!(m.set_rate(2.0, true).unwrap());
    let p0 = m.position_ms();
    let block = m.pull(4800, &[1.0], 1.0).unwrap().unwrap();
    assert_eq!(block.len(), 4800 * 2);
    let advanced = m.position_ms() - p0;
    assert!(
        (160..=240).contains(&advanced),
        "2x consumes ~200 ms of source: {advanced}"
    );
    assert!(
        tone_power(&block, 440.0, rate) > 20.0 * tone_power(&block, 880.0, rate),
        "pitch kept"
    );
    assert!(m.set_rate(0.5, true).unwrap());
    let p1 = m.position_ms();
    let _ = m.pull(4800, &[1.0], 1.0).unwrap().unwrap();
    let advanced = m.position_ms() - p1;
    assert!(
        (30..=70).contains(&advanced),
        "0.5x consumes ~50 ms: {advanced}"
    );
    assert!(m.set_rate(3.0, true).unwrap());
    let p2 = m.position_ms();
    let _ = m.pull(4800, &[1.0], 1.0).unwrap().unwrap();
    let advanced = m.position_ms() - p2;
    assert!(
        (240..=360).contains(&advanced),
        "3x (two stages) consumes ~300 ms: {advanced}"
    );
}

#[test]
fn mixer_rate_without_pitch_returns_false() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("rate2.mp4");
    let Some(idx) = write_tone_clip(&clip, 2, &[("a", 440.0)]) else {
        return;
    };
    let mut m = Mixer::open(&clip, &specs(&idx)).unwrap();
    assert!(!m.set_rate(1.5, false).unwrap());
    assert!(
        !m.set_rate(1.0, true).unwrap(),
        "rate 1 never needs the chain"
    );
    let block = m.pull(1024, &[1.0], 1.0).unwrap().unwrap();
    assert_eq!(block.len(), 1024 * 2);
}

#[test]
fn mixer_eof_returns_none() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("eof.mp4");
    let Some(idx) = write_tone_clip(&clip, 1, &[("a", 440.0)]) else {
        return;
    };
    let mut m = Mixer::open(&clip, &specs(&idx)).unwrap();
    let mut total = 0usize;
    let mut saw_none = false;
    for _ in 0..200 {
        match m.pull(4800, &[1.0], 1.0).unwrap() {
            Some(b) => total += b.len() / 2,
            None => {
                saw_none = true;
                break;
            }
        }
    }
    assert!(saw_none, "EOF is reported");
    assert!(
        (40_000..=50_000).contains(&total),
        "about one second was delivered: {total}"
    );
    assert!(m.pull(10, &[1.0], 1.0).unwrap().is_none(), "stays at EOF");
}

/// Saves `path` through the engine's clip writer: `seconds` of video at
/// `fps` plus one sine track per `(seconds, hz, title)`. `None` when no
/// software H.264 encoder is available.
fn write_av_clip(
    path: &std::path::Path,
    fps: u32,
    seconds: u32,
    tones: &[(u64, f32, &'static str)],
) -> Option<fthrust_media::clip_writer::ClipWriteResult> {
    use fthrust_core::capture_config::{Codec, EncoderPref};
    use fthrust_media::clip_writer::{ClipAudio, ClipVideo, write_clip};
    use fthrust_media::encoder::{Encoder, EncoderConfig};
    use fthrust_media::ring::EncodedRing;
    support::skip_unless_encoder(&["libopenh264", "libx264"])?;
    const SECOND: i64 = 1_000_000_000;
    let cfg = EncoderConfig {
        src_width: 160,
        src_height: 90,
        enc_width: 160,
        enc_height: 90,
        fps,
        bitrate_kbps: 300,
        codec: Codec::H264,
        encoder: EncoderPref::Software,
        preset: 1,
    };
    let mut encoder = Encoder::open(&cfg).unwrap();
    let ring = EncodedRing::new(u64::from(seconds) * 1000 + 5000, fps, 0);
    let mut out = Vec::new();
    for i in 0..seconds * fps {
        let bgra = support::synthetic_bgra(160, 90, i);
        encoder
            .encode(
                &bgra,
                160 * 4,
                160,
                90,
                i64::from(i) * SECOND / i64::from(fps),
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
        separate_audio: true,
    };
    let samples: Vec<Vec<f32>> = tones
        .iter()
        .map(|&(secs, hz, _)| {
            (0..secs * 48_000)
                .flat_map(|i| {
                    let v = (i as f32 * hz * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
                    [v, v]
                })
                .collect()
        })
        .collect();
    let tracks: Vec<ClipAudio<'_>> = tones
        .iter()
        .zip(&samples)
        .map(|(&(_, _, title), samples)| ClipAudio {
            samples,
            sample_rate: 48_000,
            channels: 2,
            title,
        })
        .collect();
    Some(write_clip(path, &video, &tracks, None).unwrap())
}

/// A track that ends before the others (a short mic track) must not make a
/// seek or pull decode the rest of the file into the other tracks' buffers.
#[test]
fn short_track_does_not_unbound_the_others() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("uneven.mp4");
    let Some(result) = write_av_clip(&clip, 30, 8, &[(8, 440.0, "Game"), (1, 880.0, "Mic")]) else {
        return;
    };
    assert_eq!(result.audio_tracks.len(), 2, "{result:?}");
    let specs = [
        SourceSpec {
            source_id: "game".into(),
            container_index: 1,
        },
        SourceSpec {
            source_id: "mic".into(),
            container_index: 2,
        },
    ];
    let mut mixer = Mixer::open(&clip, &specs).unwrap();
    assert!(mixer.failed_sources().is_empty());
    mixer.seek(3000).unwrap();
    let block = mixer
        .pull(1024, &[1.0, 1.0], 1.0)
        .unwrap()
        .expect("audio after the short track ended");
    assert!(
        support::tone_power(&block, 440.0, 48_000.0)
            > 10.0 * support::tone_power(&block, 880.0, 48_000.0),
        "the long track still plays"
    );
    assert!(
        mixer.pending_frames() <= 2 * 48_000,
        "decoded ahead by {} frames; the short track must count as ended",
        mixer.pending_frames()
    );
}

/// Copies `src` to `dst` with every packet of each stream stored after
/// all of the previous stream's: the layout of clips saved before the clip
/// writer interleaved its audio.
fn remux_stream_after_stream(src: &std::path::Path, dst: &std::path::Path) {
    let mut ictx = ffmpeg_next::format::input(src).unwrap();
    let mut octx = ffmpeg_next::format::output(dst).unwrap();
    let mut time_bases = Vec::new();
    for stream in ictx.streams() {
        let mut out = octx
            .add_stream(ffmpeg_next::encoder::find(ffmpeg_next::codec::Id::None))
            .unwrap();
        out.set_parameters(stream.parameters());
        time_bases.push(stream.time_base());
    }
    octx.write_header().unwrap();
    let mut by_stream: Vec<Vec<ffmpeg_next::Packet>> = vec![Vec::new(); time_bases.len()];
    for (stream, packet) in ictx.packets() {
        by_stream[stream.index()].push(packet);
    }
    for (index, packets) in by_stream.into_iter().enumerate() {
        let out_tb = octx.stream(index).unwrap().time_base();
        for mut packet in packets {
            packet.rescale_ts(time_bases[index], out_tb);
            packet.set_stream(index);
            packet.set_position(-1);
            packet.write(&mut octx).unwrap();
        }
    }
    octx.write_trailer().unwrap();
}

/// A clip longer than the muxer's interleave window (10 s) plays its audio
/// from the first pull, even stored with all its audio after all its video
/// (as clips were saved before the writer interleaved them). It played
/// silent: the audio track counted as ended before its first packet was
/// demuxed.
#[test]
fn stream_after_stream_clip_plays_audio_from_the_start() {
    let d = tempfile::tempdir().unwrap();
    let saved = d.path().join("long.mp4");
    let Some(result) = write_av_clip(&saved, 60, 12, &[(12, 440.0, "Game")]) else {
        return;
    };
    assert_eq!(result.audio_tracks.len(), 1, "{result:?}");
    let clip = d.path().join("flat.mp4");
    remux_stream_after_stream(&saved, &clip);
    let specs = [SourceSpec {
        source_id: "game".into(),
        container_index: 1,
    }];
    let mut mixer = Mixer::open(&clip, &specs).unwrap();
    let block = mixer
        .pull(4800, &[1.0], 1.0)
        .unwrap()
        .expect("audio at the start of the clip");
    assert!(
        support::tone_power(&block, 440.0, 48_000.0) > 0.01,
        "the tone is audible"
    );
}
