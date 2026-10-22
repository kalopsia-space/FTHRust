//! Shared helpers for the media integration tests.
#![allow(dead_code)]

/// Returns the first of `names` that is an available encoder, or `None`
/// after printing a skip notice (the reference tests skip the same way).
pub fn skip_unless_encoder(names: &[&'static str]) -> Option<&'static str> {
    fthrust_media::init().expect("ffmpeg init");
    let found = names
        .iter()
        .copied()
        .find(|name| ffmpeg_next::encoder::find_by_name(name).is_some());
    if found.is_none() {
        eprintln!("skipping: none of {names:?} is available in this FFmpeg build");
    }
    found
}

/// A BGRA frame whose gradient moves with `frame_index`, so consecutive
/// frames differ and encoders produce real motion.
pub fn synthetic_bgra(width: u32, height: u32, frame_index: u32) -> Vec<u8> {
    let mut out = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            let shift = frame_index.wrapping_mul(3);
            out[i] = (x.wrapping_add(shift) & 0xff) as u8; // B
            out[i + 1] = (y.wrapping_add(shift) & 0xff) as u8; // G
            out[i + 2] = ((x ^ y).wrapping_add(shift) & 0xff) as u8; // R
            out[i + 3] = 0xff;
        }
    }
    out
}
