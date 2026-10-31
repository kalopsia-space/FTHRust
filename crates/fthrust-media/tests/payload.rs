use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use fthrust_media::payload::{Payload, SegmentStore};
use fthrust_media::ring::{EncodedPacket, EncodedRing};

const SECOND: i64 = 1_000_000_000;
/// Longest a test waits for the disk writer.
const WRITES: Duration = Duration::from_secs(10);

fn packet(frame: i64, fps: i64, bytes: usize) -> EncodedPacket {
    EncodedPacket {
        data: vec![(frame % 251) as u8; bytes].into(),
        pts: frame,
        dts: frame,
        keyframe: frame % fps == 0,
        wall_time_ns: frame * SECOND / fps,
        generation: 0,
    }
}

fn segments(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    v
}

fn disk_ring(dir: &std::path::Path, window_ms: u64, segment_bytes: u64) -> EncodedRing {
    let store = SegmentStore::new(dir.to_path_buf(), segment_bytes).unwrap();
    EncodedRing::new_on_disk(window_ms, 30, 0, store)
}

#[test]
fn disk_ring_snapshot_reads_back_the_same_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let ring = disk_ring(dir.path(), 10_000, 4096);
    for f in 0..90 {
        ring.push(packet(f, 30, 1000));
    }
    assert!(ring.wait_disk_writes(WRITES));
    let snap = ring.snapshot(2_000, 0, 0).unwrap();
    assert!(
        snap.packets
            .iter()
            .all(|p| matches!(p.data, Payload::Disk(_)))
    );
    let mut scratch = Vec::new();
    for p in &snap.packets {
        let expected = vec![(p.pts % 251) as u8; 1000];
        p.data
            .with_bytes(&mut scratch, |b| assert_eq!(b, &expected[..]))
            .unwrap();
        assert_eq!(&*p.data.bytes().unwrap(), &expected[..]);
    }
    assert!(segments(dir.path()).len() > 1, "segments rotate");
}

#[test]
fn trimmed_segments_are_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let ring = disk_ring(dir.path(), 1_000, 4096);
    for f in 0..30 {
        ring.push(packet(f, 30, 1000));
    }
    assert!(ring.wait_disk_writes(WRITES));
    let first = segments(dir.path())[0].clone();
    for f in 30..300 {
        ring.push(packet(f, 30, 1000));
    }
    assert!(ring.wait_disk_writes(WRITES));
    assert!(!first.exists(), "the first segment was trimmed away");
    // About 1 s of 30 kB/s in 4 kB segments, plus the current one.
    assert!(
        segments(dir.path()).len() <= 12,
        "{:?}",
        segments(dir.path())
    );
}

#[test]
fn snapshot_keeps_segments_alive_after_trim() {
    let dir = tempfile::tempdir().unwrap();
    let ring = disk_ring(dir.path(), 1_000, 4096);
    for f in 0..30 {
        ring.push(packet(f, 30, 1000));
    }
    assert!(ring.wait_disk_writes(WRITES));
    let snap = ring.snapshot(1_000, 0, 0).unwrap();
    for f in 30..300 {
        ring.push(packet(f, 30, 1000));
    }
    assert!(ring.wait_disk_writes(WRITES));
    let path = snap.packets[0].data.segment_path().unwrap().to_path_buf();
    assert!(path.exists());
    assert_eq!(snap.packets[0].data.bytes().unwrap().len(), 1000);
    drop(snap);
    assert!(!path.exists(), "the file goes with the last reference");
}

#[test]
fn segment_files_are_private() {
    let dir = tempfile::tempdir().unwrap();
    let ring = disk_ring(dir.path(), 1_000, 4096);
    ring.push(packet(0, 30, 10));
    assert!(ring.wait_disk_writes(WRITES));
    assert!(!segments(dir.path()).is_empty());
    for path in segments(dir.path()) {
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", path.display());
    }
}

#[test]
fn store_failure_falls_back_to_memory() {
    let dir = tempfile::tempdir().unwrap();
    let replay = dir.path().join("replay");
    std::fs::create_dir(&replay).unwrap();
    let ring = disk_ring(&replay, 10_000, 4096);
    std::fs::remove_dir(&replay).unwrap();
    for f in 0..60 {
        ring.push(packet(f, 30, 1000));
    }
    assert!(ring.wait_disk_writes(WRITES));
    let snap = ring.snapshot(2_000, 0, 0).unwrap();
    assert_eq!(snap.packets.len(), 60);
    assert!(
        snap.packets
            .iter()
            .all(|p| matches!(p.data, Payload::Mem(_)))
    );
}

#[test]
fn clear_keeps_the_disk_store() {
    // An encoder reopen or a geometry change clears the ring; the replay
    // buffer must stay on disk afterwards instead of quietly moving to RAM.
    let dir = tempfile::tempdir().unwrap();
    let ring = disk_ring(dir.path(), 10_000, 4096);
    for f in 0..30 {
        ring.push(packet(f, 30, 1000));
    }
    assert!(ring.wait_disk_writes(WRITES));
    ring.clear();
    assert_eq!(ring.stats().packets, 0);
    for f in 0..60 {
        ring.push(packet(f, 30, 1000));
    }
    assert!(ring.wait_disk_writes(WRITES));
    let snap = ring.snapshot(2_000, 0, 0).unwrap();
    assert_eq!(snap.packets.len(), 60);
    assert!(
        snap.packets
            .iter()
            .all(|p| matches!(p.data, Payload::Disk(_))),
        "payloads after the clear stay on disk"
    );
}

#[test]
fn a_slow_disk_never_stalls_push() {
    // The encode thread pushes every packet; a disk busy with a save to the
    // same drive must not hold it up. Writes happen on the ring's writer
    // thread, and the payloads reach the disk afterwards.
    let dir = tempfile::tempdir().unwrap();
    let store = SegmentStore::new(dir.path().to_path_buf(), 4096)
        .unwrap()
        .with_write_delay(Duration::from_millis(40));
    let ring = EncodedRing::new_on_disk(10_000, 30, 0, store);
    let started = Instant::now();
    for f in 0..20 {
        ring.push(packet(f, 30, 1000));
    }
    let pushing = started.elapsed();
    assert!(
        pushing < Duration::from_millis(400),
        "20 pushes took {pushing:?} against a disk that needs 800 ms for them"
    );
    let snap = ring.snapshot(1_000, 0, 0).unwrap();
    assert_eq!(snap.packets.len(), 20, "every packet is readable at once");
    assert!(ring.wait_disk_writes(WRITES));
    let snap = ring.snapshot(1_000, 0, 0).unwrap();
    assert!(
        snap.packets
            .iter()
            .all(|p| matches!(p.data, Payload::Disk(_)))
    );
    let mut scratch = Vec::new();
    for p in &snap.packets {
        let expected = vec![(p.pts % 251) as u8; 1000];
        p.data
            .with_bytes(&mut scratch, |b| assert_eq!(b, &expected[..]))
            .unwrap();
    }
}

#[test]
fn a_stalled_disk_queues_no_more_than_the_budget() {
    // While one write hangs (a hung network mount, a disk in error
    // recovery), every later payload queued for the writer: about 0.75 GB a
    // minute at 100 Mbps, past the ring's own byte budget. Now the queue
    // stops at the budget and newer packets stay in memory, which the
    // budget bounds.
    let dir = tempfile::tempdir().unwrap();
    let store = SegmentStore::new(dir.path().to_path_buf(), 1 << 20)
        .unwrap()
        .with_write_delay(Duration::from_millis(300));
    let budget = 20_000;
    let ring = EncodedRing::new_on_disk(10_000, 30, budget, store);
    for f in 0..200 {
        ring.push(packet(f, 30, 1000));
    }
    let queued = ring.disk_queue_bytes();
    assert!(
        queued <= budget,
        "{queued} bytes queued behind a stalled disk with a {budget}-byte budget"
    );
    let snap = ring.snapshot(1_000, 0, 0).unwrap();
    assert!(snap.packets.iter().all(|p| p.data.len() == 1000));
    assert!(ring.wait_disk_writes(WRITES));
    assert_eq!(ring.disk_queue_bytes(), 0);
    let snap = ring.snapshot(1_000, 0, 0).unwrap();
    assert!(
        snap.packets
            .iter()
            .any(|p| matches!(p.data, Payload::Mem(_))),
        "the packets that could not be queued stay in memory"
    );
}
