//! BGRA to encoder-format conversion with explicit BT.709 studio-range
//! output, which `ffmpeg-next`'s scaler wrapper cannot express.

use std::ptr;

use ffmpeg_next::ffi;

use super::frame::OwnedFrame;
use crate::{MediaError, Result};

/// `SWS_BILINEAR` (the flag constant is not exported by the FFmpeg 9 bindings).
const SWS_BILINEAR: i32 = 2;

/// A `SwsContext` converting full-range BGRA into studio-range BT.709.
#[derive(Debug)]
pub struct Scaler {
    ctx: *mut ffi::SwsContext,
    src_width: u32,
    src_height: u32,
}

// SAFETY: the context is exclusively owned.
unsafe impl Send for Scaler {}

impl Scaler {
    /// Builds a bilinear scaler from `src` BGRA to `dst` at `dst_format`.
    pub fn new(
        src_width: u32,
        src_height: u32,
        dst_width: u32,
        dst_height: u32,
        dst_format: ffi::AVPixelFormat,
    ) -> Result<Self> {
        // SAFETY: plain integer arguments; null results are checked; the
        // coefficient table returned by sws_getCoefficients is static.
        let ctx = unsafe {
            let ctx = ffi::sws_getContext(
                src_width as i32,
                src_height as i32,
                ffi::AVPixelFormat::AV_PIX_FMT_BGRA,
                dst_width as i32,
                dst_height as i32,
                dst_format,
                SWS_BILINEAR,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            );
            if ctx.is_null() {
                return Err(MediaError::Invalid("sws_getContext failed".into()));
            }
            let bt709 = ffi::sws_getCoefficients(ffi::SWS_CS_ITU709);
            if bt709.is_null()
                || ffi::sws_setColorspaceDetails(ctx, bt709, 1, bt709, 0, 0, 1 << 16, 1 << 16) < 0
            {
                ffi::sws_freeContext(ctx);
                return Err(MediaError::Invalid(
                    "could not configure full-range BGRA to studio-range BT.709 conversion".into(),
                ));
            }
            ctx
        };
        Ok(Self {
            ctx,
            src_width,
            src_height,
        })
    }

    /// Source size this scaler was built for.
    pub fn source_size(&self) -> (u32, u32) {
        (self.src_width, self.src_height)
    }

    /// Converts one BGRA image (`stride` bytes per row, `height` rows) into
    /// `dst`. `height` must match the scaler's source height, `stride` must
    /// cover a source row, and `bgra` must hold `stride * (height - 1) +
    /// width * 4` bytes (the last row need not be padded).
    pub fn scale_bgra(
        &mut self,
        bgra: &[u8],
        stride: usize,
        height: u32,
        dst: &mut OwnedFrame,
    ) -> Result<()> {
        if height != self.src_height || height == 0 {
            return Err(MediaError::Invalid(format!(
                "frame height {height} does not match the scaler's source height {}",
                self.src_height
            )));
        }
        let row = self.src_width as usize * 4;
        if stride < row {
            return Err(MediaError::Invalid(format!(
                "stride {stride} is smaller than width*4 ({row})"
            )));
        }
        let needed = stride * (height as usize - 1) + row;
        if bgra.len() < needed {
            return Err(MediaError::Invalid(format!(
                "frame buffer holds {} bytes but its geometry needs {needed}",
                bgra.len()
            )));
        }
        let src_planes: [*const u8; 4] = [bgra.as_ptr(), ptr::null(), ptr::null(), ptr::null()];
        let src_strides: [i32; 4] = [stride as i32, 0, 0, 0];
        let (dst_planes, dst_strides) = dst.planes();
        // SAFETY: the source buffer length was checked above; the destination
        // frame owns buffers sized for the scaler's output.
        let rows = unsafe {
            ffi::sws_scale(
                self.ctx,
                src_planes.as_ptr(),
                src_strides.as_ptr(),
                0,
                height as i32,
                dst_planes,
                dst_strides,
            )
        };
        if rows <= 0 {
            return Err(MediaError::Invalid("sws_scale produced no rows".into()));
        }
        Ok(())
    }
}

impl Drop for Scaler {
    fn drop(&mut self) {
        // SAFETY: the context came from sws_getContext and is freed once.
        unsafe { ffi::sws_freeContext(self.ctx) }
        self.ctx = ptr::null_mut();
    }
}

/// Points a YUV-to-RGB `ctx` at `frame`'s own matrix and range, producing
/// full-range RGB. Untagged frames use BT.709 from 720 lines up and BT.601
/// below, FFmpeg's convention. Without this swscale assumes BT.601, which
/// shifts the colours of BT.709 video.
pub fn match_source_colorspace(
    ctx: &mut ffmpeg_next::software::scaling::Context,
    frame: &ffmpeg_next::frame::Video,
) {
    use ffmpeg_next::util::color::{Range, Space};
    let matrix = match frame.color_space() {
        Space::BT709 => ffi::SWS_CS_ITU709,
        Space::BT470BG | Space::SMPTE170M => ffi::SWS_CS_ITU601,
        Space::BT2020NCL | Space::BT2020CL => ffi::SWS_CS_BT2020,
        _ if frame.height() >= 720 => ffi::SWS_CS_ITU709,
        _ => ffi::SWS_CS_ITU601,
    };
    let full_range = i32::from(frame.color_range() == Range::JPEG);
    // SAFETY: the context is valid for the borrow; the coefficient table is
    // static. A failure leaves the context's previous (default) details.
    unsafe {
        let coefficients = ffi::sws_getCoefficients(matrix);
        if !coefficients.is_null() {
            ffi::sws_setColorspaceDetails(
                ctx.as_mut_ptr(),
                coefficients,
                full_range,
                coefficients,
                1,
                0,
                1 << 16,
                1 << 16,
            );
        }
    }
}

/// An `ffmpeg_next` software scaler that may move to a worker thread. The
/// wrapped `SwsContext` is only used through `&mut self` by one owner.
pub struct SendScaler(pub ffmpeg_next::software::scaling::Context);

// SAFETY: `sws_scale` has no thread affinity; the context is never shared,
// only moved with its owner.
unsafe impl Send for SendScaler {}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaler() -> Scaler {
        crate::init().unwrap();
        Scaler::new(64, 64, 32, 32, ffi::AVPixelFormat::AV_PIX_FMT_YUV420P).unwrap()
    }

    #[test]
    fn rejects_stride_smaller_than_source_width() {
        let mut s = scaler();
        let mut dst = OwnedFrame::video(ffi::AVPixelFormat::AV_PIX_FMT_YUV420P, 32, 32).unwrap();
        let buf = vec![0u8; 64 * 64 * 4];
        assert!(s.scale_bgra(&buf, 16, 64, &mut dst).is_err());
    }

    #[test]
    fn rejects_short_buffer_and_wrong_height() {
        let mut s = scaler();
        let mut dst = OwnedFrame::video(ffi::AVPixelFormat::AV_PIX_FMT_YUV420P, 32, 32).unwrap();
        let short = vec![0u8; 64 * 64 * 4 - 1];
        assert!(s.scale_bgra(&short, 64 * 4, 64, &mut dst).is_err());
        let full = vec![0u8; 64 * 64 * 4];
        assert!(
            s.scale_bgra(&full, 64 * 4, 65, &mut dst).is_err(),
            "height must match the scaler"
        );
        assert!(s.scale_bgra(&full, 64 * 4, 64, &mut dst).is_ok());
    }

    #[test]
    fn accepts_unpadded_last_row() {
        // The encoder's rule: stride*(h-1) + w*4 bytes is enough.
        let mut s = scaler();
        let mut dst = OwnedFrame::video(ffi::AVPixelFormat::AV_PIX_FMT_YUV420P, 32, 32).unwrap();
        let stride = 512;
        let buf = vec![0u8; stride * 63 + 64 * 4];
        assert!(s.scale_bgra(&buf, stride, 64, &mut dst).is_ok());
    }
}
