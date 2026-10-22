//! The only module allowed to use `unsafe`: thin wrappers over FFmpeg APIs
//! that `ffmpeg-next` does not expose safely (codec contexts with hardware
//! frames, the scaler's colourspace details, VA-API device contexts). Every
//! `unsafe` block carries a `SAFETY` comment.
#![allow(unsafe_code)]

pub mod codec;
pub mod frame;
pub mod hw;
pub mod scaler;

use ffmpeg_next::ffi;

/// Converts an FFmpeg return code into a `MediaError` when negative.
pub(crate) fn check(ret: CInt) -> crate::Result<CInt> {
    if ret < 0 {
        Err(crate::MediaError::Ffmpeg(ffmpeg_next::Error::from(ret)))
    } else {
        Ok(ret)
    }
}

pub(crate) type CInt = std::os::raw::c_int;

/// `AVERROR(EAGAIN)` on Linux.
pub(crate) const AVERROR_EAGAIN: CInt = -11;

/// Whether `ret` is `AVERROR(EAGAIN)` or `AVERROR_EOF`.
pub(crate) fn is_drained(ret: CInt) -> bool {
    ret == AVERROR_EAGAIN || ret == ffi::AVERROR_EOF
}
