use std::sync::Arc;
use std::time::Duration;

use fthrust_media::ring::{EncodedPacket, EncodedRing};

const SECOND: i64 = 1_000_000_000;

fn packet(pts: i64, wall_time_ns: i64, keyframe: bool) -> EncodedPacket {
    EncodedPacket {
        data: vec![0x01].into(),
        pts,
        dts: pts,
        keyframe,
        wall_time_ns,
        generation: 0,
    }
}

fn sized(pts: i64, wall_time_ns: i64, keyframe: bool, bytes: usize) -> EncodedPacket {
    EncodedPacket {
        data: vec![0x01; bytes].into(),
        ..packet(pts, wall_time_ns, keyframe)
    }
}

#[test]
fn full_window_uses_keyframe_before_window() {
    let fps = 60;
    let ring = EncodedRing::new(40_000, fps, 0);
    for frame in 0..=60 * fps as i64 {
        ring.push(packet(
            frame,
            frame * SECOND / fps as i64,
            frame % (fps as i64 * 2) == 0,
        ));
    }
    let s = ring.snapshot(30_000, 60 * SECOND, 0).unwrap();
    assert!(s.full_history);
    assert!(s.packets[0].keyframe);
    assert!(s.packets[0].wall_time_ns <= 30 * SECOND);
    assert_eq!(s.presentation_start_pts, 30 * fps as i64 + 1);
    assert_eq!(
        s.packets.last().unwrap().pts - s.presentation_start_pts + 1,
        30 * fps as i64
    );
    assert_eq!(s.fps, fps);
}

#[test]
fn variable_timing_and_delayed_publication() {
    let fps = 60;
    let ring = EncodedRing::new(70_000, fps, 0);
    let mut wall = 0;
    for frame in 0..3000 {
        wall += if frame % 13 == 0 {
            SECOND / 30
        } else {
            SECOND / 60
        };
        let pts = wall * fps as i64 / SECOND;
        ring.push(packet(pts, wall, frame % (fps as i64 * 2) == 0));
    }
    let target = wall - SECOND;
    let s = ring.snapshot(30_000, target, 0).unwrap();
    assert!(s.full_history);
    assert!(s.packets.last().unwrap().wall_time_ns <= target);
    assert_eq!(s.presentation_end_ns, target);
    assert_eq!(
        s.packets.last().unwrap().pts - s.presentation_start_pts + 1,
        30 * fps as i64
    );
    assert!(
        ring.wait_until_published(target, Duration::from_millis(1)),
        "already published"
    );
    assert!(
        !ring.wait_until_published(wall + SECOND, Duration::from_millis(20)),
        "never published"
    );
    assert!(
        ring.wait_until_published(0, Duration::from_millis(1)),
        "non-positive target is immediate"
    );
}

#[test]
fn wait_until_published_wakes_on_push() {
    let ring = std::sync::Arc::new(EncodedRing::new(10_000, 30, 0));
    let waiter = {
        let ring = ring.clone();
        std::thread::spawn(move || ring.wait_until_published(5 * SECOND, Duration::from_secs(5)))
    };
    std::thread::sleep(Duration::from_millis(30));
    ring.push(packet(0, 5 * SECOND, true));
    assert!(waiter.join().unwrap());
}

#[test]
fn clear_drops_old_generation() {
    let fps = 30;
    let ring = EncodedRing::new(40_000, fps, 0);
    for frame in 0..=30 * fps as i64 {
        ring.push(packet(frame, frame * SECOND / fps as i64, frame % 60 == 0));
    }
    ring.clear();
    for frame in 0..=5 * fps as i64 {
        ring.push(packet(
            frame,
            100 * SECOND + frame * SECOND / fps as i64,
            frame % 60 == 0,
        ));
    }
    let s = ring.snapshot(30_000, 105 * SECOND, 0).unwrap();
    assert!(!s.full_history);
    assert!(s.packets[0].wall_time_ns >= 100 * SECOND);
    assert_eq!(s.presentation_start_pts, s.packets[0].pts);
}

#[test]
fn snapshot_filters_by_generation() {
    let fps = 30;
    let ring = EncodedRing::new(40_000, fps, 0);
    for frame in 0..=10 * fps as i64 {
        let mut p = packet(frame, frame * SECOND / fps as i64, frame % 60 == 0);
        p.generation = 1;
        ring.push(p);
    }
    for frame in 0..=3 * fps as i64 {
        let mut p = packet(
            frame,
            10 * SECOND + frame * SECOND / fps as i64,
            frame % 60 == 0,
        );
        p.generation = 2;
        ring.push(p);
    }
    let newer = ring.snapshot(30_000, 13 * SECOND, 2).unwrap();
    assert!(!newer.full_history);
    assert_eq!(newer.packets[0].generation, 2);
    assert!(newer.packets[0].wall_time_ns >= 10 * SECOND);
    assert!(
        ring.snapshot(30_000, 13 * SECOND, 7).is_none(),
        "unknown generation yields nothing"
    );
    let all = ring.snapshot(30_000, 13 * SECOND, 0).unwrap();
    assert_eq!(all.packets[0].generation, 1);
}

#[test]
fn byte_budget_drops_whole_gops_from_front() {
    let fps = 30;
    let gop = 60i64;
    let key_bytes = 50_000usize;
    let delta_bytes = 5_000usize;
    let gop_bytes = key_bytes + (gop as usize - 1) * delta_bytes;
    let budget = (10 * gop_bytes + gop_bytes / 2) as u64;
    let ring = EncodedRing::new(905_000, fps, budget);
    let mut pushed = 0u64;
    for frame in 0..60 * fps as i64 {
        let key = frame % gop == 0;
        let bytes = if key { key_bytes } else { delta_bytes };
        ring.push(sized(frame, frame * SECOND / fps as i64, key, bytes));
        pushed += bytes as u64;
        let stats = ring.stats();
        if frame >= gop {
            assert!(stats.bytes <= budget, "frame {frame}");
        }
        assert!(stats.packets > 0);
    }
    assert!(pushed > budget);
    let stats = ring.stats();
    assert!(stats.budget_trims > 0);
    assert!(stats.bytes <= budget);
    assert!(stats.bytes >= (10 * gop_bytes) as u64);
    assert!(stats.span_ns < 22 * SECOND);
    let s = ring.snapshot(300_000, 0, 0).unwrap();
    assert!(s.packets[0].keyframe);
    assert!(!s.full_history);
    assert_eq!(s.packets.last().unwrap().pts, 60 * fps as i64 - 1);
    assert_eq!(s.presentation_start_pts, s.packets[0].pts);
}

#[test]
fn never_drops_current_gop() {
    let fps = 30;
    let ring = EncodedRing::new(905_000, fps, 1000);
    for frame in 0..90i64 {
        ring.push(sized(frame, frame * SECOND / fps as i64, frame == 0, 100));
    }
    assert_eq!(ring.stats().packets, 90);
    assert_eq!(ring.stats().budget_trims, 0);
    ring.push(sized(90, 90 * SECOND / fps as i64, true, 100));
    let stats = ring.stats();
    assert_eq!(
        (stats.packets, stats.bytes, stats.budget_trims),
        (1, 100, 1)
    );
    let s = ring.snapshot(30_000, 0, 0).unwrap();
    assert_eq!(s.packets.len(), 1);
    assert!(s.packets[0].keyframe);
}

#[test]
fn no_budget_and_clear_reset_counters() {
    let fps = 30;
    let unbounded = EncodedRing::new(905_000, fps, 0);
    for frame in 0..600i64 {
        unbounded.push(sized(
            frame,
            frame * SECOND / fps as i64,
            frame % 60 == 0,
            10_000,
        ));
    }
    assert_eq!(unbounded.stats().bytes, 600 * 10_000);
    assert_eq!(unbounded.stats().budget_trims, 0);
    let timed = EncodedRing::new(1_000, fps, 0);
    for frame in 0..300i64 {
        timed.push(sized(
            frame,
            frame * SECOND / fps as i64,
            frame % 60 == 0,
            7,
        ));
    }
    let stats = timed.stats();
    assert_eq!(stats.bytes, stats.packets as u64 * 7);
    unbounded.clear();
    let stats = unbounded.stats();
    assert_eq!(
        (
            stats.bytes,
            stats.packets,
            stats.span_ns,
            stats.latest_wall_time_ns
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn long_gap_then_save_is_partial_history() {
    // Capture paused for minutes, then a 30 s save: the ring holds only the
    // post-pause packets, so the snapshot starts at the first keyframe and
    // says so instead of returning nothing.
    let fps = 30;
    let ring = EncodedRing::new(35_000, fps, 0);
    for frame in 0..=10 * fps as i64 {
        ring.push(packet(frame, frame * SECOND / fps as i64, frame % 60 == 0));
    }
    let resume = 600 * SECOND;
    for frame in 0..=4 * fps as i64 {
        ring.push(packet(
            400 + frame,
            resume + frame * SECOND / fps as i64,
            frame % 60 == 0,
        ));
    }
    let s = ring.snapshot(30_000, resume + 4 * SECOND, 0).unwrap();
    assert!(!s.full_history);
    assert!(s.packets[0].keyframe);
    assert_eq!(
        s.packets[0].wall_time_ns, resume,
        "pre-pause packets were time-trimmed"
    );
    assert_eq!(s.presentation_start_ns, resume);
    assert_eq!(s.media_end_ns, resume + 4 * SECOND);
    assert!(
        ring.snapshot(0, resume, 0).is_none(),
        "zero duration is nothing"
    );
}

#[test]
fn snapshot_shares_payloads_with_the_ring() {
    let ring = EncodedRing::new(10_000, 30, 0);
    let p = sized(0, 0, true, 1_000);
    let data = p.data.clone();
    ring.push(p);
    let snap = ring.snapshot(1_000, 0, 0).unwrap();
    assert!(Arc::ptr_eq(&snap.packets[0].data, &data));
}

#[test]
fn snapshot_survives_the_ring_trimming_its_packets() {
    let ring = EncodedRing::new(1_000, 30, 0);
    for i in 0..30 {
        ring.push(sized(i, i * 33_333_333, i % 10 == 0, 100));
    }
    let snap = ring.snapshot(1_000, 0, 0).unwrap();
    for i in 30..120 {
        ring.push(sized(i, i * 33_333_333, i % 10 == 0, 100));
    }
    assert!(snap.packets.iter().all(|p| p.data.len() == 100));
}
