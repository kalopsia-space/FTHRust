//! VA-API device and hardware-frame contexts.

use std::ffi::CString;
use std::ptr;

use ffmpeg_next::ffi;

use super::frame::OwnedFrame;
use crate::{MediaError, Result};

/// The render node the reference uses unless `FTHR_VAAPI_DEVICE` is set.
pub const DEFAULT_DEVICE: &str = "/dev/dri/renderD128";

/// Render nodes to try, in order: the configured one, then every
/// `/dev/dri/renderD*` so a machine whose first node is a GPU without
/// VA-API still finds one (the reference tried only `renderD128`).
pub fn candidate_devices() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(dev) = std::env::var("FTHR_VAAPI_DEVICE")
        && !dev.is_empty()
    {
        out.push(dev);
        return out;
    }
    out.push(DEFAULT_DEVICE.to_string());
    if let Ok(entries) = std::fs::read_dir("/dev/dri") {
        let mut nodes: Vec<String> = entries
            .flatten()
            .map(|e| e.path().to_string_lossy().into_owned())
            .filter(|p| p.contains("/renderD"))
            .collect();
        nodes.sort();
        for node in nodes {
            if !out.contains(&node) {
                out.push(node);
            }
        }
    }
    out
}

/// A VA-API device context plus an NV12 hardware frame pool.
#[derive(Debug)]
pub struct VaapiFrames {
    device: *mut ffi::AVBufferRef,
    frames: *mut ffi::AVBufferRef,
    /// The device path that opened.
    pub device_path: String,
}

// SAFETY: the buffer refs are exclusively owned here.
unsafe impl Send for VaapiFrames {}

impl VaapiFrames {
    /// Opens the first usable render node and allocates a frame pool of
    /// `width`×`height` NV12 surfaces.
    pub fn create(width: u32, height: u32) -> Result<Self> {
        let mut last = None;
        for path in candidate_devices() {
            match Self::create_on(&path, width, height) {
                Ok(v) => return Ok(v),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| MediaError::Invalid("no render node found".into())))
    }

    /// Opens one render node and allocates the pool there.
    pub(crate) fn create_on(path: &str, width: u32, height: u32) -> Result<Self> {
        Self::create_on_with(path, width, height, ffi::AVPixelFormat::AV_PIX_FMT_NV12)
    }

    /// Opens one render node and allocates a pool of `sw_format` surfaces.
    pub(crate) fn create_on_with(
        path: &str,
        width: u32,
        height: u32,
        sw_format: ffi::AVPixelFormat,
    ) -> Result<Self> {
        let cpath =
            CString::new(path).map_err(|_| MediaError::Invalid("device path has NUL".into()))?;
        let mut device: *mut ffi::AVBufferRef = ptr::null_mut();
        // SAFETY: out-pointer and C string are valid for the call; on failure
        // nothing is leaked because device stays null.
        unsafe {
            super::check(ffi::av_hwdevice_ctx_create(
                &mut device,
                ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                cpath.as_ptr(),
                ptr::null_mut(),
                0,
            ))?;
        }
        // SAFETY: device is a valid device context ref; the frames context
        // layout is FFmpeg's public AVHWFramesContext.
        let frames = unsafe {
            let frames = ffi::av_hwframe_ctx_alloc(device);
            if frames.is_null() {
                ffi::av_buffer_unref(&mut device);
                return Err(MediaError::Invalid("av_hwframe_ctx_alloc failed".into()));
            }
            let ctx = (*frames).data as *mut ffi::AVHWFramesContext;
            (*ctx).format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
            (*ctx).sw_format = sw_format;
            (*ctx).width = width as i32;
            (*ctx).height = height as i32;
            (*ctx).initial_pool_size = 8;
            let ret = ffi::av_hwframe_ctx_init(frames);
            if ret < 0 {
                let mut frames = frames;
                ffi::av_buffer_unref(&mut frames);
                ffi::av_buffer_unref(&mut device);
                return Err(MediaError::Ffmpeg(ffmpeg_next::Error::from(ret)));
            }
            frames
        };
        Ok(Self {
            device,
            frames,
            device_path: path.to_string(),
        })
    }

    /// The device context (borrowed, not a new reference).
    #[allow(dead_code)] // used by the encoder wiring in the next task
    pub(crate) fn device_ref(&self) -> *mut ffi::AVBufferRef {
        self.device
    }

    /// New references for a codec context (`hw_device_ctx`, `hw_frames_ctx`).
    pub(crate) fn new_refs(&self) -> (*mut ffi::AVBufferRef, *mut ffi::AVBufferRef) {
        // SAFETY: both refs are valid; av_buffer_ref returns owned refs.
        unsafe {
            (
                ffi::av_buffer_ref(self.device),
                ffi::av_buffer_ref(self.frames),
            )
        }
    }

    /// Uploads a software NV12 frame into a fresh hardware surface in `hw`.
    pub fn upload(&self, hw: &mut OwnedFrame, sw: &OwnedFrame) -> Result<()> {
        hw.unref();
        // SAFETY: frames ref and both frame pointers are valid.
        unsafe {
            super::check(ffi::av_hwframe_get_buffer(self.frames, hw.as_ptr(), 0))?;
            super::check(ffi::av_hwframe_transfer_data(hw.as_ptr(), sw.as_ptr(), 0))?;
        }
        hw.copy_props_from(sw)
    }
}

impl Drop for VaapiFrames {
    fn drop(&mut self) {
        // SAFETY: refs are owned and dropped once.
        unsafe {
            ffi::av_buffer_unref(&mut self.frames);
            ffi::av_buffer_unref(&mut self.device);
        }
    }
}

/// Whether a VA-API device can be created on any candidate render node.
pub fn vaapi_available() -> bool {
    crate::init().is_ok() && VaapiFrames::create(64, 64).is_ok()
}
