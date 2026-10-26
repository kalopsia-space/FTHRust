//! BGRA to NV12 conversion and scaling on the GPU (VA-API `scale_vaapi`).

use std::ptr;

use ff::ffi;
use ffmpeg_next as ff;

use super::frame::OwnedFrame;
use super::hw::VaapiFrames;
use crate::{MediaError, Result};

/// A filter graph `buffer -> scale_vaapi -> buffersink` fed from a BGRX
/// hardware surface pool.
pub(crate) struct VaapiConverter {
    // Declared before `pool` so the graph drops first.
    #[allow(dead_code)] // owns the filters the raw pointers below point into
    graph: ff::filter::Graph,
    src: *mut ffi::AVFilterContext,
    sink: *mut ffi::AVFilterContext,
    pool: VaapiFrames,
    staging: OwnedFrame,
    hw: OwnedFrame,
    src_w: u32,
    src_h: u32,
}

// SAFETY: the graph, its filter contexts and the pools are exclusively
// owned and only used through `&mut self`.
unsafe impl Send for VaapiConverter {}

impl VaapiConverter {
    /// Builds the graph on the render node `device` opened, converting
    /// `src_w`×`src_h` BGRA into `dst_w`×`dst_h` NV12 (BT.709, limited range).
    pub(crate) fn new(
        device: &VaapiFrames,
        src_w: u32,
        src_h: u32,
        dst_w: u32,
        dst_h: u32,
    ) -> Result<Self> {
        let bgr0 = ffi::AVPixelFormat::AV_PIX_FMT_BGR0;
        if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
            return Err(MediaError::Invalid(format!(
                "cannot convert {src_w}x{src_h} to {dst_w}x{dst_h}"
            )));
        }
        let pool = device.pool_on(src_w, src_h, bgr0)?;

        let mut graph = ff::filter::Graph::new();
        let buffer = ff::filter::find("buffer")
            .ok_or_else(|| MediaError::Invalid("buffer filter missing".into()))?;
        let sink_filter = ff::filter::find("buffersink")
            .ok_or_else(|| MediaError::Invalid("buffersink filter missing".into()))?;
        let args = format!(
            "video_size={src_w}x{src_h}:pix_fmt={}:time_base=1/1",
            ffi::AVPixelFormat::AV_PIX_FMT_VAAPI as i32
        );
        // The buffer source must receive hw_frames_ctx before it is
        // initialised, so allocate it, set the parameters, then init.
        // SAFETY: the graph and filter are live; the result is checked.
        let src = unsafe {
            ffi::avfilter_graph_alloc_filter(graph.as_mut_ptr(), buffer.as_ptr(), c"in".as_ptr())
        };
        if src.is_null() {
            return Err(MediaError::Invalid(
                "could not allocate buffer source".into(),
            ));
        }
        graph.add(&sink_filter, "out", "")?;
        let sink = filter_ptr(&mut graph, "out")?;
        let (mut dev, frames) = pool.new_refs();
        // SAFETY: new_refs returned an owned device ref the graph does not need.
        unsafe { ffi::av_buffer_unref(&mut dev) };
        // SAFETY: src is a live buffer filter context; the parameters struct
        // is allocated, filled and freed here, and the frames ref is released
        // after buffersrc took its own reference.
        unsafe {
            let par = ffi::av_buffersrc_parameters_alloc();
            if par.is_null() {
                let mut frames = frames;
                ffi::av_buffer_unref(&mut frames);
                return Err(MediaError::Invalid(
                    "buffersrc parameters alloc failed".into(),
                ));
            }
            (*par).format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI as i32;
            (*par).width = src_w as i32;
            (*par).height = src_h as i32;
            (*par).time_base = ffi::AVRational { num: 1, den: 1 };
            (*par).hw_frames_ctx = frames;
            let ret = ffi::av_buffersrc_parameters_set(src, par);
            let mut frames = frames;
            ffi::av_buffer_unref(&mut frames);
            ffi::av_free(par as *mut _);
            super::check(ret)?;
            let cargs = std::ffi::CString::new(args).expect("no NUL");
            super::check(ffi::avfilter_init_str(src, cargs.as_ptr()))?;
        }
        graph.output("in", 0)?.input("out", 0)?.parse(&format!(
            "scale_vaapi=w={dst_w}:h={dst_h}:format=nv12:out_color_matrix=bt709:out_range=tv:out_chroma_location=left"
        ))?;
        graph.validate()?;

        Ok(Self {
            graph,
            src,
            sink,
            staging: OwnedFrame::video(bgr0, src_w, src_h)?,
            hw: OwnedFrame::empty()?,
            pool,
            src_w,
            src_h,
        })
    }

    /// Source size this converter was built for.
    pub(crate) fn source_size(&self) -> (u32, u32) {
        (self.src_w, self.src_h)
    }

    /// A new reference to the output `hw_frames_ctx`, for an encoder.
    pub(crate) fn output_frames_ref(&self) -> *mut ffi::AVBufferRef {
        // SAFETY: the sink is configured; the returned ref is borrowed, so
        // av_buffer_ref makes the owned one the caller releases.
        unsafe {
            let r = ffi::av_buffersink_get_hw_frames_ctx(self.sink);
            if r.is_null() {
                ptr::null_mut()
            } else {
                ffi::av_buffer_ref(r)
            }
        }
    }

    /// Writes the rows straight into a mapped surface of the pool. False
    /// when mapping is unavailable, which also disables it for good.
    fn upload_mapped(&mut self, bgra: &[u8], stride: usize, row: usize, h: usize) -> bool {
        if !self.pool.mapped_upload() {
            return false;
        }
        let result = self.pool.get_surface(&mut self.hw).and_then(|()| {
            let mapped = self.pool.map_for_write(&self.hw)?;
            // SAFETY: the mapped frame owns h rows of linesize[0] >= row
            // bytes; the source length was checked by the caller.
            unsafe {
                let f = mapped.as_ptr();
                if (*f).data[0].is_null() || ((*f).linesize[0] as usize) < row {
                    return Err(MediaError::Invalid(
                        "mapped surface has no usable plane".into(),
                    ));
                }
                super::hw::copy_rows(
                    bgra.as_ptr(),
                    stride,
                    (*f).data[0],
                    (*f).linesize[0] as usize,
                    row,
                    h,
                );
            }
            Ok(())
        });
        match result {
            Ok(()) => {
                self.hw.set_pts(0);
                true
            }
            Err(e) => {
                self.pool.mapped_unavailable(&e);
                false
            }
        }
    }

    /// The staging-frame path: copy into system memory, then transfer.
    fn upload_transfer(&mut self, bgra: &[u8], stride: usize, row: usize, h: usize) -> Result<()> {
        self.staging.make_writable()?;
        // SAFETY: the staging frame owns h rows of linesize[0] >= row bytes;
        // the source length was checked by the caller.
        unsafe {
            let f = self.staging.as_ptr();
            super::hw::copy_rows(
                bgra.as_ptr(),
                stride,
                (*f).data[0],
                (*f).linesize[0] as usize,
                row,
                h,
            );
            (*f).pts = 0;
        }
        self.pool.upload_transfer(&mut self.hw, &self.staging)
    }

    /// Uploads `bgra` (`stride` bytes per row) and converts it into `out`,
    /// an NV12 VA-API frame of the destination size. pts is left to the caller.
    pub(crate) fn convert(
        &mut self,
        bgra: &[u8],
        stride: usize,
        out: &mut OwnedFrame,
    ) -> Result<()> {
        let row = self.src_w as usize * 4;
        let h = self.src_h as usize;
        if stride < row {
            return Err(MediaError::Invalid(format!(
                "stride {stride} is smaller than width*4 ({row})"
            )));
        }
        let needed = stride * (h - 1) + row;
        if bgra.len() < needed {
            return Err(MediaError::Invalid(format!(
                "frame buffer holds {} bytes but its geometry needs {needed}",
                bgra.len()
            )));
        }
        if !self.upload_mapped(bgra, stride, row, h) {
            self.upload_transfer(bgra, stride, row, h)?;
        }
        Self::run_graph(self.src, self.sink, &self.hw, out)
    }

    /// Converts `src`, a BGR0 VA-API surface of the source size on this
    /// device (a capture surface), into `out` without any upload. `src` is
    /// only referenced for the conversion; pts is left to the caller.
    pub(crate) fn convert_surface(&mut self, src: &OwnedFrame, out: &mut OwnedFrame) -> Result<()> {
        // SAFETY: src is a valid frame; only its header fields are read.
        let (format, w, h) = unsafe {
            let f = src.as_ptr();
            ((*f).format, (*f).width, (*f).height)
        };
        if format != ffi::AVPixelFormat::AV_PIX_FMT_VAAPI as i32
            || (w, h) != (self.src_w as i32, self.src_h as i32)
        {
            return Err(MediaError::Invalid(format!(
                "surface is {w}x{h} (format {format}), converter expects {}x{} VA-API",
                self.src_w, self.src_h
            )));
        }
        Self::run_graph(self.src, self.sink, src, out)
    }

    /// Pushes `input` through the graph into `out`.
    fn run_graph(
        src: *mut ffi::AVFilterContext,
        sink: *mut ffi::AVFilterContext,
        input: &OwnedFrame,
        out: &mut OwnedFrame,
    ) -> Result<()> {
        out.unref();
        // SAFETY: src/sink belong to the graph owned by the caller's self; the
        // frames are valid. KEEP_REF leaves the input frame intact for reuse.
        unsafe {
            super::check(ffi::av_buffersrc_add_frame_flags(
                src,
                input.as_ptr(),
                ffi::AV_BUFFERSRC_FLAG_KEEP_REF as i32,
            ))?;
            super::check(ffi::av_buffersink_get_frame(sink, out.as_ptr()))?;
        }
        Ok(())
    }
}

/// The raw context of the named filter; valid while `graph` lives.
fn filter_ptr(graph: &mut ff::filter::Graph, name: &str) -> Result<*mut ffi::AVFilterContext> {
    let ctx = graph
        .get(name)
        .ok_or_else(|| MediaError::Invalid(format!("filter {name} missing")))?;
    // SAFETY: the pointer is only stored next to the graph that owns it.
    Ok(unsafe { ctx.as_ptr() } as *mut ffi::AVFilterContext)
}

/// Converts one tightly packed BGRA frame on the GPU and downloads the result
/// as packed RGB24, for colour checks in tests.
#[doc(hidden)]
pub fn gpu_convert_rgb(
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    bgra: &[u8],
) -> Result<Vec<u8>> {
    gpu_convert_rgb_with_stride(src_w, src_h, dst_w, dst_h, src_w as usize * 4, bgra)
}

/// [`gpu_convert_rgb`] for a frame whose rows are `stride` bytes apart.
#[doc(hidden)]
pub fn gpu_convert_rgb_with_stride(
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    stride: usize,
    bgra: &[u8],
) -> Result<Vec<u8>> {
    gpu_convert_rgb_with_upload(src_w, src_h, dst_w, dst_h, stride, bgra, false)
}

/// [`gpu_convert_rgb_with_stride`] with the upload path chosen: mapped
/// surface writes, or (`force_transfer`) the staging frame and transfer.
#[doc(hidden)]
pub fn gpu_convert_rgb_with_upload(
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    stride: usize,
    bgra: &[u8],
    force_transfer: bool,
) -> Result<Vec<u8>> {
    crate::init()?;
    let device = VaapiFrames::create(64, 64)?;
    if force_transfer {
        device.force_transfer_upload();
    }
    let mut conv = VaapiConverter::new(&device, src_w, src_h, dst_w, dst_h)?;
    let mut hw = OwnedFrame::empty()?;
    conv.convert(bgra, stride, &mut hw)?;
    let nv12 = ffi::AVPixelFormat::AV_PIX_FMT_NV12;
    let mut sw = OwnedFrame::empty()?;
    // SAFETY: both frames are valid; the destination format is set so the
    // transfer allocates its buffers.
    unsafe {
        (*sw.as_ptr()).format = nv12 as i32;
        super::check(ffi::av_hwframe_transfer_data(sw.as_ptr(), hw.as_ptr(), 0))?;
    }
    let mut rgb = vec![0u8; dst_w as usize * dst_h as usize * 3];
    // SAFETY: the context is created, configured, used and freed here; the
    // output buffer holds dst_h rows of dst_w*3 bytes.
    unsafe {
        let ctx = ffi::sws_getContext(
            dst_w as i32,
            dst_h as i32,
            nv12,
            dst_w as i32,
            dst_h as i32,
            ffi::AVPixelFormat::AV_PIX_FMT_RGB24,
            2,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        );
        if ctx.is_null() {
            return Err(MediaError::Invalid("sws_getContext failed".into()));
        }
        let bt709 = ffi::sws_getCoefficients(ffi::SWS_CS_ITU709);
        let cfg = ffi::sws_setColorspaceDetails(ctx, bt709, 0, bt709, 1, 0, 1 << 16, 1 << 16);
        let (planes, strides) = sw.planes();
        let dst_planes: [*mut u8; 4] = [
            rgb.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        ];
        let dst_strides: [i32; 4] = [dst_w as i32 * 3, 0, 0, 0];
        let rows = if cfg < 0 {
            -1
        } else {
            ffi::sws_scale(
                ctx,
                planes as *const *const u8,
                strides,
                0,
                dst_h as i32,
                dst_planes.as_ptr(),
                dst_strides.as_ptr(),
            )
        };
        ffi::sws_freeContext(ctx);
        if rows <= 0 {
            return Err(MediaError::Invalid("NV12 to RGB conversion failed".into()));
        }
    }
    Ok(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_pool_survives_many_held_frames() {
        if !super::super::hw::vaapi_available() {
            return;
        }
        let device = VaapiFrames::create(64, 64).unwrap();
        let mut conv = VaapiConverter::new(&device, 64, 64, 64, 64).unwrap();
        let bgra = vec![0u8; 64 * 64 * 4];
        let mut held = Vec::new();
        for i in 0..24 {
            let mut out = OwnedFrame::empty().unwrap();
            conv.convert(&bgra, 64 * 4, &mut out)
                .unwrap_or_else(|e| panic!("frame {i} failed: {e:?}"));
            held.push(out);
        }
        assert!(!conv.output_frames_ref().is_null());
    }

    #[test]
    fn rejects_zero_sizes() {
        if !super::super::hw::vaapi_available() {
            return;
        }
        let device = VaapiFrames::create(64, 64).unwrap();
        assert!(VaapiConverter::new(&device, 0, 64, 64, 64).is_err());
    }
}
