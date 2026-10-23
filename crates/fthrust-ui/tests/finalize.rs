//! Post-save overlays (notes-overlays-reference sections 8, 17, 18).

#[path = "../../fthrust-media/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fthrust_core::audio_manifest::{
    AudioManifest, AudioSource, MANIFEST_VERSION, SourceType, read_verified, sha256_file,
};
use fthrust_core::finalize::{CAMERA_DEFAULT, ImageLayer, Rect};
use fthrust_media::camera::{CameraRecorder, Source};
use fthrust_ui::services::finalize::{self, CameraUse, FinalizeJob, Finalizer, Outcome};
use fthrust_ui::state::AppState;

fn clip(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    support::write_test_clip(&p, 2, true)?;
    let manifest = AudioManifest {
        manifest_version: MANIFEST_VERSION,
        transaction_id: "00000000-0000-4000-8000-000000000001".into(),
        media_file: name.into(),
        media_sha256: sha256_file(&p).unwrap(),
        sources: vec![AudioSource {
            source_uuid: "11111111-1111-4111-8111-111111111111".into(),
            stream_index: 1,
            source_type: SourceType::Application,
            display_name: "Game".into(),
            persistent_identity: "Game".into(),
            icon_reference: None,
            first_active_100ns: 0,
            last_active_100ns: 20_000_000,
            sample_rate: 48_000,
            channels: 2,
            sample_format: "f32".into(),
        }],
    };
    manifest.write(&p).unwrap();
    Some(p)
}

fn png(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 0, 0, 255]))
        .save(&p)
        .unwrap();
    p
}

fn layer(path: PathBuf) -> ImageLayer {
    ImageLayer {
        path,
        opacity: 100,
        fill: true,
        rect: Rect {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        },
    }
}

fn job(clip: &Path, layers: Vec<ImageLayer>, camera: CameraUse) -> FinalizeJob {
    FinalizeJob {
        clip: clip.to_path_buf(),
        end: Instant::now(),
        duration_s: 2.0,
        layers,
        camera,
        camera_rect: CAMERA_DEFAULT,
    }
}

fn wait(f: &Finalizer, n: usize) -> Vec<Outcome> {
    let mut out = Vec::new();
    let end = Instant::now() + Duration::from_secs(60);
    while out.len() < n && Instant::now() < end {
        out.extend(f.poll());
        std::thread::sleep(Duration::from_millis(20));
    }
    out
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".fthr-finalize-") || n.contains(".partial."))
        .collect()
}

#[test]
fn image_layer_burns_and_rebinds_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(c) = clip(tmp.path(), "a.mp4") else {
        return;
    };
    let before = sha256_file(&c).unwrap();
    let f = Finalizer::new();
    f.submit(job(
        &c,
        vec![layer(png(tmp.path(), "red.png"))],
        CameraUse::Off,
    ));
    let out = wait(&f, 1);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].clip, c);
    assert!(out[0].warnings.is_empty(), "{:?}", out[0].warnings);
    assert_ne!(sha256_file(&c).unwrap(), before, "the clip was rewritten");
    let m = read_verified(&c)
        .unwrap()
        .expect("manifest still binds the clip");
    assert_eq!(m.media_sha256, sha256_file(&c).unwrap());
    assert!(
        leftovers(tmp.path()).is_empty(),
        "{:?}",
        leftovers(tmp.path())
    );
}

#[test]
fn nothing_to_burn_leaves_the_clip() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(c) = clip(tmp.path(), "n.mp4") else {
        return;
    };
    let before = sha256_file(&c).unwrap();
    let f = Finalizer::new();
    f.submit(job(&c, vec![], CameraUse::Off));
    let out = wait(&f, 1);
    assert!(out[0].warnings.is_empty());
    assert_eq!(sha256_file(&c).unwrap(), before);
}

#[test]
fn missing_and_unreadable_images_are_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(c) = clip(tmp.path(), "m.mp4") else {
        return;
    };
    let before = sha256_file(&c).unwrap();
    let bad = tmp.path().join("bad.png");
    std::fs::write(&bad, b"not an image at all").unwrap();
    let f = Finalizer::new();
    f.submit(job(
        &c,
        vec![
            layer(tmp.path().join("missing.png")),
            layer(bad),
            layer(png(tmp.path(), "ok.png")),
        ],
        CameraUse::Off,
    ));
    let out = wait(&f, 1);
    assert_eq!(
        out[0].warnings,
        [
            "Image overlay source is missing: missing.png",
            "Image overlay could not be read: bad.png"
        ]
    );
    assert_ne!(sha256_file(&c).unwrap(), before, "the good layer burned");
}

#[test]
fn camera_unavailable_records_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(c) = clip(tmp.path(), "u.mp4") else {
        return;
    };
    let before = sha256_file(&c).unwrap();
    let f = Finalizer::new();
    f.submit(job(&c, vec![], CameraUse::Unavailable));
    let out = wait(&f, 1);
    assert_eq!(
        out[0].warnings,
        ["Camera overlay was requested but the camera was unavailable."]
    );
    assert_eq!(sha256_file(&c).unwrap(), before);
}

#[test]
fn camera_burns_from_the_ring() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(c) = clip(tmp.path(), "cam.mp4") else {
        return;
    };
    let before = sha256_file(&c).unwrap();
    let rec = Arc::new(CameraRecorder::start(Source::synthetic(160, 120, 15)).unwrap());
    std::thread::sleep(Duration::from_millis(2300));
    let f = Finalizer::new();
    f.submit(job(&c, vec![], CameraUse::Recorder(rec.clone())));
    let out = wait(&f, 1);
    rec.stop();
    assert!(out[0].warnings.is_empty(), "{:?}", out[0].warnings);
    assert_ne!(sha256_file(&c).unwrap(), before);
    assert!(leftovers(tmp.path()).is_empty());
}

#[test]
fn saves_finalize_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let (Some(a), Some(b)) = (clip(tmp.path(), "1.mp4"), clip(tmp.path(), "2.mp4")) else {
        return;
    };
    let red = png(tmp.path(), "red.png");
    let f = Finalizer::new();
    f.submit(job(&a, vec![layer(red.clone())], CameraUse::Off));
    f.submit(job(&b, vec![layer(red)], CameraUse::Off));
    let out = wait(&f, 2);
    let order: Vec<_> = out.iter().map(|o| o.clip.clone()).collect();
    assert_eq!(order, [a, b]);
}

#[test]
fn shutdown_mid_finalize_keeps_a_whole_clip() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(c) = clip(tmp.path(), "q.mp4") else {
        return;
    };
    let mut f = Finalizer::new();
    f.submit(job(
        &c,
        vec![layer(png(tmp.path(), "red.png"))],
        CameraUse::Off,
    ));
    std::thread::sleep(Duration::from_millis(30));
    let started = Instant::now();
    f.shutdown(Duration::from_secs(5));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        leftovers(tmp.path()).is_empty(),
        "{:?}",
        leftovers(tmp.path())
    );
    // Either the base clip or the finished rewrite, never a partial file.
    assert!(read_verified(&c).unwrap().is_some());
}

#[test]
fn last_warning_is_shown_and_the_clip_is_ready() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    let c = tmp.path().join("w.mp4");
    std::fs::write(&c, b"x").unwrap();
    s.readiness.engine_committed(&c, true);
    assert!(!s.readiness.can_access(&c));
    finalize::apply(
        &mut s,
        &Outcome {
            clip: c.clone(),
            warnings: vec![
                "Image overlay source is missing: a.png".into(),
                "Camera overlay was requested but the camera was unavailable.".into(),
            ],
            camera_failed: false,
        },
    );
    assert!(s.readiness.can_access(&c));
    assert_eq!(s.readiness.warnings(&c).len(), 2);
    let bar = s.error_bar.current().expect("a warning");
    assert!(
        bar.title.contains("CLIP SAVED WITH WARNING"),
        "{}",
        bar.title
    );
    assert_eq!(
        bar.detail,
        "Camera overlay was requested but the camera was unavailable."
    );
}

#[test]
fn needed_follows_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    assert!(!finalize::needed(&s));
    let s = AppState::for_tests(tmp.path(), serde_json::json!({"camera_enabled": true}));
    assert!(finalize::needed(&s));
    let s = AppState::for_tests(
        tmp.path(),
        serde_json::json!({"image_overlay_enabled": true, "image_overlays": [{"path": "/x.png"}]}),
    );
    assert!(finalize::needed(&s));
}

// ---------------------------------------------------------------------------
// The camera service and the library hand-off.

use fthrust_media::camera::Device;
use fthrust_ui::services::camera::CameraService;

fn devices(names: &[&str]) -> Vec<Device> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| Device {
            path: PathBuf::from(format!("/dev/video{}", i * 2)),
            name: (*n).into(),
        })
        .collect()
}

fn tick_until(
    svc: &mut CameraService,
    s: &mut AppState,
    f: impl Fn(&CameraService, &AppState) -> bool,
) -> bool {
    for i in 0..400 {
        svc.tick(s, i as f64 * 0.02);
        if f(svc, s) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn camera_open_failure_turns_it_off() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({"camera_enabled": true}));
    let mut svc = CameraService::for_tests(
        Arc::new(|| devices(&["Groov-e USB"])),
        Arc::new(|_| Source::v4l2("/dev/fthrust-no-such-camera".as_ref())),
    );
    svc.refresh();
    assert!(tick_until(&mut svc, &mut s, |_, s| !s.flag("camera_enabled", true)));
    assert_eq!(s.camera_devices, ["Groov-e USB"]);
    let bar = s.error_bar.current().expect("CAMERA UNAVAILABLE");
    assert!(bar.title.contains("CAMERA UNAVAILABLE"), "{}", bar.title);
    assert!(
        bar.detail.starts_with("Could not open camera device 0. Confirm it is connected and not in use by another app. The camera overlay was turned off; choose another camera or try again."),
        "{}",
        bar.detail
    );
    assert!(bar.detail.chars().count() <= 240);
    assert_eq!(bar.actions[0].label, "OPEN CAMERA SETTINGS");
    assert!(matches!(svc.for_save(&s), CameraUse::Off));
}

#[test]
fn camera_records_and_feeds_the_preview() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({"camera_enabled": true}));
    s.page = fthrust_ui::state::Page::Settings;
    s.settings_tab = fthrust_ui::state::Tab::Visuals;
    let mut svc = CameraService::for_tests(
        Arc::new(|| devices(&["Cam A", "Cam B"])),
        Arc::new(|_| Source::synthetic(160, 120, 30)),
    );
    svc.refresh();
    assert!(tick_until(&mut svc, &mut s, |svc, s| {
        matches!(svc.for_save(s), CameraUse::Recorder(_)) && s.camera_preview.is_some()
    }));
    let [w, h] = s.camera_preview.as_ref().unwrap().1.size;
    assert_eq!((w, h), (160, 120));
    // Off: the recorder stops.
    s.settings.set("camera_enabled", serde_json::json!(false));
    svc.tick(&mut s, 100.0);
    assert!(matches!(svc.for_save(&s), CameraUse::Off));
    svc.shutdown();
}

#[test]
fn refresh_keeps_selection_and_running_device() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(
        tmp.path(),
        serde_json::json!({"camera_enabled": true, "camera_device_index": 1}),
    );
    let lists = Arc::new(std::sync::Mutex::new(vec![
        devices(&["A", "B"]),
        devices(&["A", "B", "C"]),
    ]));
    let l = lists.clone();
    let mut svc = CameraService::for_tests(
        Arc::new(move || {
            let mut v = l.lock().unwrap();
            if v.len() > 1 {
                v.remove(0)
            } else {
                v[0].clone()
            }
        }),
        Arc::new(|_| Source::synthetic(160, 120, 30)),
    );
    svc.refresh();
    let mut first = None;
    assert!(tick_until(&mut svc, &mut s, |svc, s| {
        match svc.for_save(s) {
            CameraUse::Recorder(r) => r.is_running(),
            _ => false,
        }
    }));
    if let CameraUse::Recorder(r) = svc.for_save(&s) {
        first = Some(r);
    }
    svc.refresh();
    assert!(tick_until(&mut svc, &mut s, |_, s| s.camera_devices.len() == 3));
    assert_eq!(s.settings.get_i64("camera_device_index"), Some(1));
    let CameraUse::Recorder(now) = svc.for_save(&s) else {
        panic!("still recording");
    };
    assert!(Arc::ptr_eq(&now, first.as_ref().unwrap()), "not reopened");
    svc.shutdown();
}

#[test]
fn opening_a_finalizing_clip_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    let c = tmp.path().join("f.mp4");
    std::fs::write(&c, b"x").unwrap();
    s.readiness.engine_committed(&c, true);
    assert!(!fthrust_ui::editor::sessions::open(&mut s, &c));
    let bar = s.error_bar.current().unwrap();
    assert!(bar.title.contains("CLIP STILL FINALIZING"), "{}", bar.title);
    assert_eq!(
        bar.detail,
        "Playback, editing and upload become available after optional processing has finished."
    );
    assert!(s.editor.is_none());
}

#[test]
fn a_finalized_clip_gets_a_new_thumbnail() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    let c = tmp.path().join("t.mp4");
    std::fs::write(&c, b"x").unwrap();
    s.readiness.engine_committed(&c, true);
    s.thumb_meta.insert(c.clone(), Default::default());
    finalize::apply(
        &mut s,
        &Outcome {
            clip: c.clone(),
            warnings: vec![],
            camera_failed: false,
        },
    );
    assert!(
        !s.thumb_meta.contains_key(&c),
        "the pre-burn thumbnail is dropped"
    );
}
