//! Plan 6a Task 4: the playback service on a null output with a manual clock.

#[path = "../../fthrust-media/tests/support/mod.rs"]
mod support;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fthrust_core::audio_manifest::{
    AudioManifest, AudioSource, MANIFEST_VERSION, SourceType, sha256_file,
};
use fthrust_ui::services::playback::audio_out::{
    AudioOutput, NullOutput, NullSwitches, PipeWireOutput,
};
use fthrust_ui::services::playback::sync::{SyncDecision, decide};
use fthrust_ui::services::playback::{ClockFn, PlaybackEvent, PlaybackService};

#[test]
fn sync_rule_matches_reference() {
    assert_eq!(decide(1150, 1000, false, false), SyncDecision::Hold);
    assert_eq!(decide(1060, 1000, true, false), SyncDecision::None);
    assert_eq!(decide(1030, 1000, true, false), SyncDecision::Resume);
    assert_eq!(decide(850, 1000, true, false), SyncDecision::Reseek(1000));
    assert_eq!(decide(870, 1000, false, false), SyncDecision::Reseek(1000));
    assert_eq!(decide(1100, 1000, false, false), SyncDecision::None);
    for (a, v, h) in [(1150, 1000, false), (850, 1000, true), (1030, 1000, true)] {
        assert_eq!(
            decide(a, v, h, true),
            SyncDecision::None,
            "guard suppresses {a} {v} {h}"
        );
    }
}

struct Clock(Arc<AtomicU64>);

impl Clock {
    fn new() -> Clock {
        Clock(Arc::new(AtomicU64::new(1000)))
    }
    fn func(&self) -> ClockFn {
        let c = self.0.clone();
        Arc::new(move || c.load(Ordering::Relaxed))
    }
    /// Advances `ms` in 20 ms steps, ticking the service and collecting frames.
    fn run(&self, svc: &mut PlaybackService, ms: u64) -> Vec<u64> {
        self.run_with(svc, ms, true)
    }

    fn run_with(&self, svc: &mut PlaybackService, ms: u64, take: bool) -> Vec<u64> {
        let mut frames = Vec::new();
        let mut left = ms;
        while left > 0 {
            let step = left.min(20);
            self.0.fetch_add(step, Ordering::Relaxed);
            left -= step;
            // Let the workers refill behind the clock.
            std::thread::sleep(Duration::from_millis(8));
            svc.tick();
            if take && let Some(f) = svc.take_frame() {
                frames.push(f.pts_ms);
            }
        }
        frames
    }
}

/// A clip whose audio and video lengths differ (`write_clip` keeps both).
fn write_uneven_clip(path: &Path, video_s: u32, audio_s: u32) -> Option<()> {
    use fthrust_core::capture_config::{Codec, EncoderPref};
    use fthrust_media::clip_writer::{ClipAudio, ClipVideo, write_clip};
    use fthrust_media::encoder::{Encoder, EncoderConfig};
    use fthrust_media::ring::EncodedRing;
    support::skip_unless_encoder(&["libopenh264", "libx264"])?;
    const FPS: u32 = 30;
    const SECOND: i64 = 1_000_000_000;
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
    let ring = EncodedRing::new(u64::from(video_s) * 1000 + 5000, FPS, 0);
    let mut out = Vec::new();
    for i in 0..video_s * FPS {
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
    let snapshot = ring
        .snapshot(video_s * 1000, i64::from(video_s) * SECOND, 0)
        .unwrap();
    let video = ClipVideo {
        snapshot: &snapshot,
        codec_id: encoder.codec_id(),
        extradata: &encoder.extradata(),
        width: 160,
        height: 90,
        bitrate_kbps: 300,
        separate_audio: false,
    };
    let samples: Vec<f32> = (0..u64::from(audio_s) * 48_000)
        .flat_map(|i| {
            let v = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
            [v, v]
        })
        .collect();
    let track = ClipAudio {
        samples: &samples,
        sample_rate: 48_000,
        channels: 2,
        title: "Default Mix",
    };
    write_clip(path, &video, std::slice::from_ref(&track), None).unwrap();
    Some(())
}

fn open(clip: &Path, home: &Path, clock: &Clock, output: Box<dyn AudioOutput>) -> PlaybackService {
    PlaybackService::open(clip, home, output, clock.func()).expect("open")
}

#[test]
fn playback_advances_and_delivers_frames_in_order() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("clip.mp4");
    if support::write_test_clip(&clip, 2, true).is_none() {
        return;
    }
    let clock = Clock::new();
    let mut svc = open(
        &clip,
        d.path(),
        &clock,
        Box::new(NullOutput::new(clock.func())),
    );
    assert!(svc.audio_live(), "{:?}", svc.events());
    assert!(
        (1990..=2060).contains(&svc.duration_ms()),
        "{}",
        svc.duration_ms()
    );
    svc.play();
    let frames = clock.run(&mut svc, 500);
    let pos = svc.position_ms();
    assert!((480..=560).contains(&pos), "position {pos}");
    assert!(frames.len() >= 8, "frames {frames:?}");
    assert!(frames.windows(2).all(|w| w[0] < w[1]), "order {frames:?}");
    let last = *frames.last().unwrap();
    assert!(
        (380..=560).contains(&last),
        "last frame {last} of {frames:?}"
    );
    svc.close();
}

#[test]
fn seek_and_rate_keep_position() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("clip.mp4");
    if support::write_test_clip(&clip, 4, true).is_none() {
        return;
    }
    let clock = Clock::new();
    let mut svc = open(
        &clip,
        d.path(),
        &clock,
        Box::new(NullOutput::new(clock.func())),
    );
    svc.seek_ms(1000);
    svc.play();
    clock.run(&mut svc, 200);
    let p1 = svc.position_ms();
    assert!((1150..=1260).contains(&p1), "after 200 ms: {p1}");
    svc.set_rate(2.0, true);
    clock.run(&mut svc, 200);
    let p2 = svc.position_ms();
    assert!((1500..=1700).contains(&p2), "after 200 ms at 2x: {p2}");
    svc.set_rate(1.0, true);
    clock.run(&mut svc, 100);
    let p3 = svc.position_ms();
    assert!(
        p3 >= p2 && p3 <= p2 + 160,
        "after rate back to 1x: {p3} (was {p2})"
    );
    svc.close();
}

#[test]
fn eof_is_reported_once_and_playing_stops() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("clip.mp4");
    if support::write_test_clip(&clip, 1, true).is_none() {
        return;
    }
    let clock = Clock::new();
    let mut svc = open(
        &clip,
        d.path(),
        &clock,
        Box::new(NullOutput::new(clock.func())),
    );
    svc.play();
    let mut events = Vec::new();
    for _ in 0..8 {
        clock.run(&mut svc, 250);
        events.extend(svc.events());
        if !svc.playing() {
            break;
        }
    }
    clock.run(&mut svc, 100);
    events.extend(svc.events());
    let eofs = events.iter().filter(|e| **e == PlaybackEvent::Eof).count();
    assert_eq!(eofs, 1, "{events:?}");
    assert!(!svc.playing());
    assert_eq!(svc.position_ms(), svc.duration_ms());
    svc.close();
}

#[test]
fn audio_failure_stops_cleanly() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("clip.mp4");
    if support::write_test_clip(&clip, 2, true).is_none() {
        return;
    }
    let clock = Clock::new();
    let mut svc = open(
        &clip,
        d.path(),
        &clock,
        Box::new(NullOutput::new(clock.func()).failing()),
    );
    assert!(!svc.audio_live());
    let events = svc.events();
    let failures = events
        .iter()
        .filter(|e| matches!(e, PlaybackEvent::AudioFailed(_)))
        .count();
    assert_eq!(failures, 1, "{events:?}");
    svc.play();
    let frames = clock.run(&mut svc, 400);
    let pos = svc.position_ms();
    assert!((380..=420).contains(&pos), "video clock {pos}");
    assert!(!frames.is_empty());
    assert!(svc.events().is_empty());
    svc.close();
}

#[test]
fn missing_track_is_reported_not_fatal() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("clip.mp4");
    if support::write_tone_clip(&clip, 2, &[("Game", 440.0), ("Mic", 880.0)]).is_none() {
        return;
    }
    let source = |uuid: &str, name: &str, index: u32| AudioSource {
        source_uuid: uuid.into(),
        stream_index: index,
        source_type: SourceType::Application,
        display_name: name.into(),
        persistent_identity: name.into(),
        icon_reference: None,
        first_active_100ns: 0,
        last_active_100ns: 20_000_000,
        sample_rate: 48_000,
        channels: 2,
        sample_format: "f32".into(),
    };
    let manifest = AudioManifest {
        manifest_version: MANIFEST_VERSION,
        transaction_id: uuid::Uuid::new_v4().to_string(),
        media_file: "clip.mp4".into(),
        media_sha256: sha256_file(&clip).unwrap(),
        sources: vec![
            source("11111111-1111-4111-8111-111111111111", "Game", 1),
            source("22222222-2222-4222-8222-222222222222", "Mic", 7),
        ],
    };
    manifest.write(&clip).unwrap();
    let clock = Clock::new();
    let mut svc = open(
        &clip,
        d.path(),
        &clock,
        Box::new(NullOutput::new(clock.func())),
    );
    assert!(svc.audio_live());
    let events = svc.events();
    assert_eq!(
        events,
        vec![PlaybackEvent::SourceFailed(
            "22222222-2222-4222-8222-222222222222".into()
        )]
    );
    assert_eq!(svc.sources().len(), 2);
    svc.play();
    clock.run(&mut svc, 300);
    assert!(svc.position_ms() >= 250, "{}", svc.position_ms());
    svc.close();
}

/// Plays one second of a silent clip through PipeWire (ignored: needs a
/// live session). `cargo test -p fthrust-ui --test playback_service pipewire_live -- --ignored`
#[test]
#[ignore]
fn pipewire_live() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("clip.mp4");
    if support::write_gop_clip(&clip, 2, Some(30)).is_none() {
        return;
    }
    let clock = fthrust_ui::services::playback::real_clock();
    let mut svc = PlaybackService::open(&clip, d.path(), Box::new(PipeWireOutput::new()), clock)
        .expect("open");
    assert!(svc.audio_live(), "{:?}", svc.events());
    svc.play();
    let start = std::time::Instant::now();
    let mut max_pos = 0;
    while start.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(20));
        svc.tick();
        let _ = svc.take_frame();
        max_pos = max_pos.max(svc.position_ms());
    }
    let events = svc.events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PlaybackEvent::AudioFailed(_))),
        "{events:?}"
    );
    assert!(
        (800..=1200).contains(&max_pos),
        "position after 1 s: {max_pos}"
    );
    svc.close();
}

#[test]
fn audio_device_loss_mid_play_is_reported_once() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("clip.mp4");
    if support::write_test_clip(&clip, 3, true).is_none() {
        return;
    }
    let clock = Clock::new();
    let switches = NullSwitches::default();
    let out = NullOutput::new(clock.func()).with_switches(switches.clone());
    let mut svc = open(&clip, d.path(), &clock, Box::new(out));
    assert!(svc.audio_live());
    svc.play();
    clock.run(&mut svc, 300);
    let before = svc.position_ms();
    switches.fail.store(true, Ordering::Relaxed);
    clock.run(&mut svc, 300);
    let events = svc.events();
    let failures = events
        .iter()
        .filter(|e| matches!(e, PlaybackEvent::AudioFailed(_)))
        .count();
    assert_eq!(failures, 1, "{events:?}");
    assert!(!svc.audio_live());
    assert!(svc.playing(), "playback carries on without audio");
    let after = svc.position_ms();
    assert!(
        (before + 200..=before + 400).contains(&after),
        "video clock continues: {before} -> {after}"
    );
    clock.run(&mut svc, 200);
    assert!(svc.events().is_empty(), "reported once");
    svc.close();
}

#[test]
fn position_accounts_for_queued_audio_at_rate() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("clip.mp4");
    if support::write_test_clip(&clip, 4, true).is_none() {
        return;
    }
    let clock = Clock::new();
    let mut svc = open(
        &clip,
        d.path(),
        &clock,
        Box::new(NullOutput::new(clock.func())),
    );
    svc.seek_ms(1000);
    svc.set_rate(2.0, true);
    svc.play();
    // The worker fills the queue (150 ms of output = 300 ms of source) while
    // the clock stands still: nothing has been heard yet.
    std::thread::sleep(Duration::from_millis(250));
    svc.tick();
    let pos = svc.position_ms();
    assert!(
        (990..=1050).contains(&pos),
        "queued audio must not count: {pos}"
    );
    svc.close();
}

#[test]
fn video_shorter_than_audio_still_ends() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("uneven.mp4");
    if write_uneven_clip(&clip, 1, 2).is_none() {
        return;
    }
    let clock = Clock::new();
    let mut svc = open(
        &clip,
        d.path(),
        &clock,
        Box::new(NullOutput::new(clock.func())),
    );
    svc.play();
    let mut events = Vec::new();
    for _ in 0..12 {
        clock.run(&mut svc, 250);
        events.extend(svc.events());
        if !svc.playing() {
            break;
        }
    }
    let eofs = events.iter().filter(|e| **e == PlaybackEvent::Eof).count();
    assert_eq!(eofs, 1, "{events:?}");
    assert!(!svc.playing());
    svc.close();
}

#[test]
fn audio_shorter_than_video_plays_to_the_end() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("uneven.mp4");
    if write_uneven_clip(&clip, 2, 1).is_none() {
        return;
    }
    let clock = Clock::new();
    let mut svc = open(
        &clip,
        d.path(),
        &clock,
        Box::new(NullOutput::new(clock.func())),
    );
    assert!(svc.duration_ms() >= 1900, "{}", svc.duration_ms());
    svc.play();
    let mut frames = Vec::new();
    let mut events = Vec::new();
    for _ in 0..12 {
        frames.extend(clock.run(&mut svc, 250));
        events.extend(svc.events());
        if !svc.playing() {
            break;
        }
    }
    let eofs = events.iter().filter(|e| **e == PlaybackEvent::Eof).count();
    assert_eq!(eofs, 1, "{events:?}");
    let last = *frames.last().expect("frames");
    assert!(
        last >= 1800,
        "video played past the end of the audio: last frame {last}"
    );
    svc.close();
}

#[test]
fn pause_stops_audio_and_shows_the_paused_frame() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("clip.mp4");
    if support::write_test_clip(&clip, 3, true).is_none() {
        return;
    }
    let clock = Clock::new();
    let mut svc = open(
        &clip,
        d.path(),
        &clock,
        Box::new(NullOutput::new(clock.func())),
    );
    svc.play();
    clock.run(&mut svc, 400);
    svc.pause();
    let paused = svc.position_ms();
    // Time passes; nothing may move.
    clock.run_with(&mut svc, 400, false);
    assert_eq!(svc.position_ms(), paused, "position after pause");
    let frame = (0..20)
        .find_map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            svc.tick();
            svc.take_frame()
        })
        .expect("the paused frame");
    assert!(
        frame.pts_ms + 40 >= paused && frame.pts_ms <= paused + 40,
        "paused frame {} for position {paused}",
        frame.pts_ms
    );
    svc.close();
}

#[test]
fn hold_then_rate_change_resumes_output() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("clip.mp4");
    if support::write_test_clip(&clip, 4, true).is_none() {
        return;
    }
    let clock = Clock::new();
    let switches = NullSwitches::default();
    let out = NullOutput::new(clock.func()).with_switches(switches.clone());
    let mut svc = open(&clip, d.path(), &clock, Box::new(out));
    svc.play();
    // One frame shown, then the UI stops taking frames: audio runs ahead
    // and, once the guard expires, the sync rule holds the output.
    clock.run(&mut svc, 60);
    clock.run_with(&mut svc, 1400, false);
    assert!(switches.suspended.load(Ordering::Relaxed), "held");
    svc.set_rate(1.5, true);
    assert!(
        !switches.suspended.load(Ordering::Relaxed),
        "rate change resumes the output"
    );
    svc.close();
}

#[test]
fn dropping_the_service_cancels_the_proxy_build() {
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("long.mp4");
    if support::write_gop_clip(&clip, 8, None).is_none() {
        return;
    }
    let clock = Clock::new();
    let svc = open(
        &clip,
        d.path(),
        &clock,
        Box::new(NullOutput::new(clock.func())),
    );
    drop(svc);
    std::thread::sleep(Duration::from_millis(1500));
    let cache = fthrust_media::playback::proxy::cache_dir(d.path());
    let finals: Vec<String> = std::fs::read_dir(&cache)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.contains(".partial"))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        finals.is_empty(),
        "a dropped service kept transcoding: {finals:?}"
    );
}
