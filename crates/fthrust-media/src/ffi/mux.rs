//! MP4 muxing (plain or fragmented) with the exact stream parameters the
//! reference writes.

use std::ffi::CString;
use std::ptr;

use ffmpeg_next::ffi;

use super::aac::AacEncoder;
use crate::{MediaError, Result};

/// `AVFMT_AVOID_NEG_TS_DISABLED`.
const AVOID_NEG_TS_DISABLED: i32 = 0;

/// An MP4 output context.
#[derive(Debug)]
pub struct Muxer {
    fmt: *mut ffi::AVFormatContext,
    path: CString,
    fragmented: bool,
    header_written: bool,
    finished: bool,
}

// SAFETY: the context is exclusively owned.
unsafe impl Send for Muxer {}

impl Muxer {
    /// Creates an MP4 output for `path`. `fragmented` writes a fragmented
    /// MP4 that stays playable without its trailer; otherwise a plain MP4
    /// with `avoid_negative_ts` disabled so pre-roll packets keep negative
    /// pts.
    pub fn new(path: &std::path::Path, fragmented: bool) -> Result<Self> {
        let cpath = CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| MediaError::Invalid("output path has NUL".into()))?;
        let mut fmt: *mut ffi::AVFormatContext = ptr::null_mut();
        let mp4 = c"mp4";
        // SAFETY: out-pointer and C strings are valid; null checked.
        unsafe {
            super::check(ffi::avformat_alloc_output_context2(
                &mut fmt,
                ptr::null(),
                mp4.as_ptr(),
                cpath.as_ptr(),
            ))
            .map_err(|e| MediaError::Invalid(format!("avformat_alloc_output_context2: {e}")))?;
            if fmt.is_null() {
                return Err(MediaError::Invalid("MP4 output context is null".into()));
            }
            (*fmt).avoid_negative_ts = AVOID_NEG_TS_DISABLED;
            if fragmented {
                // Bounded interleave buffering so completed fragments reach
                // the filesystem even while one stream is quiet.
                (*fmt).max_interleave_delta = 1_000_000;
                (*fmt).flags |= ffi::AVFMT_FLAG_FLUSH_PACKETS;
            }
        }
        Ok(Self {
            fmt,
            path: cpath,
            fragmented,
            header_written: false,
            finished: false,
        })
    }

    /// Sets a container metadata tag.
    pub fn set_metadata(&mut self, key: &str, value: &str) -> Result<()> {
        let (k, v) = (
            CString::new(key).map_err(|_| MediaError::Invalid("metadata key has NUL".into()))?,
            CString::new(value)
                .map_err(|_| MediaError::Invalid("metadata value has NUL".into()))?,
        );
        // SAFETY: fmt is valid; av_dict_set copies the strings.
        unsafe {
            super::check(ffi::av_dict_set(
                &mut (*self.fmt).metadata,
                k.as_ptr(),
                v.as_ptr(),
                0,
            ))
            .map(|_| ())
        }
    }

    /// Adds the video stream (time base `1/time_base_den`, frame rate `fps`,
    /// BT.709 studio range, extradata copied with padding) and returns its
    /// index.
    #[allow(clippy::too_many_arguments)]
    pub fn add_video_stream(
        &mut self,
        codec_id: ffi::AVCodecID,
        width: u32,
        height: u32,
        fps: u32,
        time_base_den: i32,
        bitrate_bps: i64,
        extradata: &[u8],
    ) -> Result<usize> {
        // SAFETY: fmt valid; the new stream and its codecpar are owned by fmt;
        // extradata is av_malloc'd with padding as FFmpeg requires.
        unsafe {
            let stream = ffi::avformat_new_stream(self.fmt, ptr::null());
            if stream.is_null() {
                return Err(MediaError::Invalid(
                    "avformat_new_stream (video) failed".into(),
                ));
            }
            (*stream).id = 0;
            (*stream).time_base = ffi::AVRational {
                num: 1,
                den: time_base_den,
            };
            (*stream).avg_frame_rate = ffi::AVRational {
                num: fps as i32,
                den: 1,
            };
            (*stream).r_frame_rate = (*stream).avg_frame_rate;
            let par = (*stream).codecpar;
            (*par).codec_type = ffi::AVMediaType::AVMEDIA_TYPE_VIDEO;
            (*par).codec_id = codec_id;
            (*par).width = width as i32;
            (*par).height = height as i32;
            (*par).format = ffi::AVPixelFormat::AV_PIX_FMT_YUV420P as i32;
            (*par).color_range = ffi::AVColorRange::AVCOL_RANGE_MPEG;
            (*par).color_primaries = ffi::AVColorPrimaries::AVCOL_PRI_BT709;
            (*par).color_trc = ffi::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
            (*par).color_space = ffi::AVColorSpace::AVCOL_SPC_BT709;
            (*par).chroma_location = ffi::AVChromaLocation::AVCHROMA_LOC_LEFT;
            if bitrate_bps > 0 {
                (*par).bit_rate = bitrate_bps;
            }
            if !extradata.is_empty() {
                let padded = extradata.len() + ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize;
                let buf = ffi::av_malloc(padded) as *mut u8;
                if !buf.is_null() {
                    ptr::copy_nonoverlapping(extradata.as_ptr(), buf, extradata.len());
                    ptr::write_bytes(
                        buf.add(extradata.len()),
                        0,
                        ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize,
                    );
                    (*par).extradata = buf;
                    (*par).extradata_size = extradata.len() as i32;
                }
            }
            Ok((*stream).index as usize)
        }
    }

    /// Whether the container wants global headers from its encoders.
    pub fn wants_global_header(&self) -> bool {
        // SAFETY: fmt and oformat are valid.
        unsafe { (*(*self.fmt).oformat).flags & ffi::AVFMT_GLOBALHEADER != 0 }
    }

    /// Adds an AAC audio stream described by `aac` and returns its index.
    /// The stream is only created once the parameters were built, so a
    /// failure here never leaves an empty stream in the container.
    pub fn add_audio_stream(&mut self, aac: &AacEncoder) -> Result<usize> {
        let par = aac.parameters()?;
        // SAFETY: fmt valid; par is owned until freed below; the new stream's
        // codecpar receives a copy.
        let result = unsafe {
            let stream = ffi::avformat_new_stream(self.fmt, ptr::null());
            if stream.is_null() {
                Err(MediaError::Invalid(
                    "avformat_new_stream (audio) failed".into(),
                ))
            } else {
                match super::check(ffi::avcodec_parameters_copy((*stream).codecpar, par)) {
                    Ok(_) => {
                        (*stream).id = 1;
                        (*stream).time_base = ffi::AVRational {
                            num: 1,
                            den: aac.sample_rate() as i32,
                        };
                        Ok((*stream).index as usize)
                    }
                    Err(e) => Err(MediaError::Invalid(format!(
                        "avcodec_parameters_copy (audio): {e}"
                    ))),
                }
            }
        };
        AacEncoder::free_parameters(par);
        result
    }

    /// Opens the file and writes the header (`movflags=use_metadata_tags`,
    /// plus fragment flags for fragmented output).
    pub fn write_header(&mut self) -> Result<()> {
        // SAFETY: fmt valid; the options dictionary is freed after the call.
        unsafe {
            if (*(*self.fmt).oformat).flags & ffi::AVFMT_NOFILE == 0 {
                super::check(ffi::avio_open(&mut (*self.fmt).pb, self.path.as_ptr(), ffi::AVIO_FLAG_WRITE)).map_err(|e| {
                    MediaError::Invalid(format!(
                        "Could not open the temporary clip for writing; check free disk space and folder permissions: {e}"
                    ))
                })?;
            }
            let mut opts: *mut ffi::AVDictionary = ptr::null_mut();
            // frag_custom lets the recorder close a fragment explicitly at
            // every keyframe; empty_moov writes the recovery metadata up
            // front instead of only at the trailer.
            let flags = if self.fragmented {
                c"empty_moov+frag_custom+default_base_moof+omit_tfhd_offset+use_metadata_tags"
            } else {
                c"use_metadata_tags"
            };
            ffi::av_dict_set(&mut opts, c"movflags".as_ptr(), flags.as_ptr(), 0);
            if self.fragmented {
                ffi::av_dict_set(&mut opts, c"flush_packets".as_ptr(), c"1".as_ptr(), 0);
            }
            let ret = ffi::avformat_write_header(self.fmt, &mut opts);
            ffi::av_dict_free(&mut opts);
            super::check(ret)
                .map_err(|e| MediaError::Invalid(format!("Failed to write the MP4 header: {e}")))?;
            if self.fragmented && !(*self.fmt).pb.is_null() {
                ffi::avio_flush((*self.fmt).pb);
            }
        }
        self.header_written = true;
        Ok(())
    }

    /// Writes one packet whose timestamps are in `src_time_base`
    /// (`(num, den)`), rescaling to the stream's time base.
    #[allow(clippy::too_many_arguments)]
    pub fn write_packet(
        &mut self,
        stream_index: usize,
        data: &[u8],
        pts: i64,
        dts: i64,
        duration: i64,
        keyframe: bool,
        src_time_base: (i32, i32),
    ) -> Result<()> {
        if !self.header_written {
            return Err(MediaError::Invalid(
                "write_packet before write_header".into(),
            ));
        }
        // SAFETY: the packet borrows `data` only for the duration of the
        // interleaved write (av_interleaved_write_frame may keep a reference
        // only when the packet is refcounted, which a borrowed buffer is not:
        // it copies), and is freed afterwards.
        unsafe {
            let pkt = ffi::av_packet_alloc();
            if pkt.is_null() {
                return Err(MediaError::Invalid("av_packet_alloc failed".into()));
            }
            let stream = *(*self.fmt).streams.add(stream_index);
            (*pkt).data = data.as_ptr() as *mut u8;
            (*pkt).size = data.len() as i32;
            (*pkt).pts = pts;
            (*pkt).dts = dts;
            (*pkt).duration = duration;
            (*pkt).stream_index = stream_index as i32;
            if keyframe {
                (*pkt).flags |= ffi::AV_PKT_FLAG_KEY;
            }
            ffi::av_packet_rescale_ts(
                pkt,
                ffi::AVRational {
                    num: src_time_base.0,
                    den: src_time_base.1,
                },
                (*stream).time_base,
            );
            let ret = ffi::av_interleaved_write_frame(self.fmt, pkt);
            let mut pkt = pkt;
            (*pkt).data = ptr::null_mut();
            (*pkt).size = 0;
            ffi::av_packet_free(&mut pkt);
            super::check(ret).map(|_| ())
        }
    }

    /// Drains the interleave queue and closes the current fragment so
    /// everything written so far is on disk and decodable (fragmented
    /// output only).
    pub fn flush_fragment(&mut self) -> Result<()> {
        if !self.fragmented || !self.header_written {
            return Err(MediaError::Invalid(
                "flush_fragment needs a fragmented, started muxer".into(),
            ));
        }
        // SAFETY: fmt and pb are valid; null packets are the documented way
        // to drain the queue and (with frag_custom) end a fragment.
        unsafe {
            if (*self.fmt).pb.is_null() {
                return Err(MediaError::Invalid("recording file is closed".into()));
            }
            super::check(ffi::av_interleaved_write_frame(self.fmt, ptr::null_mut()))?;
            super::check(ffi::av_write_frame(self.fmt, ptr::null_mut()))?;
            ffi::avio_flush((*self.fmt).pb);
            let err = (*(*self.fmt).pb).error;
            if err < 0 {
                return Err(MediaError::Ffmpeg(ffmpeg_next::Error::from(err)));
            }
        }
        Ok(())
    }

    /// Writes the trailer and closes the file. Later calls are no-ops.
    pub fn finish(&mut self) -> Result<()> {
        if !self.header_written {
            return Err(MediaError::Invalid("finish before write_header".into()));
        }
        if self.finished {
            return Ok(());
        }
        // SAFETY: fmt valid; the trailer is written once.
        let trailer = unsafe { super::check(ffi::av_write_trailer(self.fmt)) };
        self.finished = true;
        let close = self.close_io();
        trailer.map_err(|e| {
            MediaError::Invalid(format!("Failed to finalize the MP4 container: {e}"))
        })?;
        close
    }

    fn close_io(&mut self) -> Result<()> {
        // SAFETY: fmt valid; avio_closep is null-safe.
        unsafe {
            if (*self.fmt).pb.is_null() || (*(*self.fmt).oformat).flags & ffi::AVFMT_NOFILE != 0 {
                return Ok(());
            }
            super::check(ffi::avio_closep(&mut (*self.fmt).pb))
                .map(|_| ())
                .map_err(|e| MediaError::Invalid(format!("Failed to close the clip file: {e}")))
        }
    }
}

impl Drop for Muxer {
    fn drop(&mut self) {
        if !self.fmt.is_null() {
            let _ = self.close_io();
            // SAFETY: fmt is owned and freed once.
            unsafe { ffi::avformat_free_context(self.fmt) }
            self.fmt = ptr::null_mut();
        }
    }
}
