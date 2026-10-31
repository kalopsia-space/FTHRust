use std::time::{Duration, Instant};

use fthrust_engine::audio::pipewire::PipeWireAudio;
use fthrust_engine::audio::{
    AudioConfig, AudioProvider, AudioRing, AudioSourceKind, AudioTarget, SyntheticAudio,
};
use fthrust_ipc::SourceStatus;
use fthrust_media::audio_timing::AudioBlock;

const MS: i64 = 1_000_000;

fn block(start_ms: i64, frames: usize, value: f32) -> AudioBlock {
    AudioBlock {
        start_ns: start_ms * MS,
        samples: vec![value; frames * 2],
    }
}

#[test]
fn ring_window_pads_trims_and_prunes() {
    let mut ring = AudioRing::new(1);
    // 100 ms of audio from t=100 ms, then a 200 ms gap, then 100 ms more.
    ring.push(block(100, 4800, 0.5));
    ring.push(block(400, 4800, 0.25));
    // Window [300, 500): 100 ms silence, then the second block's first 100 ms.
    let w = ring.window(500 * MS, 200);
    assert_eq!(w.len(), 200 * 48 * 2);
    assert!(w[..4800 * 2].iter().all(|s| *s == 0.0), "gap is silence");
    assert!(w[4800 * 2..].iter().all(|s| *s == 0.25));
    // A window ending before any audio is all silence of the right length.
    assert!(ring.window(50 * MS, 10).iter().all(|s| *s == 0.0));
    // Capacity is (keep + 10) s: 12 s of blocks prunes the oldest.
    let mut ring = AudioRing::new(1);
    for i in 0..1200 {
        ring.push(block(i * 10, 480, 1.0));
    }
    assert!(ring.len() <= 1100, "{} blocks", ring.len());
    ring.clear();
    assert!(ring.is_empty());
}

#[test]
fn synthetic_audio_delivers_tone() {
    let audio = SyntheticAudio::new();
    let warnings = audio.start(&AudioConfig {
        system: Some(AudioTarget::DefaultSinkMonitor),
        microphone: Some(AudioTarget::Source("mic".into())),
        keep_seconds: 5,
    });
    assert!(warnings.is_empty());
    std::thread::sleep(Duration::from_millis(1200));
    let end = fthrust_wayland::backend::monotonic_ns();
    let w = audio.window(AudioSourceKind::System, end, 1000).unwrap();
    assert_eq!(w.len(), 48_000 * 2);
    let rms = (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).sqrt();
    assert!((rms - 0.212).abs() < 0.03, "rms {rms}");
    assert!(
        audio
            .window(AudioSourceKind::Microphone, end, 100)
            .is_some()
    );
    assert_eq!(audio.status().system, SourceStatus::Streaming);
    audio.stop();
    let unavailable = SyntheticAudio::unavailable();
    let warnings = unavailable.start(&AudioConfig {
        system: Some(AudioTarget::DefaultSinkMonitor),
        microphone: None,
        keep_seconds: 5,
    });
    assert_eq!(warnings[0].0, "DESKTOP_AUDIO_UNAVAILABLE");
    assert!(
        unavailable
            .window(AudioSourceKind::System, end, 100)
            .is_none()
    );
}

#[test]
fn live_system_capture_streams() {
    let audio = PipeWireAudio::new();
    let warnings = audio.start(&AudioConfig {
        system: Some(AudioTarget::DefaultSinkMonitor),
        microphone: None,
        keep_seconds: 5,
    });
    if !warnings.is_empty() {
        eprintln!("skipping: {warnings:?}");
        return;
    }
    let started = Instant::now();
    while audio.status().system != SourceStatus::Streaming {
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "status {:?}",
            audio.status()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(700));
    let end = fthrust_wayland::backend::monotonic_ns();
    let w = audio
        .window(AudioSourceKind::System, end - 50 * MS, 500)
        .unwrap();
    assert_eq!(w.len(), 24_000 * 2);
    assert!(
        audio.blocks_received(AudioSourceKind::System) > 10,
        "blocks arrived"
    );
    assert_eq!(audio.status().microphone, SourceStatus::Disabled);
    let stop_started = Instant::now();
    audio.stop();
    assert!(
        stop_started.elapsed() < Duration::from_secs(2),
        "stop is bounded"
    );
}

#[test]
fn a_long_window_does_not_hold_up_the_capture_callback() {
    // A 120 s save copied every block (about 46 MB) while holding the source
    // lock that the real-time capture callback takes for each block it
    // delivers. The window now only shares the blocks under the lock.
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    const SECOND: i64 = 1_000_000_000;
    const BLOCK_FRAMES: usize = 1024;
    let block_ns = BLOCK_FRAMES as i64 * SECOND / 48_000;
    let audio = Arc::new(PipeWireAudio::detached(&AudioConfig {
        system: Some(AudioTarget::DefaultSinkMonitor),
        microphone: None,
        keep_seconds: 120,
    }));
    let mut t = 0;
    while t < 120 * SECOND {
        audio.push_block(AudioSourceKind::System, block(t / MS, BLOCK_FRAMES, 0.1));
        t += block_ns;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let pushed = Arc::new(Mutex::new(Vec::new()));
    let producer = {
        let (audio, stop, pushed) = (audio.clone(), stop.clone(), pushed.clone());
        std::thread::spawn(move || {
            let mut t = t;
            while !stop.load(Ordering::SeqCst) {
                audio.push_block(AudioSourceKind::System, block(t / MS, BLOCK_FRAMES, 0.1));
                pushed.lock().unwrap().push(Instant::now());
                t += block_ns;
                std::thread::sleep(Duration::from_micros(100));
            }
        })
    };
    let mut windows = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        let w = audio
            .window(AudioSourceKind::System, 120 * SECOND, 120_000)
            .unwrap();
        windows.push((started, Instant::now()));
        assert_eq!(w.len(), 120 * 48_000 * 2);
        std::thread::sleep(Duration::from_millis(20));
    }
    stop.store(true, Ordering::SeqCst);
    producer.join().unwrap();
    let pushed = pushed.lock().unwrap();
    let delivered_meanwhile = windows
        .iter()
        .filter(|(started, ended)| {
            let quarter = (*ended - *started) / 4;
            let (from, to) = (*started + quarter, *ended - quarter);
            pushed.iter().any(|p| *p > from && *p < to)
        })
        .count();
    assert!(
        delivered_meanwhile > 0,
        "no block was delivered while a 120 s window was assembled ({windows:?})"
    );
}
