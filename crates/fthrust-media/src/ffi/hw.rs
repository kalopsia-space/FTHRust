//! VA-API device and hardware-frame contexts.

use std::ffi::CString;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    /// Whether uploads write into mapped surfaces; shared with every pool
    /// made from this device, cleared on the first mapping failure.
    mapped: Arc<AtomicBool>,
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
        match alloc_pool(device, width, height, sw_format) {
            Ok(frames) => Ok(Self {
                device,
                frames,
                device_path: path.to_string(),
                mapped: Arc::new(AtomicBool::new(true)),
            }),
            Err(e) => {
                // SAFETY: device is an owned ref.
                unsafe { ffi::av_buffer_unref(&mut device) };
                Err(e)
            }
        }
    }

    /// A pool of `sw_format` surfaces on this device context, so frames from
    /// both pools share one VADisplay.
    pub(crate) fn pool_on(
        &self,
        width: u32,
        height: u32,
        sw_format: ffi::AVPixelFormat,
    ) -> Result<Self> {
        let frames = alloc_pool(self.device, width, height, sw_format)?;
        // SAFETY: self.device is valid; av_buffer_ref returns an owned ref.
        let device = unsafe { ffi::av_buffer_ref(self.device) };
        if device.is_null() {
            let mut frames = frames;
            // SAFETY: frames is an owned ref.
            unsafe { ffi::av_buffer_unref(&mut frames) };
            return Err(MediaError::Invalid("av_buffer_ref failed".into()));
        }
        Ok(Self {
            device,
            frames,
            device_path: self.device_path.clone(),
            mapped: Arc::clone(&self.mapped),
        })
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

    /// Makes every upload go through `av_hwframe_transfer_data` (tests and
    /// the throughput comparison).
    pub fn force_transfer_upload(&self) {
        self.mapped.store(false, Ordering::Relaxed);
    }

    /// Whether uploads currently write into mapped surfaces.
    pub(crate) fn mapped_upload(&self) -> bool {
        self.mapped.load(Ordering::Relaxed)
    }

    /// Takes a fresh surface from the pool into `hw`.
    pub(crate) fn get_surface(&self, hw: &mut OwnedFrame) -> Result<()> {
        hw.unref();
        // SAFETY: frames ref and the frame pointer are valid.
        unsafe { super::check(ffi::av_hwframe_get_buffer(self.frames, hw.as_ptr(), 0)).map(|_| ()) }
    }

    /// Maps the surface in `hw` for overwriting. The returned frame's planes
    /// are writable; dropping it unmaps and flushes the writes.
    pub(crate) fn map_for_write(&self, hw: &OwnedFrame) -> Result<OwnedFrame> {
        let mapped = OwnedFrame::empty()?;
        // SAFETY: both frames are valid; the format is set so the mapping
        // exposes the pool's software layout.
        unsafe {
            let sw_format = (*((*self.frames).data as *mut ffi::AVHWFramesContext)).sw_format;
            (*mapped.as_ptr()).format = sw_format as i32;
            super::check(ffi::av_hwframe_map(
                mapped.as_ptr(),
                hw.as_ptr(),
                ffi::AV_HWFRAME_MAP_WRITE as i32 | ffi::AV_HWFRAME_MAP_OVERWRITE as i32,
            ))?;
        }
        Ok(mapped)
    }

    /// Records that mapping failed and transfers are used from now on.
    pub(crate) fn mapped_unavailable(&self, err: &MediaError) {
        if self.mapped.swap(false, Ordering::Relaxed) {
            tracing::info!(reason = %err, "mapped upload unavailable; using transfer");
        }
    }

    /// Uploads a software NV12 frame into a fresh hardware surface in `hw`.
    pub fn upload(&self, hw: &mut OwnedFrame, sw: &OwnedFrame) -> Result<()> {
        if self.mapped_upload() {
            self.get_surface(hw)?;
            match self.copy_into_mapped(hw, sw) {
                Ok(()) => return hw.copy_props_from(sw),
                Err(e) => self.mapped_unavailable(&e),
            }
        }
        self.upload_transfer(hw, sw)
    }

    /// [`Self::upload`] through `av_hwframe_transfer_data` only.
    pub(crate) fn upload_transfer(&self, hw: &mut OwnedFrame, sw: &OwnedFrame) -> Result<()> {
        self.get_surface(hw)?;
        // SAFETY: both frame pointers are valid.
        unsafe {
            super::check(ffi::av_hwframe_transfer_data(hw.as_ptr(), sw.as_ptr(), 0))?;
        }
        hw.copy_props_from(sw)
    }

    /// Copies the NV12 planes of `sw` into the mapped surface of `hw`.
    fn copy_into_mapped(&self, hw: &OwnedFrame, sw: &OwnedFrame) -> Result<()> {
        let mapped = self.map_for_write(hw)?;
        // SAFETY: both frames hold NV12 planes of the same size; the mapped
        // frame stays alive for the copy.
        unsafe {
            let (m, s) = (mapped.as_ptr(), sw.as_ptr());
            let w = (*s).width as usize;
            let h = (*s).height as usize;
            for (plane, rows, row) in [(0usize, h, w), (1, h.div_ceil(2), w.next_multiple_of(2))] {
                if (*m).data[plane].is_null()
                    || (*s).data[plane].is_null()
                    || ((*m).linesize[plane] as usize) < row
                    || ((*s).linesize[plane] as usize) < row
                {
                    return Err(MediaError::Invalid(
                        "mapped surface has no usable plane".into(),
                    ));
                }
                copy_rows(
                    (*s).data[plane],
                    (*s).linesize[plane] as usize,
                    (*m).data[plane],
                    (*m).linesize[plane] as usize,
                    row,
                    rows,
                );
            }
        }
        Ok(())
    }
}

/// Copies `rows` rows of `row_bytes` between planes with their own strides.
///
/// # Safety
/// Both pointers must cover `rows` rows at their stride, without overlap.
pub(crate) unsafe fn copy_rows(
    src: *const u8,
    src_stride: usize,
    dst: *mut u8,
    dst_stride: usize,
    row_bytes: usize,
    rows: usize,
) {
    for y in 0..rows {
        // SAFETY: the caller guarantees both planes cover these rows.
        unsafe {
            ptr::copy_nonoverlapping(src.add(y * src_stride), dst.add(y * dst_stride), row_bytes)
        };
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

/// Surfaces in every VA-API pool (fixed size: `av_hwframe_get_buffer`
/// fails once all are taken).
pub(crate) const POOL_SURFACES: usize = 8;

/// Allocates and initialises a [`POOL_SURFACES`]-surface pool on `device`.
fn alloc_pool(
    device: *mut ffi::AVBufferRef,
    width: u32,
    height: u32,
    sw_format: ffi::AVPixelFormat,
) -> Result<*mut ffi::AVBufferRef> {
    // SAFETY: device is a valid device context ref; the frames context
    // layout is FFmpeg's public AVHWFramesContext.
    unsafe {
        let frames = ffi::av_hwframe_ctx_alloc(device);
        if frames.is_null() {
            return Err(MediaError::Invalid("av_hwframe_ctx_alloc failed".into()));
        }
        let ctx = (*frames).data as *mut ffi::AVHWFramesContext;
        (*ctx).format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
        (*ctx).sw_format = sw_format;
        (*ctx).width = width as i32;
        (*ctx).height = height as i32;
        (*ctx).initial_pool_size = POOL_SURFACES as i32;
        let ret = ffi::av_hwframe_ctx_init(frames);
        if ret < 0 {
            let mut frames = frames;
            ffi::av_buffer_unref(&mut frames);
            return Err(MediaError::Ffmpeg(ffmpeg_next::Error::from(ret)));
        }
        Ok(frames)
    }
}

/// Whether a VA-API device can be created on any candidate render node.
pub fn vaapi_available() -> bool {
    crate::init().is_ok() && VaapiFrames::create(64, 64).is_ok()
}

/// The device number (`st_rdev`) of the character device at `path`.
pub fn render_node_dev(path: &str) -> Option<u64> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let meta = std::fs::metadata(path).ok()?;
    meta.file_type().is_char_device().then(|| meta.rdev())
}

/// Whether two DRM device numbers belong to one GPU: equal, or the same
/// `/sys/dev/char/<maj>:<min>/device`, so a primary node (`card1`) and its
/// render node (`renderD128`) match.
pub fn same_gpu(a: u64, b: u64) -> bool {
    if a == b {
        return true;
    }
    let device = |dev: u64| {
        let (major, minor) = split_dev(dev);
        std::fs::canonicalize(format!("/sys/dev/char/{major}:{minor}/device")).ok()
    };
    matches!((device(a), device(b)), (Some(x), Some(y)) if x == y)
}

/// glibc's `major`/`minor` split of a Linux `dev_t`.
fn split_dev(dev: u64) -> (u64, u64) {
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff);
    let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
    (major, minor)
}
