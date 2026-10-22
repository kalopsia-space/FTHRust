use fthrust_media::audio_timing::{
    AudioBlock, CHANNELS, FRAMES_PER_CHUNK, GAP_FILL_THRESHOLD_NS, SAMPLE_RATE, assemble_window,
    chunk_start_ns, ring_capacity_samples,
};

const MS: i64 = 1_000_000;

fn block(value: f32, frames: usize, start_ns: i64) -> AudioBlock {
    AudioBlock {
        start_ns,
        samples: vec![value; frames],
    }
}

#[test]
fn constants_match_reference() {
    assert_eq!(SAMPLE_RATE, 48_000);
    assert_eq!(CHANNELS, 2);
    assert_eq!(FRAMES_PER_CHUNK, 480);
    assert_eq!(GAP_FILL_THRESHOLD_NS, 50 * MS);
}

#[test]
fn chunk_start_subtracts_latency_and_length() {
    assert_eq!(
        chunk_start_ns(1_050_000_000, 40_000, 480, 48_000),
        1_000_000_000
    );
}

#[test]
fn negative_latency_is_zero() {
    assert_eq!(
        chunk_start_ns(2_000_000_000, -1, 480, 48_000),
        1_990_000_000
    );
    assert_eq!(
        chunk_start_ns(5, 10, 480, 0),
        5,
        "zero rate returns delivery time"
    );
}

#[test]
fn contiguous_jittered_blocks_join() {
    let blocks = [
        block(1.0, 100, 0),
        block(2.0, 100, 103 * MS),
        block(3.0, 100, 198 * MS),
    ];
    let pcm = assemble_window(&blocks, 1, 1000, 300 * MS, 300);
    assert_eq!(pcm.len(), 300);
    assert_eq!((pcm[99], pcm[100], pcm[200]), (1.0, 2.0, 3.0));
}

#[test]
fn gap_over_50ms_gets_silence_and_later_audio_stays_at_capture_time() {
    let blocks = [block(1.0, 100, 0), block(2.0, 100, 400 * MS)];
    let pcm = assemble_window(&blocks, 1, 1000, 500 * MS, 500);
    assert_eq!(pcm.len(), 500);
    assert_eq!(pcm[99], 1.0);
    assert_eq!((pcm[100], pcm[399]), (0.0, 0.0));
    assert_eq!((pcm[400], pcm[499]), (2.0, 2.0));
}

#[test]
fn late_audio_start_pads_leading_silence() {
    let blocks = [block(1.0, 200, 300 * MS)];
    let pcm = assemble_window(&blocks, 1, 1000, 500 * MS, 500);
    assert_eq!(pcm.len(), 500);
    assert_eq!((pcm[0], pcm[299], pcm[300]), (0.0, 0.0, 1.0));
}

#[test]
fn straddling_block_trimmed_keeps_interleave() {
    let samples: Vec<f32> = (0..400).map(|i| i as f32).collect();
    let blocks = [AudioBlock {
        start_ns: 0,
        samples,
    }];
    let pcm = assemble_window(&blocks, 2, 1000, 200 * MS, 150);
    assert_eq!(pcm.len(), 300);
    assert_eq!((pcm[0], pcm[1]), (100.0, 101.0), "frame 50, both channels");
}

#[test]
fn no_overlap_is_empty() {
    let blocks = [block(1.0, 100, 0)];
    assert!(assemble_window(&blocks, 1, 1000, 900 * MS, 100).is_empty());
    assert!(assemble_window(&[], 1, 1000, 900 * MS, 100).is_empty());
    assert!(assemble_window(&blocks, 0, 1000, 100 * MS, 100).is_empty());
    assert!(assemble_window(&blocks, 1, 1000, 100 * MS, 0).is_empty());
}

#[test]
fn ring_capacity_formula() {
    assert_eq!(ring_capacity_samples(30, 48_000, 2), 40 * 48_000 * 2);
    assert_eq!(ring_capacity_samples(300, 48_000, 2), 310 * 48_000 * 2);
    assert_eq!(ring_capacity_samples(0, 48_000, 2), 10 * 48_000 * 2);
}
