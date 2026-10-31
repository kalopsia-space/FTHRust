//! BGRA to encoder-format conversion with explicit BT.709 studio-range
//! output, which `ffmpeg-next`'s scaler wrapper cannot express.

use std::ffi::{CStr, c_void};
use std::ptr;

use ffmpeg_next::ffi;

use super::frame::{OwnedFrame, check_bgra_geometry};
use crate::{MediaError, Result};

/// `SWS_BILINEAR` (the flag constant is not exported by the FFmpeg 9 bindings).
const SWS_BILINEAR: i64 = 2;

/// Most slice threads one conversion uses.
const MAX_THREADS: usize = 4;

/// Slice threads for a conversion: half the cores, at most four. One
/// thread takes about 2.8 ms for a 1080p frame and 11 ms at 4K; the other
/// half of the machine stays with the game being captured.
fn conversion_threads() -> i64 {
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    (cores / 2).clamp(1, MAX_THREADS) as i64
}

/// A `SwsContext` converting full-range BGRA into studio-range BT.709,
/// split into horizontal slices that run on their own threads.
#[derive(Debug)]
pub struct Scaler {
    ctx: *mut ffi::SwsContext,
    /// The source frame header; during one conversion its plane points
    /// into the caller's buffer.
    src: *mut ffi::AVFrame,
    src_width: u32,
    src_height: u32,
}

// SAFETY: the context and the frame header are exclusively owned.
unsafe impl Send for Scaler {}

/// The free callback of a buffer that wraps memory someone else owns.
unsafe extern "C" fn keep_borrowed(_opaque: *mut c_void, _data: *mut u8) {}

impl Scaler {
    /// Builds a bilinear scaler from `src` BGRA to `dst` at `dst_format`.
    pub fn new(
        src_width: u32,
        src_height: u32,
        dst_width: u32,
        dst_height: u32,
        dst_format: ffi::AVPixelFormat,
    ) -> Result<Self> {
        Self::with_threads(
            src_width,
            src_height,
            dst_width,
            dst_height,
            dst_format,
            conversion_threads(),
        )
    }

    /// [`Scaler::new`] with `threads` slice threads (`0`: FFmpeg's
    /// automatic count).
    pub(crate) fn with_threads(
        src_width: u32,
        src_height: u32,
        dst_width: u32,
        dst_height: u32,
        dst_format: ffi::AVPixelFormat,
        threads: i64,
    ) -> Result<Self> {
        let options: [(&CStr, i64); 8] = [
            (c"srcw", i64::from(src_width)),
            (c"srch", i64::from(src_height)),
            (c"src_format", ffi::AVPixelFormat::AV_PIX_FMT_BGRA as i64),
            (c"dstw", i64::from(dst_width)),
            (c"dsth", i64::from(dst_height)),
            (c"dst_format", dst_format as i64),
            (c"sws_flags", SWS_BILINEAR),
            (c"threads", threads),
        ];
        // SAFETY: the context is checked, configured through its AVOptions
        // (whose names every supported FFmpeg shares) and freed on every
        // failure; the coefficient table from sws_getCoefficients is static.
        // sws_init_context is the explicit initialisation that keeps
        // sws_scale_frame on its slice-threaded path in FFmpeg 8 and later.
        unsafe {
            let ctx = ffi::sws_alloc_context();
            if ctx.is_null() {
                return Err(MediaError::Invalid("sws_alloc_context failed".into()));
            }
            let fail = |ctx: *mut ffi::SwsContext, what: &str| {
                ffi::sws_freeContext(ctx);
                Err(MediaError::Invalid(what.to_owned()))
            };
            let configured = options.iter().all(|(name, value)| {
                ffi::av_opt_set_int(ctx.cast(), name.as_ptr(), *value, 0) >= 0
            });
            if !configured || ffi::sws_init_context(ctx, ptr::null_mut(), ptr::null_mut()) < 0 {
                return fail(
                    ctx,
                    &format!(
                        "could not set up a {src_width}x{src_height} BGRA to {dst_width}x{dst_height} {dst_format:?} scaler"
                    ),
                );
            }
            let bt709 = ffi::sws_getCoefficients(ffi::SWS_CS_ITU709);
            if bt709.is_null()
                || ffi::sws_setColorspaceDetails(ctx, bt709, 1, bt709, 0, 0, 1 << 16, 1 << 16) < 0
            {
                return fail(
                    ctx,
                    "could not configure full-range BGRA to studio-range BT.709 conversion",
                );
            }
            let src = ffi::av_frame_alloc();
            if src.is_null() {
                return fail(ctx, "av_frame_alloc failed");
            }
            Ok(Self {
                ctx,
                src,
                src_width,
                src_height,
            })
        }
    }

    /// Slice threads the conversion runs on.
    #[cfg(test)]
    pub(crate) fn threads(&self) -> i64 {
        let mut threads = 0;
        // SAFETY: valid context; "threads" is an int option.
        let ret =
            unsafe { ffi::av_opt_get_int(self.ctx.cast(), c"threads".as_ptr(), 0, &mut threads) };
        assert!(ret >= 0);
        threads
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
        check_bgra_geometry(bgra.len(), stride, self.src_width, height as usize)?;
        let linesize = i32::try_from(stride).map_err(|_| {
            MediaError::Invalid(format!("stride {stride} is larger than FFmpeg's line size"))
        })?;
        // SAFETY: the source length was checked above and the destination
        // frame owns buffers sized for the scaler's output. The source
        // frame wraps `bgra` in a buffer whose free callback does nothing,
        // so swscale references the borrowed rows instead of copying them;
        // it reads them only inside sws_scale_frame, whose slice threads
        // finish before it returns, and drops its reference there
        // (sws_frame_end). Ours goes with av_frame_unref below, so nothing
        // refers to `bgra` once this returns.
        unsafe {
            let buf = ffi::av_buffer_create(
                bgra.as_ptr().cast_mut(),
                bgra.len(),
                Some(keep_borrowed),
                ptr::null_mut(),
                ffi::AV_BUFFER_FLAG_READONLY,
            );
            if buf.is_null() {
                return Err(MediaError::Invalid("av_buffer_create failed".into()));
            }
            let src = self.src;
            (*src).buf[0] = buf;
            (*src).data[0] = bgra.as_ptr().cast_mut();
            (*src).linesize[0] = linesize;
            (*src).format = ffi::AVPixelFormat::AV_PIX_FMT_BGRA as i32;
            (*src).width = self.src_width as i32;
            (*src).height = height as i32;
            let ret = ffi::sws_scale_frame(self.ctx, dst.as_ptr(), src);
            ffi::av_frame_unref(src);
            super::check(ret).map(|_| ())
        }
    }
}

impl Drop for Scaler {
    fn drop(&mut self) {
        // SAFETY: the context came from sws_alloc_context and the frame from
        // av_frame_alloc; each is freed once.
        unsafe {
            ffi::sws_freeContext(self.ctx);
            ffi::av_frame_free(&mut self.src);
        }
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

    /// CPU time of the whole process (every thread), in milliseconds.
    fn process_cpu_ms() -> f64 {
        // SAFETY: getrusage fills the zeroed struct we pass.
        let usage = unsafe {
            let mut usage: libc::rusage = std::mem::zeroed();
            assert_eq!(libc::getrusage(libc::RUSAGE_SELF, &mut usage), 0);
            usage
        };
        let ms = |t: libc::timeval| t.tv_sec as f64 * 1e3 + t.tv_usec as f64 / 1e3;
        ms(usage.ru_utime) + ms(usage.ru_stime)
    }

    fn gradient(width: u32, height: u32) -> Vec<u8> {
        (0..width as usize * height as usize)
            .flat_map(|i| {
                let (x, y) = (i % width as usize, i / width as usize);
                [x as u8, y as u8, (x ^ y) as u8, 0xff]
            })
            .collect()
    }

    /// The single-threaded conversion the scaler used before slice threads
    /// (`sws_getContext` plus `sws_scale`), as the reference output.
    fn reference(
        bgra: &[u8],
        src: (u32, u32),
        dst: (u32, u32),
        fmt: ffi::AVPixelFormat,
    ) -> OwnedFrame {
        let mut out = OwnedFrame::video(fmt, dst.0, dst.1).unwrap();
        // SAFETY: the context is created, used on a buffer of src.0*4 by
        // src.1 bytes and freed here; `out` owns planes for the output size.
        unsafe {
            let ctx = ffi::sws_getContext(
                src.0 as i32,
                src.1 as i32,
                ffi::AVPixelFormat::AV_PIX_FMT_BGRA,
                dst.0 as i32,
                dst.1 as i32,
                fmt,
                SWS_BILINEAR as i32,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            );
            assert!(!ctx.is_null());
            let bt709 = ffi::sws_getCoefficients(ffi::SWS_CS_ITU709);
            assert!(
                ffi::sws_setColorspaceDetails(ctx, bt709, 1, bt709, 0, 0, 1 << 16, 1 << 16) >= 0
            );
            let planes = [bgra.as_ptr(), ptr::null(), ptr::null(), ptr::null()];
            let strides = [src.0 as i32 * 4, 0, 0, 0];
            let (dst_planes, dst_strides) = out.planes();
            let rows = ffi::sws_scale(
                ctx,
                planes.as_ptr(),
                strides.as_ptr(),
                0,
                src.1 as i32,
                dst_planes,
                dst_strides,
            );
            ffi::sws_freeContext(ctx);
            assert!(rows > 0);
        }
        out
    }

    /// The visible bytes of each plane of a 4:2:0 frame (NV12 or YUV420P).
    fn planes_of(frame: &OwnedFrame) -> Vec<Vec<u8>> {
        // SAFETY: the frame owns planes of the sizes its format implies.
        unsafe {
            let f = frame.as_ptr();
            let (w, h) = ((*f).width as usize, (*f).height as usize);
            let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
            let layout = if (*f).format == ffi::AVPixelFormat::AV_PIX_FMT_NV12 as i32 {
                vec![(0, w, h), (1, cw * 2, ch)]
            } else {
                vec![(0, w, h), (1, cw, ch), (2, cw, ch)]
            };
            layout
                .into_iter()
                .map(|(p, row, rows)| {
                    (0..rows)
                        .flat_map(|y| {
                            let start = (*f).data[p].add(y * (*f).linesize[p] as usize);
                            std::slice::from_raw_parts(start, row).to_vec()
                        })
                        .collect()
                })
                .collect()
        }
    }

    #[test]
    fn converts_on_several_threads() {
        crate::init().unwrap();
        let s = Scaler::new(1920, 1080, 1920, 1080, ffi::AVPixelFormat::AV_PIX_FMT_NV12).unwrap();
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        if cores < 4 {
            eprintln!("skipping: {cores} cores convert on one or two threads");
            return;
        }
        assert!(s.threads() > 1, "{} thread(s)", s.threads());
    }

    #[test]
    fn threaded_output_matches_the_single_threaded_conversion() {
        crate::init().unwrap();
        let cases = [
            ((1920, 1080), (1920, 1080)),
            ((2560, 1440), (1920, 1080)),
            ((1366, 768), (1280, 720)),
            ((642, 361), (642, 361)),
        ];
        for (src, dst) in cases {
            let bgra = gradient(src.0, src.1);
            for fmt in [
                ffi::AVPixelFormat::AV_PIX_FMT_NV12,
                ffi::AVPixelFormat::AV_PIX_FMT_YUV420P,
            ] {
                let want = planes_of(&reference(&bgra, src, dst, fmt));
                for threads in [4, 3] {
                    let mut s =
                        Scaler::with_threads(src.0, src.1, dst.0, dst.1, fmt, threads).unwrap();
                    let mut out = OwnedFrame::video(fmt, dst.0, dst.1).unwrap();
                    s.scale_bgra(&bgra, src.0 as usize * 4, src.1, &mut out)
                        .unwrap();
                    assert!(
                        planes_of(&out) == want,
                        "{src:?} -> {dst:?} {fmt:?} on {threads} threads differs"
                    );
                }
            }
        }
    }

    /// Run with `cargo test -p fthrust-media --lib conversion_time -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement, run by hand"]
    fn conversion_time() {
        crate::init().unwrap();
        let default = conversion_threads();
        for (w, h, frames) in [(1920u32, 1080u32, 300u32), (3840, 2160, 100)] {
            let bgra = gradient(w, h);
            for fmt in [
                ffi::AVPixelFormat::AV_PIX_FMT_NV12,
                ffi::AVPixelFormat::AV_PIX_FMT_YUV420P,
            ] {
                for threads in [1, 2, 3, 4, 6, 8, 0] {
                    let mut s = Scaler::with_threads(w, h, w, h, fmt, threads).unwrap();
                    let mut dst = OwnedFrame::video(fmt, w, h).unwrap();
                    for _ in 0..10 {
                        s.scale_bgra(&bgra, w as usize * 4, h, &mut dst).unwrap();
                    }
                    let (start, cpu) = (std::time::Instant::now(), process_cpu_ms());
                    for _ in 0..frames {
                        s.scale_bgra(&bgra, w as usize * 4, h, &mut dst).unwrap();
                    }
                    let wall = start.elapsed().as_secs_f64() * 1e3 / f64::from(frames);
                    let cpu = (process_cpu_ms() - cpu) / f64::from(frames);
                    let mark = if threads == default { " (default)" } else { "" };
                    println!(
                        "{w}x{h} {fmt:?}, threads {threads}{mark}: {wall:.2} ms/frame wall, {cpu:.2} ms/frame CPU"
                    );
                }
            }
        }
    }

    #[test]
    fn rejects_strides_whose_size_overflows_or_that_ffmpeg_cannot_take() {
        let mut s = scaler();
        let mut dst = OwnedFrame::video(ffi::AVPixelFormat::AV_PIX_FMT_YUV420P, 32, 32).unwrap();
        let buf = vec![0u8; 64 * 64 * 4];
        assert!(s.scale_bgra(&buf, usize::MAX / 2, 64, &mut dst).is_err());
        assert!(s.scale_bgra(&buf, usize::MAX, 64, &mut dst).is_err());
        // One row needs no stride, but FFmpeg's line size is an int.
        let mut one_row =
            Scaler::new(64, 1, 32, 32, ffi::AVPixelFormat::AV_PIX_FMT_YUV420P).unwrap();
        let row = vec![0u8; 64 * 4];
        assert!(
            one_row
                .scale_bgra(&row, i32::MAX as usize + 1, 1, &mut dst)
                .is_err()
        );
        assert!(one_row.scale_bgra(&row, 64 * 4, 1, &mut dst).is_ok());
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
