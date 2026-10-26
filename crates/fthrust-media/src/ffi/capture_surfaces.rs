//! VA-API BGRX surfaces exported as dmabufs, so the compositor can copy a
//! capture straight into memory the GPU converter reads.

use std::os::fd::{BorrowedFd, OwnedFd};

use ffmpeg_next::ffi;

use super::frame::OwnedFrame;
use super::hw::{POOL_SURFACES, VaapiFrames, copy_rows};
use crate::{MediaError, Result};

/// `DRM_FORMAT_XRGB8888` (`'XR24'`): B, G, R, X bytes in memory.
pub const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;

/// `DRM_FORMAT_ARGB8888` (`'AR24'`): B, G, R, A bytes in memory.
const DRM_FORMAT_ARGB8888: u32 = 0x3432_5241;

/// One exported surface as a dmabuf: the media-side mirror of the Wayland
/// crate's `DmabufImage`.
#[derive(Debug)]
pub struct ExportedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// DRM fourcc of the single layer.
    pub fourcc: u32,
    /// DRM format modifier of the memory object.
    pub modifier: u64,
    /// `(fd, offset, pitch)` per plane.
    pub planes: Vec<(OwnedFd, u32, u32)>,
}

impl ExportedImage {
    /// A copy with every plane fd duplicated.
    fn try_clone(&self) -> Result<Self> {
        let planes = self
            .planes
            .iter()
            .map(|(fd, offset, pitch)| Ok((fd.try_clone()?, *offset, *pitch)))
            .collect::<std::io::Result<Vec<_>>>()?;
        Ok(Self {
            width: self.width,
            height: self.height,
            fourcc: self.fourcc,
            modifier: self.modifier,
            planes,
        })
    }
}

/// A fixed set of BGR0 VA surfaces, allocated and exported for the life of
/// the value: none returns to the pool while its dmabuf may be in use.
pub(crate) struct CaptureSurfaces {
    // Field order is drop order: the fds close, then the surfaces return to
    // the pool, then the pool goes.
    images: Vec<ExportedImage>,
    surfaces: Vec<OwnedFrame>,
    pool: VaapiFrames,
    width: u32,
    height: u32,
}

// SAFETY: the frames and the pool are exclusively owned.
unsafe impl Send for CaptureSurfaces {}

impl std::fmt::Debug for CaptureSurfaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureSurfaces")
            .field("count", &self.surfaces.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl CaptureSurfaces {
    /// Allocates `count` `width`×`height` BGR0 surfaces on `device`'s
    /// VADisplay and exports each as a dmabuf.
    pub(crate) fn new(device: &VaapiFrames, width: u32, height: u32, count: usize) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(MediaError::Invalid(format!(
                "cannot export surfaces of {width}x{height}"
            )));
        }
        if count == 0 || count > POOL_SURFACES {
            return Err(MediaError::Invalid(format!(
                "dmabuf targets: count must be 1 to {POOL_SURFACES}, got {count}"
            )));
        }
        let pool = device.pool_on(width, height, ffi::AVPixelFormat::AV_PIX_FMT_BGR0)?;
        let mut surfaces = Vec::with_capacity(count);
        let mut images = Vec::with_capacity(count);
        for _ in 0..count {
            let mut surface = OwnedFrame::empty()?;
            pool.get_surface(&mut surface)?;
            images.push(export(&surface, width, height)?);
            surfaces.push(surface);
        }
        Ok(Self {
            images,
            surfaces,
            pool,
            width,
            height,
        })
    }

    /// Size of every surface.
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// How many surfaces there are.
    pub(crate) fn len(&self) -> usize {
        self.surfaces.len()
    }

    /// The exported images, with fresh fds, in surface order.
    pub(crate) fn images(&self) -> Result<Vec<ExportedImage>> {
        self.images.iter().map(ExportedImage::try_clone).collect()
    }

    /// The VA-API frame of surface `index`. Panics when out of range.
    pub(crate) fn surface(&self, index: usize) -> &OwnedFrame {
        &self.surfaces[index]
    }

    fn checked(&self, index: usize) -> Result<&OwnedFrame> {
        self.surfaces.get(index).ok_or_else(|| {
            MediaError::Invalid(format!(
                "surface {index} out of range ({} surfaces)",
                self.surfaces.len()
            ))
        })
    }

    /// Writes BGRA rows (`stride` bytes apart) into surface `index` through
    /// a mapping, as the shm path's upload does (tests).
    pub(crate) fn write(&self, index: usize, bgra: &[u8], stride: usize) -> Result<()> {
        let surface = self.checked(index)?;
        let row = self.width as usize * 4;
        let h = self.height as usize;
        if stride < row || bgra.len() < stride * (h - 1) + row {
            return Err(MediaError::Invalid(
                "frame buffer is shorter than the surface".into(),
            ));
        }
        let mapped = self.pool.map_for_write(surface)?;
        // SAFETY: the mapped frame stays alive for the copy and its plane is
        // checked to hold h rows of at least `row` bytes; the source length
        // was checked above.
        unsafe {
            let f = mapped.as_ptr();
            if (*f).data[0].is_null() || ((*f).linesize[0] as usize) < row {
                return Err(MediaError::Invalid(
                    "mapped surface has no usable plane".into(),
                ));
            }
            copy_rows(
                bgra.as_ptr(),
                stride,
                (*f).data[0],
                (*f).linesize[0] as usize,
                row,
                h,
            );
        }
        Ok(())
    }

    /// Downloads surface `index` into `out` as BGRX rows and returns their
    /// stride (the watchdog's fallback frame).
    pub(crate) fn download(&self, index: usize, out: &mut Vec<u8>) -> Result<usize> {
        let surface = self.checked(index)?;
        let sw = OwnedFrame::empty()?;
        // SAFETY: both frames are valid; the destination format is set so the
        // transfer allocates its buffers.
        unsafe {
            (*sw.as_ptr()).format = ffi::AVPixelFormat::AV_PIX_FMT_BGR0 as i32;
            super::check(ffi::av_hwframe_transfer_data(
                sw.as_ptr(),
                surface.as_ptr(),
                0,
            ))?;
        }
        let row = self.width as usize * 4;
        let h = self.height as usize;
        out.clear();
        out.resize(row * h, 0);
        // SAFETY: the transfer filled a BGR0 frame of the surface size whose
        // plane is checked to hold rows of at least `row` bytes; `out` holds
        // h rows of `row`.
        unsafe {
            let f = sw.as_ptr();
            if (*f).data[0].is_null() || ((*f).linesize[0] as usize) < row || (*f).height < h as i32
            {
                return Err(MediaError::Invalid(
                    "downloaded surface is too small".into(),
                ));
            }
            copy_rows(
                (*f).data[0],
                (*f).linesize[0] as usize,
                out.as_mut_ptr(),
                row,
                row,
                h,
            );
        }
        Ok(row)
    }
}

/// Maps `surface` to DRM PRIME and describes it with duplicated fds; the
/// mapping (and its own fds) is released before returning.
fn export(surface: &OwnedFrame, width: u32, height: u32) -> Result<ExportedImage> {
    let mapped = OwnedFrame::empty()?;
    // SAFETY: both frames are valid; a DRM_PRIME destination makes the VA-API
    // context export the surface. data[0] of the result is an
    // AVDRMFrameDescriptor that lives as long as `mapped`, and every index
    // read from it is bounds-checked against its counts.
    unsafe {
        (*mapped.as_ptr()).format = ffi::AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32;
        super::check(ffi::av_hwframe_map(
            mapped.as_ptr(),
            surface.as_ptr(),
            ffi::AV_HWFRAME_MAP_READ as i32 | ffi::AV_HWFRAME_MAP_WRITE as i32,
        ))?;
        let desc = (*mapped.as_ptr()).data[0] as *const ffi::AVDRMFrameDescriptor;
        if desc.is_null() {
            return Err(MediaError::Invalid("DRM mapping has no descriptor".into()));
        }
        let desc = &*desc;
        if desc.nb_layers != 1 || desc.layers[0].nb_planes < 1 {
            return Err(MediaError::Invalid(format!(
                "exported surface has {} layers",
                desc.nb_layers
            )));
        }
        let layer = &desc.layers[0];
        if layer.format != DRM_FORMAT_XRGB8888 && layer.format != DRM_FORMAT_ARGB8888 {
            return Err(MediaError::Invalid(format!(
                "exported surface has DRM format {:#010x}, not XRGB8888/ARGB8888",
                layer.format
            )));
        }
        let nb_objects = desc.nb_objects.clamp(0, desc.objects.len() as i32) as usize;
        let nb_planes = layer.nb_planes.clamp(0, layer.planes.len() as i32) as usize;
        let mut planes = Vec::with_capacity(nb_planes);
        let mut modifier = None;
        for plane in &layer.planes[..nb_planes] {
            let obj = usize::try_from(plane.object_index)
                .ok()
                .filter(|&i| i < nb_objects)
                .ok_or_else(|| MediaError::Invalid("plane names no object".into()))?;
            let object = &desc.objects[obj];
            if object.fd < 0 {
                return Err(MediaError::Invalid("exported object has no fd".into()));
            }
            modifier.get_or_insert(object.format_modifier);
            let fd = BorrowedFd::borrow_raw(object.fd).try_clone_to_owned()?;
            let offset = u32::try_from(plane.offset)
                .map_err(|_| MediaError::Invalid("plane offset out of range".into()))?;
            let pitch = u32::try_from(plane.pitch)
                .map_err(|_| MediaError::Invalid("plane pitch out of range".into()))?;
            planes.push((fd, offset, pitch));
        }
        Ok(ExportedImage {
            width,
            height,
            fourcc: layer.format,
            modifier: modifier.unwrap_or(u64::MAX),
            planes,
        })
    }
}
