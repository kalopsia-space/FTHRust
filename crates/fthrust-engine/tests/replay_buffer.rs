use std::os::unix::fs::PermissionsExt;

use fthrust_core::capture_config::{BufferLocation, CaptureConfig};
use fthrust_engine::engine::make_ring_in;
use fthrust_ipc::EngineConfig;
use fthrust_media::payload::Payload;
use fthrust_media::ring::EncodedPacket;

fn config(location: BufferLocation) -> EngineConfig {
    EngineConfig {
        capture: CaptureConfig {
            fps: 30,
            buffer_seconds: 10,
            buffer_location: location,
            ..CaptureConfig::default()
        },
        max_buffer_mb: 0,
        backend_override: None,
    }
}

fn push_second(ring: &fthrust_media::ring::EncodedRing) {
    for f in 0..30i64 {
        ring.push(EncodedPacket {
            data: vec![7u8; 2000].into(),
            pts: f,
            dts: f,
            keyframe: f == 0,
            wall_time_ns: f * 33_333_333,
            generation: 0,
        });
    }
}

#[test]
fn disk_mode_ring_writes_segments() {
    let root = tempfile::tempdir().unwrap();
    let ring = make_ring_in(&config(BufferLocation::Disk), root.path());
    push_second(&ring);
    let own = fthrust_engine::replay_dir::own_dir(root.path());
    let files: Vec<_> = std::fs::read_dir(&own).unwrap().collect();
    assert!(!files.is_empty(), "segments under {}", own.display());
    let snap = ring.snapshot(1_000, 0, 0).unwrap();
    assert!(
        snap.packets
            .iter()
            .all(|p| matches!(p.data, Payload::Disk(_)))
    );
}

#[test]
fn memory_mode_ring_writes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let ring = make_ring_in(&config(BufferLocation::Memory), root.path());
    push_second(&ring);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn unwritable_cache_falls_back_to_memory() {
    let root = tempfile::tempdir().unwrap();
    let locked = root.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    let ring = make_ring_in(&config(BufferLocation::Disk), &locked.join("replay"));
    push_second(&ring);
    let snap = ring.snapshot(1_000, 0, 0).unwrap();
    assert_eq!(snap.packets.len(), 30);
    assert!(
        snap.packets
            .iter()
            .all(|p| matches!(p.data, Payload::Mem(_)))
    );
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
}
