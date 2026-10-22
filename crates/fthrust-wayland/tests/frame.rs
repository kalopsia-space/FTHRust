use fthrust_wayland::frame::{
    BgraFrame, FrameError, FrameView, OutputTransform, PixelLayout, normalize, preferred_wl_shm,
};

// wl_shm_format values (wayland.xml).
const ARGB8888: u32 = 0;
const XRGB8888: u32 = 1;
const XBGR8888: u32 = 0x3432_4258; // 'XB24'
const ABGR8888: u32 = 0x3432_4241; // 'AB24'
const RGBX8888: u32 = 0x3432_5852; // 'RX24'
const RGBA8888: u32 = 0x3432_4152; // 'RA24'
const BGRX8888: u32 = 0x3432_5842; // 'BX24'
const BGRA8888: u32 = 0x3432_4142; // 'BA24'
const XRGB2101010: u32 = 0x3033_5258; // 'XR30'
const XBGR2101010: u32 = 0x3033_4258; // 'XB30'

fn view<'a>(data: &'a [u8], w: u32, h: u32, layout: PixelLayout) -> FrameView<'a> {
    FrameView {
        data,
        stride: w as usize * 4,
        width: w,
        height: h,
        layout,
        y_invert: false,
        transform: OutputTransform::Normal,
        timestamp_ns: 7,
    }
}

#[test]
fn mapping_table_and_preference() {
    assert_eq!(PixelLayout::from_wl_shm(ARGB8888), Some(PixelLayout::Bgra));
    assert_eq!(PixelLayout::from_wl_shm(XRGB8888), Some(PixelLayout::Bgrx));
    assert_eq!(PixelLayout::from_wl_shm(ABGR8888), Some(PixelLayout::Rgba));
    assert_eq!(PixelLayout::from_wl_shm(XBGR8888), Some(PixelLayout::Rgbx));
    assert_eq!(PixelLayout::from_wl_shm(RGBA8888), Some(PixelLayout::Abgr));
    assert_eq!(PixelLayout::from_wl_shm(RGBX8888), Some(PixelLayout::Xbgr));
    assert_eq!(PixelLayout::from_wl_shm(BGRA8888), Some(PixelLayout::Argb));
    assert_eq!(PixelLayout::from_wl_shm(BGRX8888), Some(PixelLayout::Xrgb));
    assert_eq!(
        PixelLayout::from_wl_shm(XRGB2101010),
        Some(PixelLayout::Xrgb2101010)
    );
    assert_eq!(
        PixelLayout::from_wl_shm(XBGR2101010),
        Some(PixelLayout::Xbgr2101010)
    );
    assert_eq!(PixelLayout::from_wl_shm(0x1234_5678), None);
    assert_eq!(
        preferred_wl_shm(&[RGBA8888, ABGR8888, XRGB8888]),
        Some(XRGB8888)
    );
    assert_eq!(preferred_wl_shm(&[RGBA8888, ABGR8888]), Some(ABGR8888));
    assert_eq!(preferred_wl_shm(&[XBGR2101010, BGRA8888]), Some(BGRA8888));
    assert_eq!(preferred_wl_shm(&[0x1234_5678]), None);
    assert_eq!(OutputTransform::from_wl(0), Some(OutputTransform::Normal));
    assert_eq!(OutputTransform::from_wl(1), Some(OutputTransform::Rot90));
    assert_eq!(
        OutputTransform::from_wl(7),
        Some(OutputTransform::Flipped270)
    );
    assert_eq!(OutputTransform::from_wl(8), None);
}

#[test]
fn bgra_passthrough_is_exact_and_rgba_is_swizzled() {
    let px = [10u8, 20, 30, 255, 40, 50, 60, 255];
    let mut out = BgraFrame::default();
    normalize(&view(&px, 2, 1, PixelLayout::Bgra), &mut out).unwrap();
    assert_eq!(
        (out.width, out.height, out.stride, out.timestamp_ns),
        (2, 1, 8, 7)
    );
    assert_eq!(out.data, px);
    normalize(&view(&px, 2, 1, PixelLayout::Rgba), &mut out).unwrap();
    assert_eq!(out.data, [30, 20, 10, 255, 60, 50, 40, 255]);
    normalize(&view(&px, 2, 1, PixelLayout::Argb), &mut out).unwrap();
    // ARGB bytes [A,R,G,B] -> BGRA [B,G,R,A]
    assert_eq!(out.data, [255, 30, 20, 10, 255, 60, 50, 40]);
    normalize(&view(&px, 2, 1, PixelLayout::Bgrx), &mut out).unwrap();
    assert_eq!(
        out.data,
        [10, 20, 30, 255, 40, 50, 60, 255],
        "X becomes opaque"
    );
}

#[test]
fn padded_stride_is_compacted() {
    let mut data = vec![0u8; 16];
    data[..4].copy_from_slice(&[1, 2, 3, 4]);
    data[8..12].copy_from_slice(&[5, 6, 7, 8]);
    let v = FrameView {
        stride: 8,
        ..view(&data, 1, 2, PixelLayout::Bgra)
    };
    let mut out = BgraFrame::default();
    normalize(&v, &mut out).unwrap();
    assert_eq!(
        (out.stride, out.data.as_slice()),
        (4, &[1u8, 2, 3, 4, 5, 6, 7, 8][..])
    );
}

#[test]
fn y_invert_flips_rows() {
    let px = [1u8, 1, 1, 255, 2, 2, 2, 255];
    let v = FrameView {
        y_invert: true,
        ..view(&px, 1, 2, PixelLayout::Bgra)
    };
    let mut out = BgraFrame::default();
    normalize(&v, &mut out).unwrap();
    assert_eq!(out.data, [2, 2, 2, 255, 1, 1, 1, 255]);
}

#[test]
fn ten_bit_is_truncated() {
    // XRGB2101010 little-endian: bits 0..10 B, 10..20 G, 20..30 R.
    let word: u32 = 0x3FF << 20; // full red
    let px = word.to_le_bytes();
    let mut out = BgraFrame::default();
    normalize(&view(&px, 1, 1, PixelLayout::Xrgb2101010), &mut out).unwrap();
    assert_eq!(out.data, [0, 0, 255, 255]);
    let word: u32 = 0x200 << 10; // half green
    normalize(
        &view(&word.to_le_bytes(), 1, 1, PixelLayout::Xrgb2101010),
        &mut out,
    )
    .unwrap();
    assert_eq!(out.data, [0, 128, 0, 255]);
    let word: u32 = 0x3FF; // XBGR: bits 0..10 R
    normalize(
        &view(&word.to_le_bytes(), 1, 1, PixelLayout::Xbgr2101010),
        &mut out,
    )
    .unwrap();
    assert_eq!(out.data, [0, 0, 255, 255]);
}

#[test]
fn rotations_and_flips() {
    // 2 wide, 3 tall; pixel value = index.
    let px: Vec<u8> = (0..6u8).flat_map(|i| [i, i, i, 255]).collect();
    let at = |out: &BgraFrame, x: u32, y: u32| out.data[(y as usize * out.stride) + x as usize * 4];
    let run = |t: OutputTransform| {
        let v = FrameView {
            transform: t,
            ..view(&px, 2, 3, PixelLayout::Bgra)
        };
        let mut out = BgraFrame::default();
        normalize(&v, &mut out).unwrap();
        out
    };
    let o = run(OutputTransform::Rot90);
    assert_eq!((o.width, o.height), (3, 2));
    // The output was rotated 90° (counter-clockwise, as wl_output.transform
    // describes); un-rotating puts source (x,y) at (h-1-y, x).
    assert_eq!(at(&o, 2, 0), 0);
    assert_eq!(at(&o, 2, 1), 1);
    assert_eq!(at(&o, 0, 0), 4);
    let o = run(OutputTransform::Rot180);
    assert_eq!((o.width, o.height), (2, 3));
    assert_eq!(at(&o, 0, 0), 5);
    assert_eq!(at(&o, 1, 2), 0);
    let o = run(OutputTransform::Rot270);
    assert_eq!((o.width, o.height), (3, 2));
    assert_eq!(at(&o, 0, 1), 0);
    assert_eq!(at(&o, 0, 0), 1);
    let o = run(OutputTransform::Flipped);
    assert_eq!(at(&o, 0, 0), 1);
    assert_eq!(at(&o, 1, 0), 0);
    let o = run(OutputTransform::Flipped180);
    assert_eq!(at(&o, 0, 0), 4, "flipped180 is a vertical flip");
    assert_eq!(at(&o, 1, 2), 1);
}

#[test]
fn rejects_bad_input() {
    let px = [0u8; 8];
    let mut out = BgraFrame::default();
    assert_eq!(
        normalize(&view(&px, 0, 1, PixelLayout::Bgra), &mut out),
        Err(FrameError::ZeroSize)
    );
    let v = FrameView {
        stride: 4,
        ..view(&px, 2, 1, PixelLayout::Bgra)
    };
    assert_eq!(normalize(&v, &mut out), Err(FrameError::ShortStride));
    assert_eq!(
        normalize(&view(&px[..7], 2, 1, PixelLayout::Bgra), &mut out),
        Err(FrameError::ShortBuffer)
    );
}
