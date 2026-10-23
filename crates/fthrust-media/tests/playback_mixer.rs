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
