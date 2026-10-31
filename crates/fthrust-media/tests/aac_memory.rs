//! Peak memory of a clip's AAC encode, measured with a counting allocator
//! (its own test binary, so nothing else allocates through it meanwhile).

mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use fthrust_core::capture_config::{Codec, EncoderPref};
use fthrust_media::clip_writer::{ClipAudio, ClipVideo, write_clip};
use fthrust_media::encoder::{Encoder, EncoderConfig};
use fthrust_media::ring::EncodedRing;

const SECOND: i64 = 1_000_000_000;
const FPS: u32 = 30;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// The system allocator, counting the bytes Rust code holds.
struct Counting;

fn grew(by: usize) {
    let now = CURRENT.fetch_add(by, Ordering::SeqCst) + by;
    PEAK.fetch_max(now, Ordering::SeqCst);
}

// SAFETY: every call is forwarded unchanged to the system allocator; the
// counters only observe sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded with the caller's layout.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded with the caller's pointer and layout.
        unsafe { System.dealloc(ptr, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded with the caller's pointer, layout and size.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
            grew(new_size);
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn encoding_a_long_track_does_not_copy_it() {
    // The AAC encoder appended its whole input to an internal buffer before
    // encoding it, so a long save held every track twice: 46 MB more for
    // a 120 s clip. Whole frames now come straight from the caller's slice.
    if support::skip_unless_encoder(&["libopenh264", "libx264"]).is_none() {
        return;
    }
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
    let ring = EncodedRing::new(10_000, FPS, 0);
    let mut out = Vec::new();
    for i in 0..2 * FPS {
        let bgra = support::synthetic_bgra(160, 90, i);
        encoder
            .encode(
                &bgra,
                160 * 4,
                160,
                90,
                i64::from(i) * SECOND / i64::from(FPS),
                &mut out,
            )
            .unwrap();
    }
    encoder.flush(&mut out).unwrap();
    for p in out {
        ring.push(p);
    }
    let snapshot = ring.snapshot(2_000, 0, 0).unwrap();
    let video = ClipVideo {
        snapshot: &snapshot,
        codec_id: encoder.codec_id(),
        extradata: &encoder.extradata(),
        width: 160,
        height: 90,
        bitrate_kbps: 300,
        separate_audio: false,
    };
    // 60 s of stereo: 23 MB.
    let pcm: Vec<f32> = (0..60 * 48_000usize)
        .flat_map(|i| {
            let v = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
            [v, v]
        })
        .collect();
    let track = ClipAudio {
        samples: &pcm,
        sample_rate: 48_000,
        channels: 2,
        title: "Default Mix",
    };
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("long.mp4");
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let written = write_clip(&path, &video, std::slice::from_ref(&track), None).unwrap();
    let extra = PEAK.load(Ordering::SeqCst) - before;
    assert!(written.audio_included);
    let input = pcm.len() * std::mem::size_of::<f32>();
    assert!(
        extra < input / 4,
        "writing the clip held {extra} more bytes for a {input}-byte track"
    );
}
