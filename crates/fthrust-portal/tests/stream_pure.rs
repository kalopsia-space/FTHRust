use fthrust_portal::stream::{FrameLayout, choose_pts, copy_rows, monotonic};
use fthrust_wayland::frame::{BgraFrame, PixelLayout};

#[test]
fn copy_rows_swaps_rgb_and_drops_padding() {
    // Two 1-pixel rows, stride 8 (4 bytes padding), RGBx.
    let src = [
        10u8, 20, 30, 0, 99, 99, 99, 99, 40, 50, 60, 0, 99, 99, 99, 99,
    ];
    let layout = FrameLayout {
        width: 1,
        height: 2,
        stride: 8,
        layout: PixelLayout::Rgbx,
    };
    let mut out = BgraFrame::default();
    copy_rows(&src, &layout, 5, &mut out).unwrap();
    assert_eq!(
        (out.width, out.height, out.stride, out.timestamp_ns),
        (1, 2, 4, 5)
    );
    assert_eq!(out.data, [30, 20, 10, 255, 60, 50, 40, 255]);
    let layout = FrameLayout {
        layout: PixelLayout::Bgra,
        ..layout
    };
    copy_rows(&src, &layout, 6, &mut out).unwrap();
    assert_eq!(out.data, [10, 20, 30, 0, 40, 50, 60, 0]);
    assert!(
        copy_rows(&src[..9], &layout, 7, &mut out).is_err(),
        "short buffer"
    );
}

#[test]
fn pts_rule_and_monotonic() {
    let now = 10_000_000_000i64;
    assert_eq!(
        choose_pts(Some(now - 500_000_000), now),
        now - 500_000_000,
        "within 1 s: header pts"
    );
    assert_eq!(
        choose_pts(Some(now - 1_500_000_000), now),
        now,
        "older than 1 s: now"
    );
    assert_eq!(
        choose_pts(Some(now + 2_000_000_000), now),
        now,
        "future: now"
    );
    assert_eq!(choose_pts(None, now), now);
    assert_eq!(monotonic(100, 50), 101);
    assert_eq!(monotonic(100, 100), 101);
    assert_eq!(monotonic(100, 200), 200);
}
