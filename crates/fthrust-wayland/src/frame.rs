//! The raw-frame model and its normalisation to top-down BGRA. Ports
//! `shm_frame.cpp`, plus output transforms (the reference ignored them).

use wayland_client::protocol::wl_shm;

/// Byte order of a 32-bit pixel as delivered by the compositor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelLayout {
    /// `wl_shm` ARGB8888 (little-endian word: bytes B, G, R, A).
    Bgra,
    /// `wl_shm` XRGB8888.
    Bgrx,
    /// `wl_shm` ABGR8888 (bytes R, G, B, A).
    Rgba,
    /// `wl_shm` XBGR8888.
    Rgbx,
    /// `wl_shm` RGBA8888 (bytes A, B, G, R).
    Abgr,
    /// `wl_shm` RGBX8888.
    Xbgr,
    /// `wl_shm` BGRA8888 (bytes A, R, G, B).
    Argb,
    /// `wl_shm` BGRX8888.
    Xrgb,
    /// `wl_shm` [AX]RGB2101010 (little-endian word: B low, then G, then R).
    Xrgb2101010,
    /// `wl_shm` [AX]BGR2101010 (R low, then G, then B).
    Xbgr2101010,
}

impl PixelLayout {
    /// Maps a `wl_shm_format` value.
    pub fn from_wl_shm(format: u32) -> Option<PixelLayout> {
        use wl_shm::Format as F;
        Some(match F::try_from(format).ok()? {
            F::Argb8888 => PixelLayout::Bgra,
            F::Xrgb8888 => PixelLayout::Bgrx,
            F::Abgr8888 => PixelLayout::Rgba,
            F::Xbgr8888 => PixelLayout::Rgbx,
            F::Rgba8888 => PixelLayout::Abgr,
            F::Rgbx8888 => PixelLayout::Xbgr,
            F::Bgra8888 => PixelLayout::Argb,
            F::Bgrx8888 => PixelLayout::Xrgb,
            F::Argb2101010 | F::Xrgb2101010 => PixelLayout::Xrgb2101010,
            F::Abgr2101010 | F::Xbgr2101010 => PixelLayout::Xbgr2101010,
            _ => return None,
        })
    }

    /// Whether a frame in this layout is already BGRA bytes.
    pub fn is_bgra_bytes(self) -> bool {
        matches!(self, PixelLayout::Bgra | PixelLayout::Bgrx)
    }
}

/// Format preference, best first (`shm_frame.cpp:32-50`).
const PREFERENCE: [wl_shm::Format; 12] = [
    wl_shm::Format::Xrgb8888,
    wl_shm::Format::Argb8888,
    wl_shm::Format::Xbgr8888,
    wl_shm::Format::Abgr8888,
    wl_shm::Format::Rgbx8888,
    wl_shm::Format::Rgba8888,
    wl_shm::Format::Bgrx8888,
    wl_shm::Format::Bgra8888,
    wl_shm::Format::Xrgb2101010,
    wl_shm::Format::Argb2101010,
    wl_shm::Format::Xbgr2101010,
    wl_shm::Format::Abgr2101010,
];

/// The most preferred of the offered `wl_shm_format` values.
pub fn preferred_wl_shm(offered: &[u32]) -> Option<u32> {
    PREFERENCE
        .iter()
        .map(|f| *f as u32)
        .find(|code| offered.contains(code))
}

/// `wl_output.transform`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OutputTransform {
    /// No transform.
    #[default]
    Normal,
    /// 90° counter-clockwise.
    Rot90,
    /// 180°.
    Rot180,
    /// 270° counter-clockwise.
    Rot270,
    /// Horizontal flip.
    Flipped,
    /// Flip then 90°.
    Flipped90,
    /// Flip then 180°.
    Flipped180,
    /// Flip then 270°.
    Flipped270,
}

impl OutputTransform {
    /// Maps the protocol enum value.
    pub fn from_wl(value: u32) -> Option<OutputTransform> {
        Some(match value {
            0 => OutputTransform::Normal,
            1 => OutputTransform::Rot90,
            2 => OutputTransform::Rot180,
            3 => OutputTransform::Rot270,
            4 => OutputTransform::Flipped,
            5 => OutputTransform::Flipped90,
            6 => OutputTransform::Flipped180,
            7 => OutputTransform::Flipped270,
            _ => return None,
        })
    }

    fn swaps_axes(self) -> bool {
        matches!(
            self,
            OutputTransform::Rot90
                | OutputTransform::Rot270
                | OutputTransform::Flipped90
                | OutputTransform::Flipped270
        )
    }
}

/// A frame as the compositor delivered it, borrowed from the backend's
/// buffer until the next capture.
#[derive(Clone, Copy, Debug)]
pub struct FrameView<'a> {
    /// Pixel bytes, `height` rows of `stride` bytes.
    pub data: &'a [u8],
    /// Bytes per row.
    pub stride: usize,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel byte order.
    pub layout: PixelLayout,
    /// Rows are bottom-up.
    pub y_invert: bool,
    /// The output's transform (the buffer is in output orientation).
    pub transform: OutputTransform,
    /// Capture time, `CLOCK_MONOTONIC` nanoseconds.
    pub timestamp_ns: i64,
}

/// An owned, upright, top-down BGRA frame with a packed stride.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BgraFrame {
    /// `height * stride` bytes.
    pub data: Vec<u8>,
    /// Bytes per row (`width * 4`).
    pub stride: usize,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Capture time, `CLOCK_MONOTONIC` nanoseconds.
    pub timestamp_ns: i64,
}

/// Why a frame could not be normalised.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// Width or height is zero.
    #[error("frame has zero width or height")]
    ZeroSize,
    /// Stride is smaller than `width * 4`.
    #[error("frame stride is smaller than width * 4")]
    ShortStride,
    /// The buffer holds fewer than `stride * (height - 1) + width * 4` bytes.
    #[error("frame buffer is shorter than its geometry")]
    ShortBuffer,
}

fn convert_pixel(layout: PixelLayout, px: &[u8]) -> [u8; 4] {
    match layout {
        PixelLayout::Bgra => [px[0], px[1], px[2], px[3]],
        PixelLayout::Bgrx => [px[0], px[1], px[2], 255],
        PixelLayout::Rgba => [px[2], px[1], px[0], px[3]],
        PixelLayout::Rgbx => [px[2], px[1], px[0], 255],
        PixelLayout::Abgr => [px[1], px[2], px[3], px[0]],
        PixelLayout::Xbgr => [px[1], px[2], px[3], 255],
        PixelLayout::Argb => [px[3], px[2], px[1], px[0]],
        PixelLayout::Xrgb => [px[3], px[2], px[1], 255],
        PixelLayout::Xrgb2101010 => {
            let w = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
            [
                ((w & 0x3FF) >> 2) as u8,
                (((w >> 10) & 0x3FF) >> 2) as u8,
                (((w >> 20) & 0x3FF) >> 2) as u8,
                255,
            ]
        }
        PixelLayout::Xbgr2101010 => {
            let w = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
            [
                (((w >> 20) & 0x3FF) >> 2) as u8,
                (((w >> 10) & 0x3FF) >> 2) as u8,
                ((w & 0x3FF) >> 2) as u8,
                255,
            ]
        }
    }
}

/// Converts `view` into an upright, top-down, packed BGRA frame.
pub fn normalize(view: &FrameView<'_>, out: &mut BgraFrame) -> Result<(), FrameError> {
    let (w, h) = (view.width as usize, view.height as usize);
    if w == 0 || h == 0 {
        return Err(FrameError::ZeroSize);
    }
    let row = w * 4;
    if view.stride < row {
        return Err(FrameError::ShortStride);
    }
    if view.data.len() < view.stride * (h - 1) + row {
        return Err(FrameError::ShortBuffer);
    }
    out.timestamp_ns = view.timestamp_ns;

    // Stage 1: top-down BGRA at the source geometry.
    let needs_transform = view.transform != OutputTransform::Normal;
    let staged: &mut Vec<u8> = if needs_transform {
        &mut Vec::new()
    } else {
        &mut out.data
    };
    let mut stage = std::mem::take(staged);
    stage.clear();
    stage.reserve(row * h);
    let src_row = |y: usize| {
        let sy = if view.y_invert { h - 1 - y } else { y };
        &view.data[sy * view.stride..sy * view.stride + row]
    };
    if view.layout == PixelLayout::Bgra {
        for y in 0..h {
            stage.extend_from_slice(src_row(y));
        }
    } else {
        for y in 0..h {
            for px in src_row(y).as_chunks::<4>().0 {
                stage.extend_from_slice(&convert_pixel(view.layout, px));
            }
        }
    }

    if !needs_transform {
        out.data = stage;
        out.width = view.width;
        out.height = view.height;
        out.stride = row;
        return Ok(());
    }

    // Stage 2: undo the output transform so the frame is upright.
    let t = view.transform;
    let (ow, oh) = if t.swaps_axes() { (h, w) } else { (w, h) };
    let orow = ow * 4;
    out.data.clear();
    out.data.resize(orow * oh, 0);
    for y in 0..h {
        for x in 0..w {
            let (xd, yd) = match t {
                OutputTransform::Normal => (x, y),
                OutputTransform::Rot90 => (h - 1 - y, x),
                OutputTransform::Rot180 => (w - 1 - x, h - 1 - y),
                OutputTransform::Rot270 => (y, w - 1 - x),
                OutputTransform::Flipped => (w - 1 - x, y),
                OutputTransform::Flipped90 => (h - 1 - y, w - 1 - x),
                OutputTransform::Flipped180 => (x, h - 1 - y),
                OutputTransform::Flipped270 => (y, x),
            };
            let s = y * row + x * 4;
            let d = yd * orow + xd * 4;
            out.data[d..d + 4].copy_from_slice(&stage[s..s + 4]);
        }
    }
    out.width = ow as u32;
    out.height = oh as u32;
    out.stride = orow;
    Ok(())
}
