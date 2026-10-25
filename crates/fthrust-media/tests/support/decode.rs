//! H.264 decoding for the VA-API encoder tests (kept out of `mod.rs`, which
//! other crates include without `ffmpeg_next`).

/// Decodes H.264 `packets` (with `extradata` as the codec's global header)
/// up to and including output frame `index`, returning packed RGB24 and its
/// size.
pub fn decode_frame_rgb(
    extradata: &[u8],
    packets: &[fthrust_media::ring::EncodedPacket],
    index: usize,
) -> (Vec<u8>, u32, u32) {
    use ffmpeg_next::format::Pixel;
    use ffmpeg_next::software::scaling;
    fthrust_media::init().expect("ffmpeg init");
    let codec = ffmpeg_next::decoder::find(ffmpeg_next::codec::Id::H264).expect("h264 decoder");
    let mut ctx = ffmpeg_next::codec::Context::new_with_codec(codec);
    // SAFETY: the context is not open yet; av_malloc'd extradata is owned by it.
    unsafe {
        let raw = ctx.as_mut_ptr();
        let size = extradata.len();
        let buf = ffmpeg_next::ffi::av_mallocz(size + 64) as *mut u8;
        std::ptr::copy_nonoverlapping(extradata.as_ptr(), buf, size);
        (*raw).extradata = buf;
        (*raw).extradata_size = size as i32;
    }
    let mut decoder = ctx.decoder().video().expect("open decoder");
    let mut decoded = 0usize;
    let mut frame = ffmpeg_next::frame::Video::empty();
    let convert = |frame: &ffmpeg_next::frame::Video| {
        let mut rgb = ffmpeg_next::frame::Video::empty();
        let mut sws = scaling::Context::get(
            frame.format(),
            frame.width(),
            frame.height(),
            Pixel::RGB24,
            frame.width(),
            frame.height(),
            scaling::Flags::BILINEAR,
        )
        .expect("sws");
        sws.run(frame, &mut rgb).expect("sws run");
        let (w, h) = (frame.width(), frame.height());
        let mut out = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h as usize {
            out.extend_from_slice(&rgb.data(0)[y * rgb.stride(0)..][..w as usize * 3]);
        }
        (out, w, h)
    };
    for p in packets {
        let pkt = ffmpeg_next::Packet::copy(&p.data);
        decoder.send_packet(&pkt).expect("send packet");
        while decoder.receive_frame(&mut frame).is_ok() {
            if decoded == index {
                return convert(&frame);
            }
            decoded += 1;
        }
    }
    decoder.send_eof().ok();
    while decoder.receive_frame(&mut frame).is_ok() {
        if decoded == index {
            return convert(&frame);
        }
        decoded += 1;
    }
    panic!("only {decoded} frames decoded, wanted index {index}");
}
