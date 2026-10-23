//! Plan 5 Task 12: screenshots through the Screenshot portal.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fthrust_ui::services::screenshot::{PortalScreenshot, ScreenshotFailure, local_path_from_uri};
use fthrust_ui::services::{CardKind, Services};
use fthrust_ui::state::{AppState, DialogKind};

fn state() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.tokens.reduce_motion = true;
    (tmp, s)
}

struct FakePortal {
    results: Mutex<Vec<Result<PathBuf, ScreenshotFailure>>>,
    calls: Mutex<usize>,
}

impl PortalScreenshot for FakePortal {
    fn take(&self) -> Result<PathBuf, ScreenshotFailure> {
        *self.calls.lock().unwrap() += 1;
        let mut r = self.results.lock().unwrap();
        if r.is_empty() {
            Err(ScreenshotFailure {
                code: "PORTAL_UNAVAILABLE",
                detail: "no more results".into(),
            })
        } else {
            r.remove(0)
        }
    }
}

fn png(dir: &std::path::Path, name: &str, w: u32, h: u32) -> PathBuf {
    let p = dir.join(name);
    image::RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]))
        .save(&p)
        .unwrap();
    p
}

fn settle(services: &mut Services, s: &mut AppState) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while services.screenshot_inflight() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        services.tick(s);
    }
    services.tick(s);
}

#[test]
fn screenshot_saves_png_and_refreshes_library() {
    let (tmp, mut s) = state();
    let src = png(tmp.path(), "portal.png", 64, 64);
    let portal = Arc::new(FakePortal {
        results: Mutex::new(vec![Ok(src)]),
        calls: Mutex::new(0),
    });
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.set_screenshot_portal_for_tests(portal.clone());
    s.screenshot_requested = true;
    services.tick(&mut s);
    settle(&mut services, &mut s);
    let dir = s.library.clips_root.join("Screenshots");
    let files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("screenshot_from_") && name.ends_with(".png"),
        "{name}"
    );
    assert!(!name.contains(".partial"));
    assert_eq!(image::open(&files[0]).unwrap().width(), 64);
    assert!(
        s.library.records.iter().any(|r| r.path == files[0]),
        "upserted into the library"
    );
    assert!(s.readiness.can_access(&files[0]));
    assert!(services.cards_for_tests().contains(&CardKind::Screenshot));
    assert!(
        s.pending_sounds
            .contains(&"screenshot_captured".to_string())
    );
    assert!(s.error_bar.entries().is_empty());
    assert_eq!(*portal.calls.lock().unwrap(), 1);
}

#[test]
fn screenshot_rejects_oversized_png() {
    let (tmp, mut s) = state();
    let src = png(tmp.path(), "big.png", 16385, 2);
    let portal = Arc::new(FakePortal {
        results: Mutex::new(vec![Ok(src)]),
        calls: Mutex::new(0),
    });
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.set_screenshot_portal_for_tests(portal);
    s.screenshot_requested = true;
    services.tick(&mut s);
    settle(&mut services, &mut s);
    let titles: Vec<String> = s
        .error_bar
        .entries()
        .iter()
        .map(|e| e.title.clone())
        .collect();
    assert_eq!(titles, vec!["SCREENSHOT FAILED".to_string()]);
    assert!(
        s.error_bar
            .current()
            .unwrap()
            .detail
            .starts_with("INVALID_IMAGE: ")
    );
    assert!(
        matches!(s.dialogs.modal.front(), Some(DialogKind::Message { title, .. }) if title == "Screenshot Failed")
    );
    let dir = s.library.clips_root.join("Screenshots");
    assert!(
        std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0) == 0,
        "no partial left behind"
    );
}

#[test]
fn screenshot_queue_caps_at_ten() {
    let (tmp, mut s) = state();
    let results: Vec<Result<PathBuf, ScreenshotFailure>> = (0..12)
        .map(|i| Ok(png(tmp.path(), &format!("p{i}.png"), 8, 8)))
        .collect();
    let portal = Arc::new(FakePortal {
        results: Mutex::new(results),
        calls: Mutex::new(0),
    });
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.set_screenshot_portal_for_tests(portal.clone());
    services.hold_screenshot_portal_for_tests(true);
    for _ in 0..15 {
        s.screenshot_requested = true;
        services.tick(&mut s);
    }
    assert_eq!(
        services.screenshot_pending(),
        9,
        "one in flight plus nine queued"
    );
    services.hold_screenshot_portal_for_tests(false);
    settle(&mut services, &mut s);
    let deadline = Instant::now() + Duration::from_secs(10);
    while *portal.calls.lock().unwrap() < 10 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        services.tick(&mut s);
    }
    settle(&mut services, &mut s);
    assert_eq!(*portal.calls.lock().unwrap(), 10);
    let dir = s.library.clips_root.join("Screenshots");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 10);
}

#[test]
fn screenshot_cancel_shows_dialog_and_error() {
    let (_tmp, mut s) = state();
    let portal = Arc::new(FakePortal {
        results: Mutex::new(vec![Err(ScreenshotFailure {
            code: "CANCELLED",
            detail: "Screenshot was canceled in the compositor dialog.".into(),
        })]),
        calls: Mutex::new(0),
    });
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.set_screenshot_portal_for_tests(portal);
    s.screenshot_requested = true;
    services.tick(&mut s);
    settle(&mut services, &mut s);
    assert_eq!(
        s.error_bar.current().unwrap().detail,
        "CANCELLED: Screenshot was canceled in the compositor dialog."
    );
    assert!(
        matches!(s.dialogs.modal.front(), Some(DialogKind::Message { message, .. }) if message == "CANCELLED\n\nScreenshot was canceled in the compositor dialog.")
    );
    assert!(services.cards_for_tests().is_empty());
    assert!(
        local_path_from_uri("file:///tmp/a%20b.png")
            .is_ok_and(|p| p.as_path() == std::path::Path::new("/tmp/a b.png"))
    );
    assert!(local_path_from_uri("https://example/x.png").is_err());
}

#[test]
#[ignore = "needs a desktop portal with a Screenshot backend"]
fn screenshot_live_portal() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start(&s, &ctx);
    s.screenshot_requested = true;
    services.tick(&mut s);
    let deadline = Instant::now() + Duration::from_secs(70);
    while services.screenshot_inflight() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        services.tick(&mut s);
    }
    let dir = s.library.clips_root.join("Screenshots");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    services.shutdown();
}
