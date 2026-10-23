//! Plan 6a Task 2: the playback video decoder.

mod support;

use fthrust_media::playback::video::{VideoDecoder, frame_at};
use support::write_tone_clip;

/// The generator's green channel is `(y + 3·n) & 0xff`: its mean over the
/// top 90 rows is `44.5 + 3·n`, which survives lossy encoding when averaged
/// over the rows while adjacent frames still differ by 3.
fn mean_green_top(frame: &fthrust_media::playback::video::Frame) -> f64 {
    let w = frame.width as usize;
    let mut sum = 0u64;
    let mut n = 0u64;
    for y in 0..90usize {
        for x in 0..w {
            sum += u64::from(frame.rgba[(y * w + x) * 4 + 1]);
            n += 1;
        }
    }
    sum as f64 / n as f64
}

/// Consecutive frames differ by 3 in the generator; after a lossy
/// encode/decode round trip the mean still moves by about that much.
fn assert_step(prev: f64, next: f64) {
    let step = next - prev;
    assert!(
        (2.0..=4.0).contains(&step),
        "mean green step {step:.2} (prev {prev:.2}, next {next:.2})"
    );
}

#[test]
fn decoder_reports_info_and_frames_in_order() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("v.mp4");
    let Some(_) = write_tone_clip(&clip, 2, &[("a", 440.0)]) else {
        return;
    };
    let mut dec = VideoDecoder::open(&clip).unwrap();
    let info = dec.info().clone();
    assert_eq!((info.width, info.height), (320, 180));
    assert!((info.fps - 30.0).abs() < 0.5, "{}", info.fps);
    assert!(
        (1960..=2040).contains(&info.duration_ms),
        "{}",
        info.duration_ms
    );
    let mut last: Option<(u64, f64)> = None;
    for _ in 0..10u32 {
        let f = dec.next_frame().unwrap().expect("frame");
        let green = mean_green_top(&f);
        if let Some((prev_pts, prev_green)) = last {
            assert!(f.pts_ms > prev_pts, "increasing pts");
            assert!(
                (f.pts_ms - prev_pts) >= 30 && (f.pts_ms - prev_pts) <= 36,
                "step {}",
                f.pts_ms - prev_pts
            );
            assert_step(prev_green, green);
        } else {
            assert_eq!(f.pts_ms, 0);
        }
        last = Some((f.pts_ms, green));
    }
}

#[test]
fn seek_lands_on_the_requested_frame() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("v.mp4");
    let Some(_) = write_tone_clip(&clip, 2, &[("a", 440.0)]) else {
        return;
    };
    // Frame 30 decoded sequentially is the yardstick for the seek.
    let mut seq = VideoDecoder::open(&clip).unwrap();
    let mut thirtieth = None;
    for _ in 0..=30 {
        thirtieth = seq.next_frame().unwrap();
    }
    let yardstick = mean_green_top(&thirtieth.unwrap());
    let mut dec = VideoDecoder::open(&clip).unwrap();
    dec.seek(1000).unwrap();
    let f = dec.next_frame().unwrap().unwrap();
    assert!((1000..=1034).contains(&f.pts_ms), "{}", f.pts_ms);
    assert!(
        (mean_green_top(&f) - yardstick).abs() < 1.0,
        "seek landed on frame 30"
    );
    let _ = dec.next_frame().unwrap().unwrap();
    let _ = dec.next_frame().unwrap().unwrap();
    dec.seek(1000).unwrap();
    let again = dec.next_frame().unwrap().unwrap();
    assert_eq!(again.pts_ms, f.pts_ms, "seek is idempotent");
    dec.seek(5000).unwrap();
    let last = dec.next_frame().unwrap().expect("the last frame");
    assert!(last.pts_ms >= 1900, "{}", last.pts_ms);
    assert!(dec.next_frame().unwrap().is_none(), "then EOF");
}

#[test]
fn frame_at_matches_decoder() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("v.mp4");
    let Some(_) = write_tone_clip(&clip, 2, &[("a", 440.0)]) else {
        return;
    };
    let one = frame_at(&clip, 1500).unwrap();
    let mut dec = VideoDecoder::open(&clip).unwrap();
    dec.seek(1500).unwrap();
    let f = dec.next_frame().unwrap().unwrap();
    assert_eq!(one.pts_ms, f.pts_ms);
    assert_eq!(one.rgba, f.rgba);
}
