use fthrust_core::capture_config::{
    ApplyStatus, CLIP_LENGTHS, CaptureConfig, CaptureConfigTracker, FPS_VALUES, MAX_CLIP_SECONDS,
    RING_BUDGET_MAX_MB, RING_BUDGET_MIN_MB, compute_buffer_seconds, custom_clip_lengths,
    linux_ring_budget_mb, validate_clip_length, validate_custom_bitrate_kbps,
    validate_encoder_preset, validate_fps,
};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/capture-config.json")).unwrap()
}

#[test]
fn constants_match_fixture() {
    let c = &fixture()["constants"];
    assert_eq!(c["max_clip_seconds"], MAX_CLIP_SECONDS);
    assert_eq!(c["ring_min_mb"], RING_BUDGET_MIN_MB);
    assert_eq!(c["ring_max_mb"], RING_BUDGET_MAX_MB);
    assert_eq!(
        CLIP_LENGTHS,
        [5, 10, 15, 30, 45, 60, 90, 120, 180, 240, 300]
    );
    assert_eq!(FPS_VALUES, [30, 60, 90, 120, 144, 165, 180, 240]);
}

#[test]
fn buffer_seconds_cases_match_fixture() {
    for case in fixture()["buffer_seconds_cases"].as_array().unwrap() {
        let len = case[0].as_u64().unwrap() as u32;
        let custom: Vec<u32> = case[1]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect();
        let expected = case[2].as_u64().unwrap() as u32;
        assert_eq!(
            compute_buffer_seconds(len, &custom).unwrap(),
            expected,
            "{case}"
        );
    }
}

#[test]
fn ring_budget_cases_match_fixture() {
    for case in fixture()["ring_budget_cases"].as_array().unwrap() {
        let bitrate = case[0].as_u64().unwrap() as u32;
        let buffer = case[1].as_u64().unwrap() as u32;
        let ram = case[2].as_u64().unwrap();
        let expected = case[3].as_u64().unwrap();
        assert_eq!(
            linux_ring_budget_mb(bitrate, buffer, Some(ram)),
            expected,
            "{case}"
        );
    }
}

#[test]
fn invalid_fps_is_error_not_clamped() {
    assert!(validate_fps(14).is_err());
    assert!(validate_fps(241).is_err());
    assert_eq!(validate_fps(15).unwrap(), 15);
    assert_eq!(validate_fps(240).unwrap(), 240);
    assert!(validate_clip_length(4).is_err());
    assert!(validate_clip_length(901).is_err());
    assert!(validate_custom_bitrate_kbps(499).is_err());
    assert_eq!(validate_custom_bitrate_kbps(200_000).unwrap(), 200_000);
    assert!(validate_encoder_preset(0).is_err());
    assert!(validate_encoder_preset(8).is_err());
    assert_eq!(validate_encoder_preset(7).unwrap(), 7);
}

#[test]
fn every_clip_length_fits_ring() {
    for len in CLIP_LENGTHS {
        assert!(compute_buffer_seconds(len, &[]).unwrap() <= MAX_CLIP_SECONDS);
    }
}

#[test]
fn custom_clip_lengths_are_validated_deduped_and_sorted() {
    let values = json!([60, 5, "x", 900, 901, 60, 4, true]);
    assert_eq!(custom_clip_lengths(&values), vec![5, 60, 900]);
    assert_eq!(custom_clip_lengths(&json!("nope")), Vec::<u32>::new());
}

#[test]
fn resolution_and_bitrate_tables_match_main_py() {
    use fthrust_core::capture_config::{BitrateLevel, bitrate_preset_kbps, resolution_dims};
    assert_eq!(resolution_dims("480p"), Some((854, 480)));
    assert_eq!(resolution_dims("720p"), Some((1280, 720)));
    assert_eq!(resolution_dims("1080p"), Some((1920, 1080)));
    assert_eq!(resolution_dims("1440p"), Some((2560, 1440)));
    assert_eq!(resolution_dims("source"), Some((0, 0)));
    assert_eq!(resolution_dims("8k"), None);
    assert_eq!(
        bitrate_preset_kbps("720p", BitrateLevel::High),
        Some(20_000)
    );
    assert_eq!(
        bitrate_preset_kbps("1440p", BitrateLevel::Low),
        Some(15_000)
    );
    assert_eq!(
        bitrate_preset_kbps("source", BitrateLevel::Medium),
        Some(25_000)
    );
    assert_eq!(bitrate_preset_kbps("8k", BitrateLevel::Medium), None);
}

#[test]
fn from_settings_uses_defaults_and_custom_bitrate() {
    use fthrust_core::FthrPaths;
    use fthrust_core::capture_config::{Codec, EncoderPref, Scaling};
    use fthrust_core::settings::SettingsStore;
    let tmp = tempfile::tempdir().unwrap();
    let mut store = SettingsStore::load(FthrPaths::new(tmp.path())).unwrap();
    let cfg = CaptureConfig::from_settings(&store).unwrap();
    assert_eq!(cfg.fps, 60);
    assert_eq!(cfg.buffer_seconds, 32);
    assert_eq!((cfg.width, cfg.height), (0, 0));
    assert_eq!(cfg.bitrate_kbps, 25_000);
    assert_eq!(cfg.codec, Codec::Auto);
    assert_eq!(cfg.preset, 4);
    assert_eq!(cfg.scaling, Scaling::Stretch);
    assert!(cfg.audio_enabled);
    assert_eq!(cfg.encoder, EncoderPref::Auto);
    assert_eq!(cfg.normal_clip_seconds, 30);
    assert!(!cfg.separate_audio_enabled);
    assert!(cfg.crop.is_none());

    store.set("resolution", json!("720p"));
    store.set("bitrate_level", json!("custom"));
    store.set("custom_bitrate_kbps", json!(40_000));
    store.set("codec_pref", json!("hevc"));
    store.set("encoder_pref", json!("amf"));
    store.set("scaling_mode", json!("fit"));
    store.set("audio_capture_mode", json!("separated"));
    store.set("custom_clip_lengths", json!([120, 60]));
    store.set(
        "quick_crop",
        json!({"x": 10, "y": 20, "w": 100, "h": 50, "src_w": 200, "src_h": 100}),
    );
    let cfg = CaptureConfig::from_settings(&store).unwrap();
    assert_eq!((cfg.width, cfg.height), (1280, 720));
    assert_eq!(cfg.bitrate_kbps, 40_000);
    assert_eq!(cfg.codec, Codec::Hevc);
    assert_eq!(
        cfg.encoder,
        EncoderPref::Vaapi,
        "the settings key amf is VA-API on Linux"
    );
    assert_eq!(cfg.scaling, Scaling::Fit);
    assert!(cfg.separate_audio_enabled);
    assert_eq!(cfg.buffer_seconds, 122);
    let crop = cfg.crop.unwrap();
    assert!((crop.x - 0.05).abs() < 1e-9 && (crop.y - 0.2).abs() < 1e-9);
    assert!((crop.w - 0.5).abs() < 1e-9 && (crop.h - 0.5).abs() < 1e-9);

    store.set("framerate", json!(9));
    assert!(
        CaptureConfig::from_settings(&store).is_err(),
        "invalid values are errors, never clamped"
    );
}

fn cfg(fps: u32) -> CaptureConfig {
    CaptureConfig {
        fps,
        ..CaptureConfig::default()
    }
}

#[test]
fn failed_restart_keeps_previous_active_config() {
    let mut tracker = CaptureConfigTracker::new(Some(cfg(60)));
    assert_eq!(tracker.status(), ApplyStatus::Active);
    tracker.request(cfg(120));
    assert_eq!(tracker.status(), ApplyStatus::Requested);
    tracker.begin_apply().unwrap();
    assert_eq!(tracker.status(), ApplyStatus::Applying);
    tracker.fail("encoder refused");
    assert_eq!(tracker.status(), ApplyStatus::Failed);
    assert_eq!(tracker.error(), "encoder refused");
    assert_eq!(tracker.active().map(|c| c.fps), Some(60));
    assert_eq!(tracker.requested().map(|c| c.fps), Some(120));

    tracker.request(cfg(144));
    tracker.begin_apply().unwrap();
    tracker.succeed().unwrap();
    assert_eq!(tracker.status(), ApplyStatus::Active);
    assert_eq!(tracker.active().map(|c| c.fps), Some(144));
    assert_eq!(tracker.error(), "");

    tracker.deactivate("engine exited");
    assert!(tracker.active().is_none());
    assert_eq!(tracker.status(), ApplyStatus::Failed);

    let mut empty = CaptureConfigTracker::new(None);
    assert!(empty.begin_apply().is_err());
    assert!(empty.succeed().is_err());
}

#[test]
fn ring_window_is_clip_plus_gop_and_margin() {
    assert_eq!(fthrust_core::capture_config::ring_window_ms(45), 47_500);
    assert_eq!(fthrust_core::capture_config::ring_window_ms(30), 32_500);
}

#[test]
fn buffer_location_parses_and_defaults_to_memory() {
    use fthrust_core::capture_config::BufferLocation;
    assert_eq!(BufferLocation::parse("disk"), BufferLocation::Disk);
    assert_eq!(BufferLocation::parse(" Disk "), BufferLocation::Disk);
    assert_eq!(BufferLocation::parse("memory"), BufferLocation::Memory);
    assert_eq!(BufferLocation::parse("ssd"), BufferLocation::Memory);
    assert_eq!(BufferLocation::default(), BufferLocation::Memory);
    // An engine config from an older UI has no field: memory.
    let mut v = serde_json::to_value(CaptureConfig::default()).unwrap();
    v.as_object_mut().unwrap().remove("buffer_location");
    let c: CaptureConfig = serde_json::from_value(v).unwrap();
    assert_eq!(c.buffer_location, BufferLocation::Memory);
}

#[test]
fn av1_presets_use_the_matched_table() {
    use fthrust_core::capture_config::{BitrateLevel, Codec, codec_preset_kbps};
    let av1 = |l| codec_preset_kbps("1080p", l, Codec::Av1);
    assert_eq!(av1(BitrateLevel::Low), Some(5_500));
    assert_eq!(av1(BitrateLevel::Medium), Some(17_500));
    assert_eq!(av1(BitrateLevel::High), Some(45_000));
    assert_eq!(
        codec_preset_kbps("source", BitrateLevel::High, Codec::Av1),
        Some(45_000)
    );
}

#[test]
fn h264_presets_are_unchanged() {
    use fthrust_core::capture_config::{
        BitrateLevel, Codec, bitrate_preset_kbps, codec_preset_kbps,
    };
    for codec in [Codec::Auto, Codec::H264, Codec::Hevc] {
        for level in [BitrateLevel::Low, BitrateLevel::Medium, BitrateLevel::High] {
            for res in ["480p", "720p", "1080p", "1440p", "source"] {
                assert_eq!(
                    codec_preset_kbps(res, level, codec),
                    bitrate_preset_kbps(res, level)
                );
            }
        }
    }
}

#[test]
fn custom_bitrate_ignores_codec() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store =
        fthrust_core::settings::SettingsStore::load(fthrust_core::FthrPaths::new(tmp.path()))
            .unwrap();
    store.set("codec_pref", json!("av1"));
    store.set("bitrate_level", json!("custom"));
    store.set("custom_bitrate_kbps", json!(33_000));
    assert_eq!(
        CaptureConfig::from_settings(&store).unwrap().bitrate_kbps,
        33_000
    );
    store.set("bitrate_level", json!("high"));
    store.set("resolution", json!("1080p"));
    assert_eq!(
        CaptureConfig::from_settings(&store).unwrap().bitrate_kbps,
        45_000
    );
}
