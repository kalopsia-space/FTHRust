//! The playback video decoder: RGBA frames with presentation times,
//! keyframe-aware seeking that decodes forward to the requested position,
//! and one-shot frame extraction for paused seeks.

use std::path::Path;

use ffmpeg_next::format::Pixel;
use ffmpeg_next::media::Type;
use ffmpeg_next::software::scaling::{Context as Scaler, Flags};
use ffmpeg_next::{codec, format, frame};

use crate::ffi::scaler::SendScaler;
use crate::{MediaError, Result};

/// Decoder threads in the UI process: 1080p180 H.264 decodes at about 2.6x
/// real time with four (1.7x with one), for some 27 MB more than one thread;
/// FFmpeg's automatic count is faster still but holds twice the frames.
const PLAYBACK_DECODE_THREADS: usize = 4;
/// What the stream says about itself.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frames per second (average).
    pub fps: f64,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

/// One decoded frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Presentation time.
    pub pts_ms: u64,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Tightly packed RGBA rows.
    pub rgba: Vec<u8>,
}

/// The decoder.
pub struct VideoDecoder {
    ictx: format::context::Input,
    index: usize,
    time_base: f64,
    decoder: codec::decoder::Video,
    scaler: Option<SendScaler>,
    /// The colour space and range the scaler was configured for.
    scaler_colour: Option<(
        ffmpeg_next::util::color::Space,
        ffmpeg_next::util::color::Range,
    )>,
    rgba: Option<frame::Video>,
    info: VideoInfo,
    /// After a seek: frames before this time are skipped.
    target_ms: Option<u64>,
    /// The newest skipped frame, unconverted (returned when the target is
    /// past the end).
    last_before: Option<frame::Video>,
    /// RGBA conversions so far (for tests).
    conversions: u64,
    packets_done: bool,
    eof: bool,
}

impl VideoDecoder {
    /// Opens the best video stream of `path`.
    pub fn open(path: &Path) -> Result<VideoDecoder> {
        crate::init()?;
        let ictx = format::input(path)?;
        let stream = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| MediaError::Invalid("no video stream".into()))?;
        let index = stream.index();
        let tb = stream.time_base();
        let time_base = f64::from(tb.numerator()) / f64::from(tb.denominator());
        let rate = stream.avg_frame_rate();
        let fps = if rate.denominator() > 0 {
            f64::from(rate.numerator()) / f64::from(rate.denominator())
        } else {
            0.0
        };
        let mut ctx = codec::context::Context::from_parameters(stream.parameters())?;
        ctx.set_threading(crate::frame_threads(PLAYBACK_DECODE_THREADS));
        let decoder = ctx.decoder().video()?;
        let duration_ms = if ictx.duration() > 0 {
            (ictx.duration() as f64 * 1000.0 / f64::from(ffmpeg_next::ffi::AV_TIME_BASE)).round()
                as u64
        } else if stream.duration() > 0 {
            (stream.duration() as f64 * time_base * 1000.0).round() as u64
        } else {
            0
        };
        let info = VideoInfo {
            width: decoder.width(),
            height: decoder.height(),
            fps,
            duration_ms,
        };
        Ok(VideoDecoder {
            ictx,
            index,
            time_base,
            decoder,
            scaler: None,
            scaler_colour: None,
            rgba: None,
            info,
            target_ms: None,
            last_before: None,
            conversions: 0,
            packets_done: false,
            eof: false,
        })
    }

    /// Stream metadata.
    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// Frames converted to RGBA so far.
    pub fn conversions(&self) -> u64 {
        self.conversions
    }

    fn pts_ms_of(&self, decoded: &frame::Video) -> u64 {
        let pts = decoded.pts().or(decoded.timestamp()).unwrap_or(0).max(0);
        (pts as f64 * self.time_base * 1000.0).round() as u64
    }

    /// Seeks so the next frame is the first at or after `position_ms`
    /// (the last frame when the position is past the end).
    pub fn seek(&mut self, position_ms: u64) -> Result<()> {
        let ts = (position_ms as i64) * i64::from(ffmpeg_next::ffi::AV_TIME_BASE) / 1000;
        if let Err(e) = self.ictx.seek(ts, ..ts) {
            tracing::debug!("video seek failed, decoding from the start: {e}");
            let _ = self.ictx.seek(0, ..0);
        }
        self.decoder.flush();
        self.target_ms = Some(position_ms);
        self.last_before = None;
        self.packets_done = false;
        self.eof = false;
        Ok(())
    }

    fn convert(&mut self, decoded: &frame::Video) -> Result<Frame> {
        let (w, h) = (decoded.width(), decoded.height());
        let colour = (decoded.color_space(), decoded.color_range());
        let needs_new = self.scaler_colour != Some(colour)
            || self.scaler.as_ref().is_none_or(|s| {
                s.0.input().width != w
                    || s.0.input().height != h
                    || s.0.input().format != decoded.format()
            });
        if needs_new {
            self.scaler = Some(SendScaler(Scaler::get(
                decoded.format(),
                w,
                h,
                Pixel::RGBA,
                w,
                h,
                Flags::BILINEAR,
            )?));
            if let Some(s) = self.scaler.as_mut() {
                crate::ffi::scaler::match_source_colorspace(&mut s.0, decoded);
            }
            self.scaler_colour = Some(colour);
        }
        let out = self
            .rgba
            .get_or_insert_with(|| frame::Video::new(Pixel::RGBA, w, h));
        if out.width() != w || out.height() != h {
            *out = frame::Video::new(Pixel::RGBA, w, h);
        }
        self.scaler.as_mut().expect("scaler").0.run(decoded, out)?;
        let stride = out.stride(0);
        let data = out.data(0);
        let row = (w * 4) as usize;
        let mut rgba = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            rgba.extend_from_slice(&data[y * stride..y * stride + row]);
        }
        let pts_ms = self.pts_ms_of(decoded);
        self.conversions += 1;
        Ok(Frame {
            pts_ms,
            width: w,
            height: h,
            rgba,
        })
    }

    /// Receives every frame the decoder has ready; returns the first that
    /// passes the seek target, keeping the newest skipped one.
    fn receive(&mut self) -> Result<Option<Frame>> {
        let mut decoded = frame::Video::empty();
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            match self.target_ms {
                Some(target) if self.pts_ms_of(&decoded) < target => {
                    // Skipped: keep it unconverted in case the target is past the end.
                    self.last_before = Some(std::mem::replace(&mut decoded, frame::Video::empty()));
                }
                _ => {
                    self.target_ms = None;
                    self.last_before = None;
                    return Ok(Some(self.convert(&decoded)?));
                }
            }
        }
        Ok(None)
    }

    /// The next frame in presentation order; `None` at the end.
    pub fn next_frame(&mut self) -> Result<Option<Frame>> {
        if self.eof {
            return Ok(None);
        }
        loop {
            if let Some(f) = self.receive()? {
                return Ok(Some(f));
            }
            if self.packets_done {
                self.eof = true;
                // A seek past the end lands on the last frame.
                return match self.last_before.take() {
                    Some(last) => Ok(Some(self.convert(&last)?)),
                    None => Ok(None),
                };
            }
            let mut packet = ffmpeg_next::Packet::empty();
            match packet.read(&mut self.ictx) {
                Ok(()) => {
                    if packet.stream() != self.index {
                        continue;
                    }
                    if self.decoder.send_packet(&packet).is_err() {
                        continue;
                    }
                }
                Err(_) => {
                    self.packets_done = true;
                    let _ = self.decoder.send_eof();
                }
            }
        }
    }
}

/// One frame at `position_ms` (the first at or after it; the last frame
/// past the end), for paused seeks and poster frames.
pub fn frame_at(path: &Path, position_ms: u64) -> Result<Frame> {
    let mut dec = VideoDecoder::open(path)?;
    dec.seek(position_ms)?;
    dec.next_frame()?
        .ok_or_else(|| MediaError::Invalid("no decodable video frame".into()))
}
