//! Native FFmpeg AAC encoding of interleaved float32 PCM.

use std::ptr;

use ffmpeg_next::ffi;

use super::codec::{RawPacket, packet_bytes};
use crate::{MediaError, Result};

/// AAC encoder with a float32 interleaved → planar resampler in front.
#[derive(Debug)]
pub struct AacEncoder {
    ctx: *mut ffi::AVCodecContext,
    swr: *mut ffi::SwrContext,
    frame: *mut ffi::AVFrame,
    pkt: *mut ffi::AVPacket,
    frame_size: usize,
    channels: usize,
    sample_rate: u32,
    next_pts: i64,
    /// Interleaved samples waiting for a full frame.
    pending: Vec<f32>,
}

// SAFETY: every pointer is exclusively owned.
unsafe impl Send for AacEncoder {}

impl AacEncoder {
    /// Bitrate the reference uses.
    pub const BITRATE_BPS: i64 = 128_000;

    /// Opens the native `aac` encoder at `sample_rate` Hz and `channels`.
    pub fn new(sample_rate: u32, channels: u32, global_header: bool) -> Result<Self> {
        if sample_rate == 0 || channels == 0 {
            return Err(MediaError::Invalid(
                "audio sample rate and channel count must be non-zero".into(),
            ));
        }
        // SAFETY: each FFmpeg call is checked and everything allocated so
        // far is released on every failure path via `release`.
        unsafe {
            let codec = ffi::avcodec_find_encoder(ffi::AVCodecID::AV_CODEC_ID_AAC);
            if codec.is_null() {
                return Err(MediaError::EncoderUnavailable("aac".into()));
            }
            let mut this = Self {
                ctx: ffi::avcodec_alloc_context3(codec),
                swr: ptr::null_mut(),
                frame: ptr::null_mut(),
                pkt: ptr::null_mut(),
                frame_size: 0,
                channels: channels as usize,
                sample_rate,
                next_pts: 0,
                pending: Vec::new(),
            };
            if this.ctx.is_null() {
                return Err(MediaError::Invalid(
                    "avcodec_alloc_context3 (aac) failed".into(),
                ));
            }
            let ctx = this.ctx;
            (*ctx).sample_rate = sample_rate as i32;
            (*ctx).bit_rate = Self::BITRATE_BPS;
            (*ctx).sample_fmt = ffi::AVSampleFormat::AV_SAMPLE_FMT_FLTP;
            ffi::av_channel_layout_default(&mut (*ctx).ch_layout, channels as i32);
            (*ctx).time_base = ffi::AVRational {
                num: 1,
                den: sample_rate as i32,
            };
            if global_header {
                (*ctx).flags |= ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            }
            super::check(ffi::avcodec_open2(ctx, codec, ptr::null_mut()))
                .map_err(|e| MediaError::Invalid(format!("avcodec_open2 (aac): {e}")))?;

            let mut src_layout: ffi::AVChannelLayout = std::mem::zeroed();
            let mut dst_layout: ffi::AVChannelLayout = std::mem::zeroed();
            ffi::av_channel_layout_default(&mut src_layout, channels as i32);
            ffi::av_channel_layout_default(&mut dst_layout, channels as i32);
            let ret = ffi::swr_alloc_set_opts2(
                &mut this.swr,
                &dst_layout,
                ffi::AVSampleFormat::AV_SAMPLE_FMT_FLTP,
                sample_rate as i32,
                &src_layout,
                ffi::AVSampleFormat::AV_SAMPLE_FMT_FLT,
                sample_rate as i32,
                0,
                ptr::null_mut(),
            );
            if ret < 0 || this.swr.is_null() {
                return Err(MediaError::Invalid("swr_alloc_set_opts2 failed".into()));
            }
            super::check(ffi::swr_init(this.swr))
                .map_err(|e| MediaError::Invalid(format!("swr_init: {e}")))?;

            this.frame = ffi::av_frame_alloc();
            if this.frame.is_null() {
                return Err(MediaError::Invalid("av_frame_alloc (aac) failed".into()));
            }
            this.frame_size = (*ctx).frame_size.max(1) as usize;
            (*this.frame).nb_samples = this.frame_size as i32;
            (*this.frame).format = ffi::AVSampleFormat::AV_SAMPLE_FMT_FLTP as i32;
            super::check(ffi::av_channel_layout_copy(
                &mut (*this.frame).ch_layout,
                &(*ctx).ch_layout,
            ))?;
            (*this.frame).sample_rate = sample_rate as i32;
            super::check(ffi::av_frame_get_buffer(this.frame, 0))
                .map_err(|e| MediaError::Invalid(format!("av_frame_get_buffer (aac): {e}")))?;
            this.pkt = ffi::av_packet_alloc();
            if this.pkt.is_null() {
                return Err(MediaError::Invalid("av_packet_alloc (aac) failed".into()));
            }
            Ok(this)
        }
    }

    /// Sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Allocates codec parameters describing this encoder (caller frees via
    /// [`AacEncoder::free_parameters`]).
    pub(crate) fn parameters(&self) -> Result<*mut ffi::AVCodecParameters> {
        // SAFETY: ctx is open; the returned parameters are owned by the caller.
        unsafe {
            let par = ffi::avcodec_parameters_alloc();
            if par.is_null() {
                return Err(MediaError::Invalid(
                    "avcodec_parameters_alloc failed".into(),
                ));
            }
            if let Err(e) = super::check(ffi::avcodec_parameters_from_context(par, self.ctx)) {
                let mut par = par;
                ffi::avcodec_parameters_free(&mut par);
                return Err(e);
            }
            Ok(par)
        }
    }

    /// Frees parameters from [`AacEncoder::parameters`].
    pub(crate) fn free_parameters(mut par: *mut ffi::AVCodecParameters) {
        // SAFETY: par came from avcodec_parameters_alloc.
        unsafe { ffi::avcodec_parameters_free(&mut par) }
    }

    /// Encodes interleaved float32 samples; packets carry pts in samples.
    /// Samples are buffered until a full encoder frame is available (the
    /// AAC encoder only accepts a short frame as the last one).
    pub fn encode(&mut self, interleaved: &[f32], mut sink: impl FnMut(RawPacket)) -> Result<()> {
        self.pending.extend_from_slice(interleaved);
        let frame_samples = self.frame_size * self.channels;
        let mut offset = 0;
        while self.pending.len() - offset >= frame_samples {
            self.send_frame(offset, self.frame_size)?;
            self.receive(&mut sink)?;
            offset += frame_samples;
        }
        self.pending.drain(..offset);
        Ok(())
    }

    /// Sends `n` frames starting at `offset` samples into `pending`.
    fn send_frame(&mut self, offset: usize, n: usize) -> Result<()> {
        // SAFETY: frame and swr are valid; the source slice covers n frames
        // of `channels` samples each (checked by the callers).
        unsafe {
            super::check(ffi::av_frame_make_writable(self.frame))?;
            (*self.frame).nb_samples = n as i32;
            (*self.frame).pts = self.next_pts;
            let src = self.pending.as_ptr().add(offset) as *const u8;
            let src_data: [*const u8; 1] = [src];
            let converted = ffi::swr_convert(
                self.swr,
                (*self.frame).data.as_mut_ptr(),
                n as i32,
                src_data.as_ptr(),
                n as i32,
            );
            super::check(converted)?;
            super::check(ffi::avcodec_send_frame(self.ctx, self.frame))?;
        }
        self.next_pts += n as i64;
        Ok(())
    }

    /// Encodes the buffered remainder as the last (short) frame and drains
    /// the encoder at the end of the track.
    pub fn flush(&mut self, mut sink: impl FnMut(RawPacket)) -> Result<()> {
        let rest = self.pending.len() / self.channels;
        if rest > 0 {
            self.send_frame(0, rest)?;
            self.pending.clear();
            self.receive(&mut sink)?;
        }
        // SAFETY: ctx is valid; a null frame starts draining.
        let ret = unsafe { ffi::avcodec_send_frame(self.ctx, ptr::null()) };
        if ret < 0 && ret != ffi::AVERROR_EOF {
            return Err(MediaError::Ffmpeg(ffmpeg_next::Error::from(ret)));
        }
        self.receive(&mut sink)
    }

    fn receive(&mut self, sink: &mut impl FnMut(RawPacket)) -> Result<()> {
        loop {
            // SAFETY: ctx and pkt are valid; data is copied before unref.
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
                    keyframe: true,
                };
                ffi::av_packet_unref(self.pkt);
                packet
            };
            sink(packet);
        }
    }
}

impl Drop for AacEncoder {
    fn drop(&mut self) {
        // SAFETY: each pointer is owned and freed at most once (null-safe).
        unsafe {
            if !self.pkt.is_null() {
                ffi::av_packet_free(&mut self.pkt);
            }
            if !self.frame.is_null() {
                ffi::av_frame_free(&mut self.frame);
            }
            if !self.swr.is_null() {
                ffi::swr_free(&mut self.swr);
            }
            if !self.ctx.is_null() {
                ffi::avcodec_free_context(&mut self.ctx);
            }
        }
    }
}
