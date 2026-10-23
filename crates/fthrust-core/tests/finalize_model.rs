//! Post-save overlays (notes-overlays-reference sections 4, 8, 17) and Gary
//! maths (section 9).

use std::path::{Path, PathBuf};

use fthrust_core::FthrPaths;
use fthrust_core::export::graph::{AudioOut, MapRef, VideoOut};
use fthrust_core::finalize::{self, ImageLayer, Rect};
use fthrust_core::gary;
use fthrust_core::settings::SettingsStore;
use serde_json::json;

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn rect_clamp_matches_reference() {
    let d = finalize::CAMERA_DEFAULT;
    assert_eq!(d, rect(0.72, 0.64, 0.25, 0.34));
    // Size first, then position with the clamped size; four decimals.
    let r = finalize::clamp_rect(&json!({"x": 0.95, "y": -1, "w": 0.99, "h": 0.01}), d);
    assert_eq!(r, rect(0.08, 0.0, 0.92, 0.08));
    let r = finalize::clamp_rect(&json!({"x": 0.123456, "y": 0.5, "w": 0.3, "h": 0.3}), d);
    assert_eq!(r, rect(0.1235, 0.5, 0.3, 0.3));
    // Bad fields fall back per field; a non-object is the default.
    let r = finalize::clamp_rect(&json!({"x": "left", "y": 0.1, "w": null, "h": 0.2}), d);
    assert_eq!(r, rect(0.72, 0.1, 0.25, 0.2));
    assert_eq!(finalize::clamp_rect(&json!("nope"), d), d);
}

fn store(dir: &Path, values: serde_json::Value) -> SettingsStore {
    let mut s = SettingsStore::load(FthrPaths::new(dir)).unwrap();
    for (k, v) in values.as_object().unwrap() {
        s.set(k, v.clone());
    }
    s
}

#[test]
fn layers_normalised() {
    let tmp = tempfile::tempdir().unwrap();
    let s = store(
        tmp.path(),
        json!({
            "image_overlay_enabled": true,
            "image_overlays": [
                {"path": "/a.png", "opacity": 3, "fit": "fill", "rect": {"x": 0.1, "y": 0.1, "w": 0.2, "h": 0.2}},
                {"path": "/b.png", "enabled": false},
                {"path": "/z.png", "enabled": null},
                {"path": "", "opacity": 50},
                "junk",
                {"path": "/c.png", "opacity": "x", "fit": "FILL", "rect": "broken"}
            ]
        }),
    );
    let layers = finalize::image_layers(&s);
    assert_eq!(
        layers,
        vec![
            ImageLayer {
                path: PathBuf::from("/a.png"),
                opacity: 10,
                fill: true,
                rect: rect(0.1, 0.1, 0.2, 0.2),
            },
            // Index 5 in the stored list: slot 5 % 5 = 0, (0.76, 0.04).
            ImageLayer {
                path: PathBuf::from("/c.png"),
                opacity: 100,
                fill: false,
                rect: rect(0.76, 0.04, 0.20, 0.24),
            },
        ]
    );
    let off = store(
        tmp.path(),
        json!({"image_overlay_enabled": false, "image_overlays": [{"path": "/a.png"}]}),
    );
    assert!(finalize::image_layers(&off).is_empty());
}

#[test]
fn camera_rect_from_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let s = store(
        tmp.path(),
        json!({"camera_overlay_rect": {"x": 0.9, "y": 0.9, "w": 0.25, "h": 0.34}}),
    );
    assert_eq!(finalize::camera_rect(&s), rect(0.75, 0.66, 0.25, 0.34));
}

#[test]
fn pixels_truncate_and_floor() {
    assert_eq!(
        finalize::pixels(rect(0.72, 0.64, 0.25, 0.34), (1920, 1080)),
        (1382, 691, 480, 366)
    );
    // 64×48 minimum, even sizes, positions truncated.
    assert_eq!(
        finalize::pixels(rect(0.333, 0.5, 0.08, 0.08), (320, 180)),
        (106, 90, 64, 48)
    );
    assert_eq!(
        finalize::pixels(rect(0.0, 0.0, 0.5, 0.5), (1001, 501)),
        (0, 0, 500, 250)
    );
}

#[test]
fn graph_images_then_camera() {
    let layers = vec![
        ImageLayer {
            path: PathBuf::from("/img/logo's:1.png"),
            opacity: 72,
            fill: false,
            rect: rect(0.76, 0.04, 0.20, 0.24),
        },
        ImageLayer {
            path: PathBuf::from("/img/b.jpg"),
            opacity: 100,
            fill: true,
            rect: rect(0.04, 0.04, 0.24, 0.30),
        },
    ];
    let p = finalize::plan(
        &layers,
        Some(Path::new("/tmp/.fthr-finalize-x/camera.mkv")),
        finalize::CAMERA_DEFAULT,
        (1920, 1080),
    )
    .expect("something to burn");
    assert_eq!(
        p.filter_graph.as_deref(),
        Some(
            "movie=filename='/img/logo\\'s\\:1.png',format=rgba,scale=384:258:force_original_aspect_ratio=decrease,pad=384:258:(ow-iw)/2:(oh-ih)/2:color=0x00000000,colorchannelmixer=aa=0.720[image0];\
[0:v][image0]overlay=1459:43:eof_action=repeat[layer0];\
movie=filename='/img/b.jpg',format=rgba,scale=460:324:force_original_aspect_ratio=increase,crop=460:324,colorchannelmixer=aa=1.000[image1];\
[layer0][image1]overlay=76:43:eof_action=repeat[layer1];\
movie=filename='/tmp/.fthr-finalize-x/camera.mkv',scale=480:366:force_original_aspect_ratio=decrease,pad=480:366:(ow-iw)/2:(oh-ih)/2[cam];\
[layer1][cam]overlay=1382:691:eof_action=repeat[vout]"
        )
    );
    assert_eq!(p.video_map, MapRef::Label("vout".into()));
    assert_eq!(p.video, VideoOut::Encode);
    assert_eq!(p.audio_map, Some(MapRef::AllAudioOptional));
    assert_eq!(p.audio, AudioOut::Copy);
    assert_eq!((p.seek_s, p.duration_s, p.shortest), (None, None, false));
    // One image only: its overlay is the output.
    let one = finalize::plan(&layers[1..], None, finalize::CAMERA_DEFAULT, (1920, 1080)).unwrap();
    assert!(
        one.filter_graph
            .unwrap()
            .ends_with("[0:v][image0]overlay=76:43:eof_action=repeat[vout]")
    );
}

#[test]
fn nothing_to_burn_is_none() {
    assert!(finalize::plan(&[], None, finalize::CAMERA_DEFAULT, (1920, 1080)).is_none());
}

#[test]
fn gary_maths() {
    assert_eq!(gary::clamp_thresholds(55, 85), (55, 85));
    assert_eq!(gary::clamp_thresholds(0, 3), (1, 6));
    assert_eq!(gary::clamp_thresholds(99, 50), (95, 100));
    assert_eq!(gary::intensity_for_level(0.55, 55, 85), 0.0);
    assert_eq!(gary::intensity_for_level(0.85, 55, 85), 1.0);
    assert!((gary::intensity_for_level(0.70, 55, 85) - 0.5).abs() < 1e-9);
    assert_eq!(gary::intensity_for_level(f64::NAN, 55, 85), 0.0);
    // Steps land exactly when within a step.
    assert!((gary::step_intensity(0.0, 1.0) - 0.18).abs() < 1e-9);
    assert!((gary::step_intensity(1.0, 0.0) - 0.935).abs() < 1e-9);
    assert_eq!(gary::step_intensity(0.9, 1.0), 1.0);
    assert_eq!(gary::step_intensity(0.05, 0.0), 0.0);
}
