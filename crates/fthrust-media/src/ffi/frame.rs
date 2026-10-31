//! An owned `AVFrame` with the handful of operations the encoder needs.

use std::ptr;

use ffmpeg_next::ffi;

use crate::{MediaError, Result};

/// Bytes `rows` rows take when they start `stride` bytes apart and the
/// last holds only `row_bytes` (it need not be padded), or `None` when there
/// are no rows or the size does not fit a `usize`.
pub(crate) fn packed_len(stride: usize, row_bytes: usize, rows: usize) -> Option<usize> {
    stride
        .checked_mul(rows.checked_sub(1)?)?
        .checked_add(row_bytes)
}

/// Checks that `len` bytes hold `rows` BGRA rows of `width` pixels that
/// start `stride` bytes apart; returns the row length in bytes.
pub(crate) fn check_bgra_geometry(
    len: usize,
    stride: usize,
    width: u32,
    rows: usize,
) -> Result<usize> {
    let row = (width as usize)
        .checked_mul(4)
        .ok_or_else(|| MediaError::Invalid(format!("frame width {width} is too large")))?;
    if stride < row {
        return Err(MediaError::Invalid(format!(
            "stride {stride} is smaller than width*4 ({row})"
        )));
    }
    let needed = packed_len(stride, row, rows).ok_or_else(|| {
        MediaError::Invalid(format!(
            "{rows} rows {stride} bytes apart do not fit in memory"
        ))
    })?;
    if len < needed {
        return Err(MediaError::Invalid(format!(
            "frame buffer holds {len} bytes but its geometry needs {needed}"
        )));
    }
    Ok(row)
}

/// An owned `AVFrame`.
#[derive(Debug)]
pub struct OwnedFrame(*mut ffi::AVFrame);

// SAFETY: the frame is exclusively owned; FFmpeg frames may be moved between
// threads when not shared.
unsafe impl Send for OwnedFrame {}

impl OwnedFrame {
    /// Allocates an empty frame (no buffers).
    pub fn empty() -> Result<Self> {
        // SAFETY: av_frame_alloc has no preconditions; null is checked.
        let ptr = unsafe { ffi::av_frame_alloc() };
        if ptr.is_null() {
            return Err(MediaError::Invalid("av_frame_alloc failed".into()));
        }
        Ok(Self(ptr))
    }

    /// Allocates a video frame with buffers for `format` at `width`×`height`.
    pub fn video(format: ffi::AVPixelFormat, width: u32, height: u32) -> Result<Self> {
        let frame = Self::empty()?;
        // SAFETY: the frame was just allocated and is exclusively owned.
        unsafe {
            (*frame.0).format = format as i32;
            (*frame.0).width = width as i32;
            (*frame.0).height = height as i32;
            super::check(ffi::av_frame_get_buffer(frame.0, 32))?;
        }
        Ok(frame)
    }

    /// Raw pointer for FFmpeg calls.
    pub fn as_ptr(&self) -> *mut ffi::AVFrame {
        self.0
    }

    /// Makes the frame's buffers writable (copies if shared).
    pub fn make_writable(&mut self) -> Result<()> {
        // SAFETY: valid frame pointer.
        unsafe { super::check(ffi::av_frame_make_writable(self.0)).map(|_| ()) }
    }

    /// Releases buffers and resets the frame (keeps the allocation).
    pub fn unref(&mut self) {
        // SAFETY: valid frame pointer.
        unsafe { ffi::av_frame_unref(self.0) }
    }

    /// Sets the presentation timestamp.
    pub fn set_pts(&mut self, pts: i64) {
        // SAFETY: valid frame pointer.
        unsafe { (*self.0).pts = pts }
    }

    /// Marks the frame as a forced keyframe (or clears the mark).
    pub fn set_forced_keyframe(&mut self, key: bool) {
        // SAFETY: valid frame pointer.
        unsafe {
            if key {
                (*self.0).pict_type = ffi::AVPictureType::AV_PICTURE_TYPE_I;
                (*self.0).flags |= ffi::AV_FRAME_FLAG_KEY;
            } else {
                (*self.0).pict_type = ffi::AVPictureType::AV_PICTURE_TYPE_NONE;
                (*self.0).flags &= !ffi::AV_FRAME_FLAG_KEY;
            }
        }
    }

    /// Makes this frame a new reference to `src`'s buffers (`av_frame_ref`),
    /// dropping whatever it held. Pixel data is shared, not copied.
    pub fn reference(&mut self, src: &OwnedFrame) -> Result<()> {
        self.unref();
        // SAFETY: both pointers are valid frames; self was just unreferenced.
        unsafe { super::check(ffi::av_frame_ref(self.0, src.0)).map(|_| ()) }
    }

    /// Whether the frame holds buffers.
    pub fn is_referenced(&self) -> bool {
        // SAFETY: valid frame pointer.
        unsafe { !(*self.0).data[0].is_null() || !(*self.0).buf[0].is_null() }
    }

    /// Copies timestamps and metadata from `src`.
    pub fn copy_props_from(&mut self, src: &OwnedFrame) -> Result<()> {
        // SAFETY: both pointers are valid frames.
        unsafe { super::check(ffi::av_frame_copy_props(self.0, src.0)).map(|_| ()) }
    }

    /// Plane pointers and line sizes, for the scaler.
    pub(crate) fn planes(&mut self) -> (*const *mut u8, *const i32) {
        // SAFETY: valid frame pointer; the arrays live inside the frame.
        unsafe { ((*self.0).data.as_ptr(), (*self.0).linesize.as_ptr()) }
    }
}

impl Drop for OwnedFrame {
    fn drop(&mut self) {
        // SAFETY: the pointer came from av_frame_alloc and is dropped once.
        unsafe { ffi::av_frame_free(&mut self.0) }
        self.0 = ptr::null_mut();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_len_leaves_the_last_row_unpadded_and_never_overflows() {
        assert_eq!(packed_len(512, 256, 64), Some(512 * 63 + 256));
        assert_eq!(packed_len(512, 256, 1), Some(256));
        assert_eq!(packed_len(512, 256, 0), None);
        assert_eq!(packed_len(usize::MAX, 1, 2), None);
        assert_eq!(packed_len(usize::MAX / 2, 4, 3), None);
    }

    #[test]
    fn bgra_geometry_errors_instead_of_overflowing() {
        assert_eq!(check_bgra_geometry(64 * 4 * 8, 64 * 4, 64, 8).unwrap(), 256);
        assert!(check_bgra_geometry(64 * 4 * 8 - 1, 64 * 4, 64, 8).is_err());
        assert!(check_bgra_geometry(1 << 20, 64 * 4 - 1, 64, 8).is_err());
        assert!(check_bgra_geometry(1 << 20, usize::MAX, 64, 2).is_err());
        assert!(check_bgra_geometry(1 << 20, 0, u32::MAX, 1).is_err());
    }
}
