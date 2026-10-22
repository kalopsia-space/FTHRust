//! Poster frames for the library grid. Ports the reference's ffmpeg call
//! `-ss 0 -frames:v 1 -vf scale=w='min(640,iw)':h='min(360,ih)':
//! force_original_aspect_ratio=decrease`, in-process.

use std::path::Path;

use fast_image_resize::images::Image;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use ffmpeg_next::format::Pixel;
use ffmpeg_next::media::Type;
use ffmpeg_next::software::scaling::{Context as Scaler, Flags};
use ffmpeg_next::{codec, format, frame};
use image::RgbaImage;

use crate::{MediaError, Result};

/// Poster width bound.
pub const POSTER_MAX_WIDTH: u32 = 640;
/// Poster height bound.
pub const POSTER_MAX_HEIGHT: u32 = 360;

/// The size the scale filter produces: shrink to fit inside
/// `min(max_w, w)` × `min(max_h, h)` keeping aspect, never enlarge.
pub fn fit_within(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (1, 1);
    }
    let target_w = max_width.min(width).max(1);
    let target_h = max_height.min(height).max(1);
    let scale =
        (f64::from(target_w) / f64::from(width)).min(f64::from(target_h) / f64::from(height));
    if scale >= 1.0 {
        return (width, height);
    }
    let w = (f64::from(width) * scale).round().max(1.0) as u32;
    let h = (f64::from(height) * scale).round().max(1.0) as u32;
    (w, h)
}

/// The library poster: the first frame, fitted into 640×360.
pub fn poster(path: &Path) -> Result<RgbaImage> {
    extract_frame(path, 0.0, POSTER_MAX_WIDTH, POSTER_MAX_HEIGHT)
}

/// Decodes the first frame presented at or after `at_seconds` (seeking to
/// the keyframe before it) and fits it inside `max_width` × `max_height`.
/// Past the end of the stream the last decodable frame is returned.
pub fn extract_frame(
    path: &Path,
    at_seconds: f64,
    max_width: u32,
    max_height: u32,
) -> Result<RgbaImage> {
    crate::init()?;
    let mut ictx = format::input(path)?;
    let stream = ictx
        .streams()
        .best(Type::Video)
        .ok_or_else(|| MediaError::Invalid("no video stream".into()))?;
    let index = stream.index();
    let tb = stream.time_base();
    let tb_seconds = f64::from(tb.numerator()) / f64::from(tb.denominator());
    let ctx = codec::context::Context::from_parameters(stream.parameters())?;
    let mut decoder = ctx.decoder().video()?;

    let at = if at_seconds.is_finite() {
        at_seconds.max(0.0)
    } else {
        0.0
    };
    if at > 0.0 {
        let ts = (at * f64::from(ffmpeg_next::ffi::AV_TIME_BASE)) as i64;
        if let Err(e) = ictx.seek(ts, ..ts.saturating_add(1)) {
            tracing::debug!(path = %path.display(), "seek failed, decoding from the start: {e}");
        }
    }

    let mut decoded = frame::Video::empty();
    let mut rgba: Option<frame::Video> = None;
    let mut scaler: Option<Scaler> = None;
    let mut reached = false;
    let convert = |decoded: &frame::Video,
                   rgba: &mut Option<frame::Video>,
                   scaler: &mut Option<Scaler>|
     -> Result<()> {
        let (w, h) = (decoded.width(), decoded.height());
        let needs_new = scaler.as_ref().is_none_or(|s| {
            s.input().width != w || s.input().height != h || s.input().format != decoded.format()
        });
        if needs_new {
            *scaler = Some(Scaler::get(
                decoded.format(),
                w,
                h,
                Pixel::RGBA,
                w,
                h,
                Flags::BILINEAR,
            )?);
        }
        let out = rgba.get_or_insert_with(|| frame::Video::new(Pixel::RGBA, w, h));
        if out.width() != w || out.height() != h {
            *out = frame::Video::new(Pixel::RGBA, w, h);
        }
        scaler
            .as_mut()
            .expect("scaler just created")
            .run(decoded, out)?;
        Ok(())
    };

    'packets: for (s, packet) in ictx.packets() {
        if s.index() != index {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        while decoder.receive_frame(&mut decoded).is_ok() {
            convert(&decoded, &mut rgba, &mut scaler)?;
            let pts = decoded.pts().or(decoded.timestamp());
            let t = pts.map_or(0.0, |p| p as f64 * tb_seconds);
            if t >= at {
                reached = true;
                break 'packets;
            }
        }
    }
    if !reached {
        let _ = decoder.send_eof();
        while decoder.receive_frame(&mut decoded).is_ok() {
            convert(&decoded, &mut rgba, &mut scaler)?;
        }
    }
    let rgba = rgba.ok_or_else(|| MediaError::Invalid("no decodable video frame".into()))?;
    let (w, h) = (rgba.width(), rgba.height());
    let stride = rgba.stride(0);
    let row = w as usize * 4;
    let mut pixels = Vec::with_capacity(row * h as usize);
    for y in 0..h as usize {
        pixels.extend_from_slice(&rgba.data(0)[y * stride..y * stride + row]);
    }
    let (dw, dh) = fit_within(w, h, max_width, max_height);
    if (dw, dh) == (w, h) {
        return RgbaImage::from_raw(w, h, pixels)
            .ok_or_else(|| MediaError::Invalid("frame buffer size".into()));
    }
    let src = Image::from_vec_u8(w, h, pixels, PixelType::U8x4)
        .map_err(|e| MediaError::Invalid(e.to_string()))?;
    let mut dst = Image::new(dw, dh, PixelType::U8x4);
    Resizer::new()
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
        )
        .map_err(|e| MediaError::Invalid(e.to_string()))?;
    RgbaImage::from_raw(dw, dh, dst.into_vec())
        .ok_or_else(|| MediaError::Invalid("resized buffer size".into()))
}
