use fthrust_core::capture_config::{CaptureConfig, Scaling};
use fthrust_ipc::{BackendKind, EngineConfig};

fn cfg(f: impl FnOnce(&mut CaptureConfig)) -> EngineConfig {
    let mut capture = CaptureConfig::default();
    f(&mut capture);
    EngineConfig {
        capture,
        max_buffer_mb: 0,
        backend_override: None,
    }
}

#[test]
fn defaults_when_zero_or_missing() {
    let (n, warnings) = cfg(|_| {}).normalized();
    assert_eq!(n.capture.fps, 60);
    assert_eq!(n.capture.buffer_seconds, 30);
    assert_eq!(n.capture.bitrate_kbps, 16000);
    assert_eq!(n.capture.preset, 4);
    assert_eq!((n.capture.width, n.capture.height), (0, 0));
    assert_eq!(n.max_buffer_mb, 0);
    assert_eq!(n.capture.scaling, Scaling::Stretch);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn clamps_with_one_warning_each() {
    let (n, warnings) = EngineConfig {
        capture: CaptureConfig {
            fps: 1000,
            buffer_seconds: 901,
            bitrate_kbps: 100,
            preset: 9,
            width: 8000,
            height: 100,
            ..CaptureConfig::default()
        },
        max_buffer_mb: 10,
        backend_override: None,
    }
    .normalized();
    assert_eq!(n.capture.fps, 360);
    assert_eq!(n.capture.buffer_seconds, 900);
    assert_eq!(n.capture.bitrate_kbps, 500);
    assert_eq!(n.capture.preset, 7);
    assert_eq!((n.capture.width, n.capture.height), (0, 0));
    assert_eq!(n.max_buffer_mb, 64);
    assert_eq!(warnings.len(), 6, "{warnings:?}");
    for (field, used) in [
        ("fps", "360"),
        ("buffer_seconds", "900"),
        ("bitrate_kbps", "500"),
        ("preset", "7"),
        ("size", "native"),
        ("max_buffer_mb", "64"),
    ] {
        assert!(
            warnings.iter().any(|w| w.starts_with(field)
                && w.contains("out of range")
                && w.ends_with(&format!("using {used}"))),
            "{field}: {warnings:?}"
        );
    }
    let (n, warnings) = cfg(|c| {
        c.bitrate_kbps = 300_000;
    })
    .normalized();
    assert_eq!(n.capture.bitrate_kbps, 200_000);
    assert_eq!(warnings.len(), 1);
    let (n, warnings) = EngineConfig {
        capture: CaptureConfig::default(),
        max_buffer_mb: 20_000,
        backend_override: None,
    }
    .normalized();
    assert_eq!(n.max_buffer_mb, 16384);
    assert_eq!(warnings.len(), 1);
}

#[test]
fn half_specified_size_is_native() {
    let (n, warnings) = cfg(|c| {
        c.width = 1920;
        c.height = 0;
    })
    .normalized();
    assert_eq!((n.capture.width, n.capture.height), (0, 0));
    assert_eq!(warnings.len(), 1);
    let (n, warnings) = cfg(|c| {
        c.width = 1920;
        c.height = 1080;
    })
    .normalized();
    assert_eq!((n.capture.width, n.capture.height), (1920, 1080));
    assert!(warnings.is_empty());
    let (n, _) = cfg(|c| {
        c.width = 7680;
        c.height = 4320;
    })
    .normalized();
    assert_eq!(
        (n.capture.width, n.capture.height),
        (7680, 4320),
        "the maximum is allowed"
    );
}

#[test]
fn parse_override_cases() {
    assert_eq!(
        BackendKind::parse_override("WLR"),
        Some(BackendKind::WlrScreencopy)
    );
    assert_eq!(
        BackendKind::parse_override("ext"),
        Some(BackendKind::ExtImageCopy)
    );
    assert_eq!(
        BackendKind::parse_override("Portal"),
        Some(BackendKind::PortalScreenCast)
    );
    assert_eq!(
        BackendKind::parse_override("synthetic"),
        Some(BackendKind::Synthetic)
    );
    assert_eq!(BackendKind::parse_override("x11"), None);
    assert_eq!(BackendKind::parse_override(""), None);
    assert_eq!(BackendKind::WlrScreencopy.name(), "wlr-screencopy");
    assert_eq!(BackendKind::ExtImageCopy.name(), "ext-image-copy-capture");
    assert_eq!(BackendKind::PortalScreenCast.name(), "ScreenCast portal");
    assert_eq!(BackendKind::Synthetic.name(), "synthetic");
}

#[test]
fn socket_path_is_under_runtime_dir() {
    let p = fthrust_ipc::socket_path().unwrap();
    assert_eq!(p.file_name().unwrap(), "engine.sock");
    assert!(p.parent().unwrap().ends_with("fthrust") || p.parent().unwrap().ends_with("run"));
}
