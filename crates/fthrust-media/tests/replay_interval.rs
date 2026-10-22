use fthrust_media::replay_interval::{
    Sample, SelectError, Selection, presentation_start_pts, presentation_start_pts_seconds, select,
};

const SECOND: i64 = 1_000_000_000;

fn constant_rate(fps: i64, seconds: i64, keyframe_interval: i64, epoch: i64) -> Vec<Sample> {
    (0..=fps * seconds)
        .map(|frame| Sample {
            timestamp: epoch + frame * SECOND / fps,
            keyframe: frame % keyframe_interval == 0,
        })
        .collect()
}

fn assert_full_window(
    samples: &[Sample],
    target_end: i64,
    duration: i64,
    tolerance: i64,
) -> Selection {
    let s = select(samples, target_end, duration).expect("valid selection");
    assert!(s.full_history);
    assert!(samples[s.decode_start].keyframe);
    assert!(samples[s.decode_start].timestamp <= s.requested_start);
    assert!(samples[s.presentation_start].timestamp >= s.requested_start);
    assert!(samples[s.presentation_start].timestamp - s.requested_start <= tolerance);
    assert!(s.media_end <= target_end);
    assert!(target_end - s.media_end <= tolerance);
    s
}

#[test]
fn exact_window_normal_rate() {
    for fps in [30, 60, 120] {
        let samples = constant_rate(fps, 120, fps * 2, 0);
        assert_full_window(&samples, 60 * SECOND, 30 * SECOND, SECOND / fps);
    }
    let long = constant_rate(60, 120, 120, 0);
    assert_full_window(&long, 120 * SECOND, 60 * SECOND, SECOND / 60);
    assert_full_window(&long, 77 * SECOND, 17 * SECOND, SECOND / 60);
}

#[test]
fn decode_starts_at_keyframe_before_window() {
    let fps = 60;
    let long_gop = constant_rate(fps, 70, fps * 4, 0);
    let exact = select(&long_gop, 60 * SECOND, 28 * SECOND).unwrap();
    assert_eq!(long_gop[exact.decode_start].timestamp, 32 * SECOND);
    let after = select(&long_gop, 60 * SECOND, 28 * SECOND - SECOND / fps).unwrap();
    assert_eq!(long_gop[after.decode_start].timestamp, 32 * SECOND);
    assert!(long_gop[after.decode_start].timestamp < after.requested_start);
    let before_next = select(&long_gop, 60 * SECOND, 24 * SECOND + SECOND / fps).unwrap();
    assert_eq!(long_gop[before_next.decode_start].timestamp, 32 * SECOND);
    assert!(long_gop[before_next.decode_start].timestamp < before_next.requested_start);
    let short_gop = constant_rate(fps, 70, fps / 2, 0);
    assert_full_window(&short_gop, 60 * SECOND, 30 * SECOND, SECOND / fps);
}

#[test]
fn insufficient_history_is_partial_from_first_keyframe() {
    let five = constant_rate(60, 5, 120, 0);
    let partial = select(&five, 5 * SECOND, 30 * SECOND).unwrap();
    assert!(!partial.full_history);
    assert_eq!(partial.decode_start, partial.presentation_start);
    assert!(five[partial.decode_start].keyframe);

    let startup = constant_rate(60, 1, 120, 0);
    let early = select(&startup, SECOND, 30 * SECOND).unwrap();
    assert!(!early.full_history);

    let post_recovery = constant_rate(60, 5, 120, 100 * SECOND);
    let recovered = select(&post_recovery, 105 * SECOND, 30 * SECOND).unwrap();
    assert!(!recovered.full_history);
    assert_eq!(recovered.requested_start, 75 * SECOND);
    assert!(post_recovery[recovered.decode_start].timestamp >= 100 * SECOND);
}

#[test]
fn irregular_timestamps_and_nonzero_epoch() {
    let mut variable = Vec::new();
    let mut timestamp = 50 * SECOND;
    for i in 0..2400 {
        timestamp += if i % 11 == 0 {
            SECOND / 30
        } else {
            SECOND / 60
        };
        variable.push(Sample {
            timestamp,
            keyframe: i % 120 == 0,
        });
    }
    let target = variable.last().unwrap().timestamp;
    assert_full_window(&variable, target, 30 * SECOND, SECOND / 30);

    let mut dropped = constant_rate(60, 65, 120, 200 * SECOND);
    dropped.remove(59 * 60 + 17);
    assert_full_window(&dropped, 260 * SECOND, 30 * SECOND, 2 * SECOND / 60);

    let delayed = constant_rate(60, 61, 120, 300 * SECOND);
    let s = select(&delayed, 360 * SECOND, 30 * SECOND).unwrap();
    assert_eq!(s.media_end, 360 * SECOND);
    assert!(delayed[s.end + 1].timestamp > 360 * SECOND);
}

#[test]
fn ring_and_save_boundaries() {
    let exact = constant_rate(60, 30, 120, 0);
    let s = select(&exact, 30 * SECOND, 30 * SECOND).unwrap();
    assert!(s.full_history);
    let slightly_more = constant_rate(60, 31, 120, 0);
    assert_full_window(&slightly_more, 31 * SECOND, 30 * SECOND, SECOND / 60);
    let wrapped = constant_rate(60, 40, 120, 500 * SECOND);
    assert_full_window(&wrapped, 540 * SECOND, 30 * SECOND, SECOND / 60);
    let mut arriving = constant_rate(60, 31, 120, 0);
    arriving.push(Sample {
        timestamp: 31 * SECOND + SECOND / 60,
        keyframe: false,
    });
    let bounded = select(&arriving, 31 * SECOND, 30 * SECOND).unwrap();
    assert!(bounded.end + 1 < arriving.len());
    assert_eq!(bounded.media_end, 31 * SECOND);
}

#[test]
fn rapid_saves_do_not_mutate_input() {
    let samples = constant_rate(60, 65, 120, 0);
    let original = samples.clone();
    let first = select(&samples, 60 * SECOND, 30 * SECOND).unwrap();
    let second = select(&samples, 61 * SECOND, 30 * SECOND).unwrap();
    assert!(first.end < second.end);
    assert_eq!(samples, original);
}

#[test]
fn invalid_inputs_are_errors() {
    assert_eq!(select(&[], 10, 5).unwrap_err(), SelectError::EmptyInput);
    let no_keyframe = [
        Sample {
            timestamp: 0,
            keyframe: false,
        },
        Sample {
            timestamp: 1,
            keyframe: false,
        },
        Sample {
            timestamp: 2,
            keyframe: false,
        },
    ];
    assert_eq!(
        select(&no_keyframe, 2, 1).unwrap_err(),
        SelectError::MissingStartKeyframe
    );
    let one = [Sample {
        timestamp: 10,
        keyframe: true,
    }];
    assert_eq!(
        select(&one, 5, 1).unwrap_err(),
        SelectError::NoPacketAtTarget
    );
    assert_eq!(select(&one, 10, 0).unwrap_err(), SelectError::EmptyInput);
}

#[test]
fn presentation_offset_math() {
    let offset = presentation_start_pts_seconds(385, 30, 5, true, 140);
    assert_eq!(offset, 236);
    assert_eq!((385 - offset + 1) as f64 / 30.0, 5.0);
    assert_eq!(presentation_start_pts_seconds(25, 30, 30, false, 7), 7);
    assert_eq!(
        presentation_start_pts(100, 0, true, 42),
        42,
        "non-positive ticks keep decode start"
    );
    assert_eq!(presentation_start_pts(100, 30, true, 42), 71);
}
