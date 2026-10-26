use std::os::unix::fs::PermissionsExt;

use fthrust_media::payload::{Payload, SegmentStore};
use fthrust_media::ring::{EncodedPacket, EncodedRing};

const SECOND: i64 = 1_000_000_000;

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
    let first = segments(dir.path())[0].clone();
    for f in 30..300 {
        ring.push(packet(f, 30, 1000));
    }
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
    let snap = ring.snapshot(1_000, 0, 0).unwrap();
    for f in 30..300 {
        ring.push(packet(f, 30, 1000));
    }
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
    let snap = ring.snapshot(2_000, 0, 0).unwrap();
    assert_eq!(snap.packets.len(), 60);
    assert!(
        snap.packets
            .iter()
            .all(|p| matches!(p.data, Payload::Mem(_)))
    );
}
