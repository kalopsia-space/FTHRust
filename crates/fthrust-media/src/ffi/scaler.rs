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
    /// `dst`. The caller guarantees `bgra` holds `stride * height` bytes.
    pub fn scale_bgra(
        &mut self,
        bgra: &[u8],
        stride: usize,
        height: u32,
        dst: &mut OwnedFrame,
    ) -> Result<()> {
        if bgra.len() < stride * height as usize {
            return Err(MediaError::Invalid(
                "frame buffer shorter than stride * height".into(),
            ));
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
