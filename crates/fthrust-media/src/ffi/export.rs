//! FFmpeg calls the export runner needs that `ffmpeg-next` does not expose.

use ffmpeg_next as ff;

/// `(width, height, time_base)` of a configured `buffersink`/`abuffersink`.
pub fn buffersink_props(ctx: &mut ff::filter::Context) -> (u32, u32, ff::Rational) {
    // SAFETY: the context is a configured buffersink of a validated graph;
    // the getters only read its negotiated link.
    unsafe {
        let p = ctx.as_mut_ptr();
        let tb = ff::ffi::av_buffersink_get_time_base(p);
        (
            ff::ffi::av_buffersink_get_w(p).max(0) as u32,
            ff::ffi::av_buffersink_get_h(p).max(0) as u32,
            ff::Rational::new(tb.num, tb.den),
        )
    }
}

/// Clears the container-specific codec tag of a stream copied from another
/// container (MP4 and Matroska tags differ).
pub fn clear_codec_tag(ost: &mut ff::format::stream::StreamMut<'_>) {
    // SAFETY: plain field of the stream's own codec parameters.
    unsafe { (*ost.parameters().as_mut_ptr()).codec_tag = 0 };
}

/// Describes an H.264 stream fed with packets from the capture encoder:
/// `1/fps` time base, BT.709 limited range, the encoder's extradata.
pub fn set_h264_stream(
    ost: &mut ff::format::stream::StreamMut<'_>,
    codec_id: ff::codec::Id,
    width: u32,
    height: u32,
    fps: u32,
    extradata: &[u8],
) {
    // SAFETY: the stream and its codecpar are owned by the output context;
    // extradata is av_mallocz'd with the padding FFmpeg requires and owned
    // by codecpar afterwards.
    unsafe {
        let st = ost.as_mut_ptr();
        (*st).time_base = ff::ffi::AVRational {
            num: 1,
            den: fps as i32,
        };
        (*st).avg_frame_rate = ff::ffi::AVRational {
            num: fps as i32,
            den: 1,
        };
        let par = (*st).codecpar;
        (*par).codec_type = ff::ffi::AVMediaType::AVMEDIA_TYPE_VIDEO;
        (*par).codec_id = codec_id.into();
        (*par).width = width as i32;
        (*par).height = height as i32;
        (*par).format = ff::ffi::AVPixelFormat::AV_PIX_FMT_YUV420P as i32;
        (*par).color_range = ff::ffi::AVColorRange::AVCOL_RANGE_MPEG;
        (*par).color_primaries = ff::ffi::AVColorPrimaries::AVCOL_PRI_BT709;
        (*par).color_trc = ff::ffi::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
        (*par).color_space = ff::ffi::AVColorSpace::AVCOL_SPC_BT709;
        if !extradata.is_empty() {
            let pad = ff::ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize;
            let buf = ff::ffi::av_mallocz(extradata.len() + pad) as *mut u8;
            if !buf.is_null() {
                std::ptr::copy_nonoverlapping(extradata.as_ptr(), buf, extradata.len());
                (*par).extradata = buf;
                (*par).extradata_size = extradata.len() as i32;
            }
        }
    }
}
