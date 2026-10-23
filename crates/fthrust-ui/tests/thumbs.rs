//! Thumbnail results wake the UI.

use std::time::Duration;

use fthrust_ui::library::thumbs::ThumbPool;

#[test]
fn thumbnail_results_request_a_repaint() {
    let tmp = tempfile::tempdir().unwrap();
    let png = tmp.path().join("shot.png");
    image::RgbaImage::from_pixel(32, 32, image::Rgba([9, 8, 7, 255]))
        .save(&png)
        .unwrap();
    let ctx = egui::Context::default();
    let pool = ThumbPool::start(tmp.path().join("cache"));
    pool.set_repaint(ctx.clone());
    // A fresh context may already carry egui's initial repaint request:
    // run one pass so the flag reflects only the worker's wake-up.
    let mut out = ctx.run_ui(egui::RawInput::default(), |_ui| {});
    out.textures_delta.clear();
    assert!(pool.request(&png, "fp1", false));
    let result = pool
        .results()
        .recv_timeout(Duration::from_secs(10))
        .expect("a result");
    assert_eq!(result.path, png);
    assert!(ctx.has_requested_repaint(), "the worker woke the UI");
}
