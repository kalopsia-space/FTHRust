//! Plan 5 Task 15: notification sounds.

use std::sync::{Arc, Mutex};

use fthrust_media::audio_decode::Pcm;
use fthrust_ui::services::sounds::{Sink, SoundPlayer, effective_volume, resolve_sound_path};
use fthrust_ui::state::AppState;

struct FakeSink {
    played: Mutex<Vec<(usize, f32)>>,
}

impl Sink for FakeSink {
    fn play(&self, pcm: Arc<Pcm>, gain: f32) {
        self.played.lock().unwrap().push((pcm.samples.len(), gain));
    }

    fn stop(&self) {}
}

#[test]
fn sounds_master_volume_and_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(
        tmp.path(),
        serde_json::json!({"sound_volume_clip": 80, "sound_volume_master": 50}),
    );
    assert_eq!(effective_volume(&s.settings, "clip_captured"), Some(40));
    assert_eq!(effective_volume(&s.settings, "startup"), Some(50));
    s.set_setting("notification_sounds_enabled", serde_json::json!(false));
    assert_eq!(effective_volume(&s.settings, "clip_captured"), None);
    s.set_setting("notification_sounds_enabled", serde_json::json!(true));
    s.set_setting("sound_volume_master", serde_json::json!(250));
    assert_eq!(
        effective_volume(&s.settings, "clip_captured"),
        Some(80),
        "master clamps to 100"
    );

    let sink = Arc::new(FakeSink {
        played: Mutex::new(Vec::new()),
    });
    let mut player = SoundPlayer::with_sink(sink.clone(), fthrust_ui::assets_dir());
    player.preload(&s.theme);
    s.set_setting("sound_volume_master", serde_json::json!(50));
    player.play(&s.settings, &s.theme, "clip_captured");
    let played = sink.played.lock().unwrap().clone();
    assert_eq!(played.len(), 1);
    assert!(played[0].0 > 1000, "decoded samples: {}", played[0].0);
    assert!((played[0].1 - 0.4).abs() < 1e-6, "gain {}", played[0].1);
    s.set_setting("notification_sounds_enabled", serde_json::json!(false));
    player.play(&s.settings, &s.theme, "clip_captured");
    assert_eq!(sink.played.lock().unwrap().len(), 1, "gated");
    // Preview ignores the gate but applies the volumes.
    player.preview(&s.settings, &s.theme, "error");
    let played = sink.played.lock().unwrap().clone();
    assert_eq!(played.len(), 2);
    assert!((played[1].1 - 0.5).abs() < 1e-6);
    // Unknown keys are ignored.
    player.play(&s.settings, &s.theme, "nope");
    assert_eq!(sink.played.lock().unwrap().len(), 2);
    // Bundled paths; a custom theme sound wins.
    let bundled =
        resolve_sound_path(&s.theme, &fthrust_ui::assets_dir(), "screenshot_captured").unwrap();
    assert!(
        bundled.ends_with("sounds/screenshot_saved.wav"),
        "{bundled:?}"
    );
    let custom_src = tmp.path().join("my.wav");
    std::fs::copy(&bundled, &custom_src).unwrap();
    s.theme
        .set_custom_sound("screenshot_captured", &custom_src)
        .unwrap();
    let custom =
        resolve_sound_path(&s.theme, &fthrust_ui::assets_dir(), "screenshot_captured").unwrap();
    assert_ne!(custom, bundled);
    assert!(custom.exists());
}

#[test]
fn customize_preview_plays_through_the_services() {
    use fthrust_ui::services::Services;
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(
        tmp.path(),
        serde_json::json!({"notification_sounds_enabled": false}),
    );
    let sink = Arc::new(FakeSink {
        played: Mutex::new(Vec::new()),
    });
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.set_sound_sink_for_tests(sink.clone());
    s.sound_preview_requested = Some("error".into());
    services.tick(&mut s);
    assert_eq!(
        sink.played.lock().unwrap().len(),
        1,
        "the preview plays even with sounds disabled"
    );
    assert!(s.sound_preview_requested.is_none());
}
