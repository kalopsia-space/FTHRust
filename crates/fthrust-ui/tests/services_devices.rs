//! Audit 2026-10-31 tier 1 #2: the encoder test, the audio device lists and
//! the settings actions that used to fall through to "no consumer".

use std::cell::Cell;
use std::path::PathBuf;

use fthrust_media::capabilities::{CacheSignature, EncoderCapability};
use fthrust_ui::services::devices::{encoder_capabilities_with, parse_listing};
use fthrust_ui::services::{Event, Request, Services};
use fthrust_ui::state::{AppState, BrowseKind};

fn state() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    (tmp, s)
}

fn nvenc() -> EncoderCapability {
    EncoderCapability {
        key: "nvenc".into(),
        label: "NVIDIA NVENC".into(),
        codecs: vec!["h264".into()],
        codec_names: vec![("h264".into(), "h264_nvenc".into())],
        supports_presets: true,
    }
}

fn signature(dir: &std::path::Path) -> CacheSignature {
    let lib = dir.join("libavcodec.so.62");
    std::fs::write(&lib, b"x").unwrap();
    CacheSignature::for_path(&lib).unwrap()
}

#[test]
fn a_matching_cache_skips_the_probe_and_redetect_reprobes() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("encoder_capabilities.json");
    let sig = signature(tmp.path());
    let probes = Cell::new(0);
    let probe = || {
        probes.set(probes.get() + 1);
        vec![nvenc()]
    };
    // First start: nothing cached, so it probes and stores.
    assert_eq!(
        encoder_capabilities_with(&cache, false, Some(sig.clone()), probe),
        vec![nvenc()]
    );
    assert_eq!(probes.get(), 1);
    assert!(cache.is_file());
    // Next start: the cache answers.
    let found = encoder_capabilities_with(&cache, false, Some(sig.clone()), || {
        probes.set(probes.get() + 1);
        Vec::new()
    });
    assert_eq!(found, vec![nvenc()]);
    assert_eq!(probes.get(), 1, "a valid cache must not re-probe");
    // Redetect ignores the cache.
    let found = encoder_capabilities_with(&cache, true, Some(sig), || {
        probes.set(probes.get() + 1);
        Vec::new()
    });
    assert!(found.is_empty());
    assert_eq!(probes.get(), 2);
}

#[test]
fn listings_parse_into_label_and_id() {
    let text = r#"{"schema_version":1,"microphones":[
        {"endpoint_id":"alsa_input.usb-mic","display_name":"USB Mic","device_state":1,"is_default":true},
        {"endpoint_id":"alsa_input.pci","display_name":"","device_state":1,"is_default":false},
        {"endpoint_id":"","display_name":"Nameless","device_state":1,"is_default":false}
    ]}"#;
    assert_eq!(
        parse_listing(text).unwrap(),
        vec![
            ("USB Mic".to_owned(), "alsa_input.usb-mic".to_owned()),
            ("alsa_input.pci".to_owned(), "alsa_input.pci".to_owned()),
        ]
    );
    assert_eq!(
        parse_listing(r#"{"schema_version":1,"error":"no PipeWire"}"#).unwrap_err(),
        "no PipeWire"
    );
    assert!(parse_listing("not json").is_err());
}

#[test]
fn capability_and_device_events_fill_the_settings_state() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    assert!(s.encoder_capabilities.is_none());
    services.inject(Event::EncoderCapabilities(vec![nvenc()]));
    services.inject(Event::AudioDevices {
        playback: Ok(vec![("Speakers".into(), "alsa_output.pci".into())]),
        microphones: Ok(vec![("USB Mic".into(), "alsa_input.usb".into())]),
    });
    services.tick(&mut s);
    assert_eq!(s.encoder_capabilities, Some(vec![nvenc()]));
    assert_eq!(
        s.audio_devices,
        vec![("Speakers".to_owned(), "alsa_output.pci".to_owned())]
    );
    assert_eq!(
        s.mic_devices,
        vec![("USB Mic".to_owned(), "alsa_input.usb".to_owned())]
    );
    // A failed listing keeps what was there.
    services.inject(Event::AudioDevices {
        playback: Err("timed out".into()),
        microphones: Err("timed out".into()),
    });
    services.tick(&mut s);
    assert_eq!(s.audio_devices.len(), 1);
    assert_eq!(s.mic_devices.len(), 1);
}

#[test]
fn settings_actions_become_requests() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    s.redetect_encoders_requested = true;
    s.refresh_audio_devices_requested = true;
    s.open_screenshot_requested = Some(PathBuf::from("/c/shot.png"));
    s.show_in_file_manager_requested = Some(PathBuf::from("/c/clip.mp4"));
    services.tick(&mut s);
    let sent = services.drain_requests_for_tests();
    for want in [
        Request::RedetectEncoders,
        Request::RefreshAudioDevices,
        Request::OpenScreenshot(PathBuf::from("/c/shot.png")),
        Request::ShowInFileManager(PathBuf::from("/c/clip.mp4")),
    ] {
        assert!(sent.contains(&want), "{want:?} missing from {sent:?}");
    }
}

#[test]
fn theme_export_asks_where_and_writes_a_zip() {
    let (tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    let dest = tmp.path().join("my theme");
    services.inject(Event::Picked {
        kind: BrowseKind::ThemeExport,
        paths: Ok(vec![dest.clone()]),
    });
    services.tick(&mut s);
    let written = tmp.path().join("my theme.zip");
    assert!(written.is_file(), "the export gets a .zip name");
    let bytes = std::fs::read(&written).unwrap();
    assert!(bytes.starts_with(b"PK\x03\x04"), "a ZIP archive");
    assert!(
        bytes.windows(10).any(|w| w == b"theme.json"),
        "with theme.json inside"
    );
}

/// The engine reports a failed listing on stderr; that error reaches the
/// log instead of a JSON parse error about empty output.
#[test]
fn a_failed_listing_reports_the_engines_error() {
    use fthrust_ui::services::devices::listing_result;
    assert_eq!(
        listing_result(
            false,
            b"",
            b"some log line\n{\"schema_version\":1,\"error\":\"cannot connect to PipeWire\"}\n"
        )
        .unwrap_err(),
        "cannot connect to PipeWire"
    );
    assert!(listing_result(false, b"", b"").is_err());
    assert_eq!(
        listing_result(
            true,
            br#"{"schema_version":1,"microphones":[{"endpoint_id":"a","display_name":"A"}]}"#,
            b""
        )
        .unwrap(),
        vec![("A".to_owned(), "a".to_owned())]
    );
}
