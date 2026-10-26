mod support;

use fthrust_media::vaapi::{
    gpu_convert_rgb, gpu_convert_rgb_with_stride, gpu_convert_rgb_with_upload, vaapi_available,
};
use support::{assert_rgb_near, solid};

/// Converts a `w`×`h` frame whose rows are padded to `stride` with 0xFF.
fn gpu_convert_rgb_strided(
    w: u32,
    h: u32,
    stride: usize,
    bgra: [u8; 4],
) -> fthrust_media::Result<Vec<u8>> {
    let mut buf = vec![0xffu8; stride * h as usize];
    for y in 0..h as usize {
        for x in 0..w as usize {
            buf[y * stride + x * 4..][..4].copy_from_slice(&bgra);
        }
    }
    gpu_convert_rgb_with_stride(w, h, w, h, stride, &buf)
}

#[test]
fn gpu_conversion_matches_bt709_limited_range() {
    if !vaapi_available() {
        eprintln!("skipping: no VA-API device");
        return;
    }
    for (bgra, rgb) in [
        ([0, 0, 0, 255], [0, 0, 0]),
        ([255, 255, 255, 255], [255, 255, 255]),
        ([0, 0, 255, 255], [255, 0, 0]),
        ([0x20, 0xe3, 0xa0, 255], [0xa0, 0xe3, 0x20]),
    ] {
        let frame = solid(64, 64, bgra);
        let out = gpu_convert_rgb(64, 64, 64, 64, &frame).unwrap();
        assert_rgb_near(&out[..3], rgb, 6);
    }
}

#[test]
fn gpu_conversion_scales_odd_source_to_output_size() {
    if !vaapi_available() {
        eprintln!("skipping: no VA-API device");
        return;
    }
    let frame = solid(1366, 768, [0, 0, 255, 255]);
    let out = gpu_convert_rgb(1366, 768, 1280, 720, &frame).unwrap();
    assert_eq!(out.len(), 1280 * 720 * 3);
    assert_rgb_near(&out[(360 * 1280 + 640) * 3..][..3], [255, 0, 0], 6);
}

#[test]
fn gpu_conversion_honours_stride_padding() {
    if !vaapi_available() {
        eprintln!("skipping: no VA-API device");
        return;
    }
    let out = gpu_convert_rgb_strided(64, 64, 64 * 4 + 256, [0, 0, 0, 255]).unwrap();
    assert_rgb_near(&out[(63 * 64 + 63) * 3..][..3], [0, 0, 0], 6);
}

#[test]
fn mapped_and_transfer_uploads_match() {
    if !vaapi_available() {
        eprintln!("skipping: no VA-API device");
        return;
    }
    let (w, h) = (200u32, 120u32);
    let stride = w as usize * 4 + 96;
    let mut buf = vec![0xabu8; stride * h as usize];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let p = &mut buf[y * stride + x * 4..][..4];
            p.copy_from_slice(&[(x * 3) as u8, (y * 5) as u8, (x + y) as u8, 255]);
        }
    }
    let mapped = gpu_convert_rgb_with_upload(w, h, w, h, stride, &buf, false).unwrap();
    let transfer = gpu_convert_rgb_with_upload(w, h, w, h, stride, &buf, true).unwrap();
    assert_eq!(mapped.len(), transfer.len());
    assert!(mapped == transfer, "mapped and transfer uploads differ");
}
