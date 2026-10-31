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

    /// Copies the `columns`×`rows` pixels the content watchdog samples out
    /// of surface `index` into `out`, packed BGRX, `columns` pixels a row
    /// (see [`gather_grid`]). The surface is mapped for reading and only
    /// those pixels are touched: no download of the whole surface.
    pub(crate) fn sample_grid(
        &self,
        index: usize,
        columns: usize,
        rows: usize,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let surface = self.checked(index)?;
        // A direct mapping (vaDeriveImage) costs no copy, and reading 144
        // pixels from it is cheap; FFmpeg's default read mapping copies the
        // whole surface first. Drivers that cannot derive get that one.
        let mapped = match map_for_read(surface, true) {
            Ok(mapped) => {
                log_sample_path_once(true);
                mapped
            }
            Err(_) => {
                let mapped = map_for_read(surface, false)?;
                log_sample_path_once(false);
                mapped
            }
        };
        let (w, h) = (self.width as usize, self.height as usize);
        // SAFETY: the mapping is alive until `mapped` drops after this block;
        // its plane is checked to hold h rows of linesize >= w * 4 bytes, and
        // gather_grid only reads pixels inside w x h.
        unsafe {
            let f = mapped.as_ptr();
            let (data, linesize) = ((*f).data[0], (*f).linesize[0]);
            if data.is_null()
                || linesize < 0
                || (linesize as usize) < w * 4
                || (*f).height < h as i32
            {
                return Err(MediaError::Invalid("mapped surface is too small".into()));
            }
            let linesize = linesize as usize;
            gather_grid(w, h, columns, rows, out, |x, y| {
                let p = data.add(y * linesize + x * 4);
                [*p, *p.add(1), *p.add(2), *p.add(3)]
            });
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

/// Says once per process, per path, how the content watchdog reads the
/// capture surfaces: the copying path reads back the whole surface.
fn log_sample_path_once(direct: bool) {
    static DIRECT: std::sync::Once = std::sync::Once::new();
    static COPY: std::sync::Once = std::sync::Once::new();
    if direct {
        DIRECT.call_once(|| {
            tracing::info!(
                "the content watchdog reads its 144 pixels from a direct surface mapping"
            );
        });
    } else {
        COPY.call_once(|| {
            tracing::info!(
                "the driver cannot map capture surfaces directly; the content watchdog copies the whole surface once a second"
            );
        });
    }
}

/// Maps a BGR0 surface for reading, `direct`ly (no copy) or through a copy
/// the VA-API context makes; the mapping lives as long as the frame.
fn map_for_read(surface: &OwnedFrame, direct: bool) -> Result<OwnedFrame> {
    let mapped = OwnedFrame::empty()?;
    let mut flags = ffi::AV_HWFRAME_MAP_READ as i32;
    if direct {
        flags |= ffi::AV_HWFRAME_MAP_DIRECT as i32;
    }
    // SAFETY: both frames are valid; a software destination format makes the
    // VA-API context map the surface into `mapped`.
    unsafe {
        (*mapped.as_ptr()).format = ffi::AVPixelFormat::AV_PIX_FMT_BGR0 as i32;
        super::check(ffi::av_hwframe_map(
            mapped.as_ptr(),
            surface.as_ptr(),
            flags,
        ))?;
    }
    Ok(mapped)
}

/// Gathers the `columns`×`rows` grid of a `width`×`height` image into `out`
/// as a packed `columns`-wide BGRX image: for each cell the pixel at its
/// centre, the one `fthrust_core::capture_health::sample_bgra_grid` reads.
/// Sampling the packed image with the same grid reads every gathered pixel
/// once, so it gives the same metrics as sampling the whole image.
fn gather_grid(
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
    out: &mut Vec<u8>,
    pixel: impl Fn(usize, usize) -> [u8; 4],
) {
    let (columns, rows) = (columns.max(1), rows.max(1));
    let centre =
        |i: usize, n: usize, extent: usize| (extent - 1).min((2 * i + 1) * extent / (2 * n));
    out.clear();
    out.reserve(columns * rows * 4);
    for row in 0..rows {
        let y = centre(row, rows, height.max(1));
        for column in 0..columns {
            out.extend_from_slice(&pixel(centre(column, columns, width.max(1)), y));
        }
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

#[cfg(test)]
mod tests {
    use fthrust_core::capture_health::sample_bgra_grid;

    use super::*;

    /// A frame whose every pixel differs, with padded rows.
    fn pattern(width: usize, height: usize, stride: usize) -> Vec<u8> {
        let mut data = vec![0u8; stride * height];
        for y in 0..height {
            for x in 0..width {
                let i = y * stride + x * 4;
                data[i..i + 4].copy_from_slice(&[
                    (x * 7 + y) as u8,
                    (x * 3 + y * 11) as u8,
                    (x ^ y) as u8,
                    0,
                ]);
            }
        }
        data
    }

    #[test]
    fn the_gathered_grid_samples_like_the_whole_frame() {
        let (columns, rows) = (16, 9);
        for (w, h) in [(1920, 1080), (2560, 1440), (101, 67), (16, 9), (7, 3)] {
            let stride = w * 4 + 64;
            let frame = pattern(w, h, stride);
            let mut grid = Vec::new();
            gather_grid(w, h, columns, rows, &mut grid, |x, y| {
                let i = y * stride + x * 4;
                [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
            });
            assert_eq!(grid.len(), columns * rows * 4);
            let whole = sample_bgra_grid(&frame, w, h, stride, columns, rows).unwrap();
            let sparse =
                sample_bgra_grid(&grid, columns, rows, columns * 4, columns, rows).unwrap();
            assert_eq!(whole, sparse, "{w}x{h}");
        }
    }

    #[test]
    fn a_mapped_sample_matches_the_downloaded_surface() {
        if !super::super::hw::vaapi_available() {
            return eprintln!("skipping: no VA-API device");
        }
        let (w, h) = (640u32, 360u32);
        let device = VaapiFrames::create(64, 64).unwrap();
        let surfaces = CaptureSurfaces::new(&device, w, h, 2).unwrap();
        let (wu, hu) = (w as usize, h as usize);
        surfaces.write(1, &pattern(wu, hu, wu * 4), wu * 4).unwrap();
        let mut grid = Vec::new();
        surfaces.sample_grid(1, 16, 9, &mut grid).unwrap();
        let mut whole = Vec::new();
        let stride = surfaces.download(1, &mut whole).unwrap();
        assert_eq!(
            sample_bgra_grid(&grid, 16, 9, 16 * 4, 16, 9).unwrap(),
            sample_bgra_grid(&whole, wu, hu, stride, 16, 9).unwrap()
        );
    }
}
