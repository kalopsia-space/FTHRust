//! Read-only container inspection: the fields ffprobe would print for the
//! metadata rules, and per-packet timing for the CFR gate.

use std::ffi::CStr;
use std::path::Path;

use ffmpeg_next::ffi;

use crate::Result;

/// What a stream is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    /// A video stream.
    Video,
    /// An audio stream.
    Audio,
    /// Anything else (subtitles, data).
    Other,
}

/// One stream's container-level facts.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamDescription {
    /// Container stream index.
    pub index: usize,
    /// Stream kind.
    pub kind: StreamKind,
    /// FFmpeg codec name (`h264`, `aac`, ...).
    pub codec_name: String,
    /// Width (video), else 0.
    pub width: i32,
    /// Height (video), else 0.
    pub height: i32,
    /// `avg_frame_rate` as `(num, den)`.
    pub avg_frame_rate: (i32, i32),
    /// `r_frame_rate` as `(num, den)`.
    pub r_frame_rate: (i32, i32),
    /// Stream duration in seconds, when the container states it.
    pub duration_s: Option<f64>,
    /// Stream bitrate in bits per second (0 when unknown).
    pub bit_rate: i64,
    /// Channel count (audio), else 0.
    pub channels: i32,
    /// Sample rate (audio), else 0.
    pub sample_rate: i32,
    /// Stream metadata tags.
    pub tags: Vec<(String, String)>,
}

/// A container's facts.
#[derive(Clone, Debug, PartialEq)]
pub struct FormatDescription {
    /// Duration in seconds, when known.
    pub duration_s: Option<f64>,
    /// File size in bytes, when the I/O layer knows it.
    pub size: Option<u64>,
    /// Container bitrate (0 when unknown).
    pub bit_rate: i64,
    /// Container metadata tags.
    pub tags: Vec<(String, String)>,
    /// Streams in container order.
    pub streams: Vec<StreamDescription>,
}

fn tags(dict: ffmpeg_next::DictionaryRef<'_>) -> Vec<(String, String)> {
    dict.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn rational(r: ffmpeg_next::Rational) -> (i32, i32) {
    (r.numerator(), r.denominator())
}

/// Opens `path` and describes its container and streams.
pub fn describe(path: &Path) -> Result<FormatDescription> {
    crate::init()?;
    let ictx = ffmpeg_next::format::input(path)?;
    let duration = ictx.duration();
    let duration_s = (duration != ffi::AV_NOPTS_VALUE && duration > 0)
        .then(|| duration as f64 / f64::from(ffi::AV_TIME_BASE));
    // SAFETY: the input context and its pb are valid for the life of ictx.
    let size = unsafe {
        let pb = (*ictx.as_ptr()).pb;
        if pb.is_null() {
            None
        } else {
            let n = ffi::avio_size(pb);
            (n > 0).then_some(n as u64)
        }
    };
    let mut streams = Vec::new();
    for stream in ictx.streams() {
        let par = stream.parameters();
        // SAFETY: the parameters pointer belongs to a live stream.
        let (kind, codec_name, width, height, bit_rate, channels, sample_rate) = unsafe {
            let p = &*par.as_ptr();
            let kind = match p.codec_type {
                ffi::AVMediaType::AVMEDIA_TYPE_VIDEO => StreamKind::Video,
                ffi::AVMediaType::AVMEDIA_TYPE_AUDIO => StreamKind::Audio,
                _ => StreamKind::Other,
            };
            let name = ffi::avcodec_get_name(p.codec_id);
            let name = if name.is_null() {
                String::new()
            } else {
                CStr::from_ptr(name).to_string_lossy().into_owned()
            };
            (
                kind,
                name,
                p.width,
                p.height,
                p.bit_rate,
                p.ch_layout.nb_channels,
                p.sample_rate,
            )
        };
        let tb = stream.time_base();
        let d = stream.duration();
        let duration_s = (d != ffi::AV_NOPTS_VALUE && d > 0 && tb.denominator() != 0)
            .then(|| d as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator()));
        streams.push(StreamDescription {
            index: stream.index(),
            kind,
            codec_name,
            width,
            height,
            avg_frame_rate: rational(stream.avg_frame_rate()),
            r_frame_rate: rational(stream.rate()),
            duration_s,
            bit_rate,
            channels,
            sample_rate,
            tags: tags(stream.metadata()),
        });
    }
    Ok(FormatDescription {
        duration_s,
        size,
        bit_rate: ictx.bit_rate(),
        tags: tags(ictx.metadata()),
        streams,
    })
}

/// Timing of one packet in seconds: `pts_time` (`None` when the packet has
/// no pts) and `duration_time` (`None` when unknown, `Some(0.0)` when the
/// container says zero, as ffprobe prints it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PacketTiming {
    /// Presentation time in seconds.
    pub pts_s: Option<f64>,
    /// Packet duration in seconds.
    pub duration_s: Option<f64>,
}

/// Reads the timing of every packet of the first video stream without
/// decoding. `max_packets` bounds the work on absurd inputs.
pub fn video_packet_timing(path: &Path, max_packets: usize) -> Result<Vec<PacketTiming>> {
    crate::init()?;
    let mut ictx = ffmpeg_next::format::input(path)?;
    let Some(video) = ictx.streams().best(ffmpeg_next::media::Type::Video) else {
        return Ok(Vec::new());
    };
    let index = video.index();
    let tb = video.time_base();
    let scale = |ticks: i64| f64::from(tb.numerator()) * ticks as f64 / f64::from(tb.denominator());
    let mut out = Vec::new();
    for (stream, packet) in ictx.packets() {
        if stream.index() != index {
            continue;
        }
        if out.len() >= max_packets {
            break;
        }
        let duration = packet.duration();
        out.push(PacketTiming {
            pts_s: packet.pts().map(scale),
            duration_s: (duration != ffi::AV_NOPTS_VALUE).then(|| scale(duration)),
        });
    }
    Ok(out)
}
