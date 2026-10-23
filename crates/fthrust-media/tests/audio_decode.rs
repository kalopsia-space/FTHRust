//! Plan 5 Task 3: notification sound decoding.

use fthrust_media::audio_decode::{Pcm, decode_to_pcm};

fn assets() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

#[test]
fn decode_wav_to_48k_stereo() {
    let pcm: Pcm = decode_to_pcm(&assets().join("sounds/clip_captured.wav")).unwrap();
    assert_eq!(pcm.rate, 48000);
    assert_eq!(pcm.channels, 2);
    assert_eq!(pcm.samples.len() % 2, 0);
    assert!(pcm.samples.len() > 4800, "more than 50 ms of audio");
    let peak = pcm.samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05 && peak <= 1.0, "peak {peak}");
    assert!(pcm.samples.len() <= 48000 * 2 * 30, "capped at 30 s");
}

#[test]
fn decode_rejects_non_audio() {
    let d = tempfile::tempdir().unwrap();
    let png = d.path().join("x.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]))
        .save(&png)
        .unwrap();
    assert!(decode_to_pcm(&png).is_err());
    assert!(decode_to_pcm(&d.path().join("missing.wav")).is_err());
}
