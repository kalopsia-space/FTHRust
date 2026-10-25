//! An owned encoder `AVCodecContext` opened the way the reference opens it.

use std::ffi::CString;
use std::ptr;

use ffmpeg_next::ffi;

use super::frame::OwnedFrame;
use super::hw::VaapiFrames;
use super::vpp::VaapiConverter;
use crate::{MediaError, Result};

/// What an encoder is opened with.
#[derive(Clone, Debug)]
pub struct OpenParams {
    /// Output width.
    pub width: u32,
    /// Output height.
    pub height: u32,
    /// Frame rate (time base `1/fps`).
    pub fps: u32,
    /// Target bitrate in bits per second.
    pub bitrate_bps: i64,
    /// Private options applied before `avcodec_open2`.
    pub options: Vec<(&'static str, String)>,
    /// Keyframe interval in frames (`None` = two seconds).
    pub gop_frames: Option<u32>,
    /// Pixel format of the frames the caller will send (NV12 for hardware,
    /// YUV420P for software).
    pub pix_fmt: ffi::AVPixelFormat,
}

/// One packet received from the encoder.
#[derive(Debug)]
pub struct RawPacket {
    /// Compressed bytes (copied out of the packet).
    pub data: std::sync::Arc<[u8]>,
    /// Presentation timestamp.
    pub pts: i64,
    /// Decode timestamp.
    pub dts: i64,
    /// Whether the packet is a keyframe.
    pub keyframe: bool,
}

/// Copies a packet's payload; a packet without data (zero-size output)
/// yields an empty vector instead of a null-pointer slice.
///
/// # Safety
/// `p` must be a packet whose `data` is either null or valid for `size`
/// bytes.
pub(crate) unsafe fn packet_bytes(p: &ffi::AVPacket) -> std::sync::Arc<[u8]> {
    if p.data.is_null() || p.size <= 0 {
        return std::sync::Arc::from([]);
    }
    // SAFETY: guaranteed by the caller contract above.
    unsafe { std::slice::from_raw_parts(p.data, p.size as usize) }.into()
}

/// What a VA-API encoder takes its frames from.
pub(crate) enum HwInput {
    /// The caller uploads NV12 frames into the pool.
    Pool(VaapiFrames),
    /// The converter produces NV12 frames on the same device; the device
    /// owner `VaapiFrames` is kept for uploads after a CPU fallback.
    Converter(VaapiFrames, VaapiConverter),
}

/// An open encoder context.
pub struct RawEncoder {
    ctx: *mut ffi::AVCodecContext,
    pkt: *mut ffi::AVPacket,
    hw: Option<VaapiFrames>,
    // The converter holds its own device reference, so drop order against
    // `hw` does not matter.
    converter: Option<VaapiConverter>,
    name: &'static str,
}

impl std::fmt::Debug for RawEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawEncoder")
            .field("name", &self.name)
            .field("vaapi", &self.hw.is_some())
            .field("gpu_converter", &self.converter.is_some())
            .finish()
    }
}

// SAFETY: the context and packet are exclusively owned.
unsafe impl Send for RawEncoder {}

impl RawEncoder {
    /// Whether an encoder named `name` exists in this FFmpeg build.
    pub fn exists(name: &str) -> bool {
        let Ok(cname) = CString::new(name) else {
            return false;
        };
        // SAFETY: valid C string.
        unsafe { !ffi::avcodec_find_encoder_by_name(cname.as_ptr()).is_null() }
    }

    /// Opens `name` with `params`; a VA-API encoder gets `hw` as its device
    /// and frame pool (or the converter's output pool). Colour is BT.709 studio range, GOP is `2*fps` with no
    /// B-frames, and a global header is requested.
    pub(crate) fn open(
        name: &'static str,
        params: &OpenParams,
        hw: Option<HwInput>,
    ) -> Result<Self> {
        let (hw, converter) = match hw {
            None => (None, None),
            Some(HwInput::Pool(frames)) => (Some(frames), None),
            Some(HwInput::Converter(frames, converter)) => (Some(frames), Some(converter)),
        };
        let cname =
            CString::new(name).map_err(|_| MediaError::Invalid("codec name has NUL".into()))?;
        // SAFETY: every pointer is checked before use; on any failure the
        // partially built context is freed before returning.
        unsafe {
            let codec = ffi::avcodec_find_encoder_by_name(cname.as_ptr());
            if codec.is_null() {
                return Err(MediaError::EncoderUnavailable(name.to_string()));
            }
            let ctx = ffi::avcodec_alloc_context3(codec);
            if ctx.is_null() {
                return Err(MediaError::Invalid("avcodec_alloc_context3 failed".into()));
            }
            let free = |mut ctx: *mut ffi::AVCodecContext| ffi::avcodec_free_context(&mut ctx);

            (*ctx).width = params.width as i32;
            (*ctx).height = params.height as i32;
            (*ctx).time_base = ffi::AVRational {
                num: 1,
                den: params.fps as i32,
            };
            (*ctx).framerate = ffi::AVRational {
                num: params.fps as i32,
                den: 1,
            };
            (*ctx).bit_rate = params.bitrate_bps;
            (*ctx).gop_size = params.gop_frames.unwrap_or(params.fps * 2) as i32;
            (*ctx).max_b_frames = 0;
            (*ctx).color_range = ffi::AVColorRange::AVCOL_RANGE_MPEG;
            (*ctx).color_primaries = ffi::AVColorPrimaries::AVCOL_PRI_BT709;
            (*ctx).color_trc = ffi::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
            (*ctx).colorspace = ffi::AVColorSpace::AVCOL_SPC_BT709;
            (*ctx).chroma_sample_location = ffi::AVChromaLocation::AVCHROMA_LOC_LEFT;
            if let Some(hw) = &hw {
                let (device, mut frames) = hw.new_refs();
                if let Some(converter) = &converter {
                    ffi::av_buffer_unref(&mut frames);
                    frames = converter.output_frames_ref();
                }
                if device.is_null() || frames.is_null() {
                    let (mut device, mut frames) = (device, frames);
                    ffi::av_buffer_unref(&mut device);
                    ffi::av_buffer_unref(&mut frames);
                    free(ctx);
                    return Err(MediaError::Invalid("hardware frames ref missing".into()));
                }
                (*ctx).hw_device_ctx = device;
                (*ctx).hw_frames_ctx = frames;
                (*ctx).pix_fmt = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
            } else {
                (*ctx).pix_fmt = params.pix_fmt;
            }
            for (key, value) in &params.options {
                let (Ok(k), Ok(v)) = (CString::new(*key), CString::new(value.as_str())) else {
                    continue;
                };
                if (*ctx).priv_data.is_null() {
                    break;
                }
                let ret = ffi::av_opt_set((*ctx).priv_data, k.as_ptr(), v.as_ptr(), 0);
                if ret < 0 {
                    tracing::debug!(encoder = name, option = key, "option not accepted");
                }
            }
            (*ctx).flags |= ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32;

            let ret = ffi::avcodec_open2(ctx, codec, ptr::null_mut());
            if ret < 0 {
                free(ctx);
                return Err(MediaError::Ffmpeg(ffmpeg_next::Error::from(ret)));
            }
            let pkt = ffi::av_packet_alloc();
            if pkt.is_null() {
                free(ctx);
                return Err(MediaError::Invalid("av_packet_alloc failed".into()));
            }
            Ok(Self {
                ctx,
                pkt,
                hw,
                converter,
                name,
            })
        }
    }

    /// The encoder name this context was opened with.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Whether frames go through the VA-API pool.
    pub fn is_vaapi(&self) -> bool {
        self.hw.is_some()
    }

    /// The VA-API pool, when present.
    pub fn vaapi(&self) -> Option<&VaapiFrames> {
        self.hw.as_ref()
    }

    /// Whether frames are converted on the GPU.
    pub(crate) fn has_converter(&self) -> bool {
        self.converter.is_some()
    }

    /// The GPU converter's source size, when one is active.
    pub(crate) fn converter_source_size(&self) -> Option<(u32, u32)> {
        self.converter.as_ref().map(VaapiConverter::source_size)
    }

    /// Replaces the converter with one for `src_w`x`src_h` (same output size).
    pub(crate) fn rebuild_converter(&mut self, src_w: u32, src_h: u32) -> Result<()> {
        let hw = self
            .hw
            .as_ref()
            .ok_or_else(|| MediaError::Invalid("no VA-API device".into()))?;
        let (w, h) = self.size();
        self.converter = Some(VaapiConverter::new(hw, src_w, src_h, w, h)?);
        Ok(())
    }

    /// Drops the GPU converter; frames must be uploaded from then on.
    pub(crate) fn drop_converter(&mut self) {
        self.converter = None;
    }

    /// Converts a BGRA frame into `out` on the GPU.
    pub(crate) fn convert(
        &mut self,
        bgra: &[u8],
        stride: usize,
        out: &mut OwnedFrame,
    ) -> Result<()> {
        match &mut self.converter {
            Some(c) => c.convert(bgra, stride, out),
            None => Err(MediaError::Invalid("no GPU converter".into())),
        }
    }

    /// The pixel format of frames to feed the encoder (NV12 for VA-API,
    /// else the context's format).
    pub fn input_format(&self) -> ffi::AVPixelFormat {
        if self.hw.is_some() {
            ffi::AVPixelFormat::AV_PIX_FMT_NV12
        } else {
            // SAFETY: valid context.
            unsafe { (*self.ctx).pix_fmt }
        }
    }

    /// Output size.
    pub fn size(&self) -> (u32, u32) {
        // SAFETY: valid context.
        unsafe { ((*self.ctx).width as u32, (*self.ctx).height as u32) }
    }

    /// Submits a frame (`None` to start draining).
    pub fn send_frame(&mut self, frame: Option<&OwnedFrame>) -> Result<()> {
        let ptr = frame.map_or(ptr::null(), |f| f.as_ptr() as *const ffi::AVFrame);
        // SAFETY: context valid; frame pointer valid or null.
        unsafe { super::check(ffi::avcodec_send_frame(self.ctx, ptr)).map(|_| ()) }
    }

    /// Receives every packet currently available.
    pub fn receive_packets(&mut self, mut sink: impl FnMut(RawPacket)) -> Result<()> {
        loop {
            // SAFETY: context and packet are valid; the packet is unreferenced
            // after its data has been copied out.
            let ret = unsafe { ffi::avcodec_receive_packet(self.ctx, self.pkt) };
            if super::is_drained(ret) {
                return Ok(());
            }
            super::check(ret)?;
            let packet = unsafe {
                let p = &*self.pkt;
                let packet = RawPacket {
                    data: packet_bytes(p),
                    pts: p.pts,
                    dts: p.dts,
                    keyframe: p.flags & ffi::AV_PKT_FLAG_KEY != 0,
                };
                ffi::av_packet_unref(self.pkt);
                packet
            };
            sink(packet);
        }
    }

    /// The codec's global header (SPS/PPS, hvcC, av1C).
    pub fn extradata(&self) -> Vec<u8> {
        // SAFETY: valid context; extradata is null or extradata_size bytes.
        unsafe {
            let c = &*self.ctx;
            if c.extradata.is_null() || c.extradata_size <= 0 {
                return Vec::new();
            }
            std::slice::from_raw_parts(c.extradata, c.extradata_size as usize).to_vec()
        }
    }

    /// The codec id.
    pub fn codec_id(&self) -> ffi::AVCodecID {
        // SAFETY: valid context with an open codec.
        unsafe { (*self.ctx).codec_id }
    }
}

impl Drop for RawEncoder {
    fn drop(&mut self) {
        // SAFETY: both pointers are owned and freed once; hw refs held by the
        // context are released by avcodec_free_context, ours by VaapiFrames.
        unsafe {
            ffi::av_packet_free(&mut self.pkt);
            ffi::avcodec_free_context(&mut self.ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_packet_copies_to_empty_vec() {
        // SAFETY: a freshly allocated packet has null data and size 0.
        unsafe {
            let pkt = ffi::av_packet_alloc();
            assert!(packet_bytes(&*pkt).is_empty());
            let mut pkt = pkt;
            ffi::av_packet_free(&mut pkt);
        }
    }
}
