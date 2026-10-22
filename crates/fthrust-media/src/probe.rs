//! Media facts for the editor and the finalisation gate, ported from
//! `core/media_metadata.py`: configured FTHR tags beat packet estimates,
//! missing values stay missing, and a total bitrate derived from file size
//! is never reused as the video bitrate.

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::ffi::probe::{self as raw, StreamKind};

/// Where the presented frame rate came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FpsSource {
    /// The `fthr_frame_rate` tag an FTHR writer published.
    Configured,
    /// The container's `avg_frame_rate`.
    Average,
    /// The container's `r_frame_rate`.
    Real,
}

impl FpsSource {
    /// The ffprobe field name the reference reports.
    pub fn field_name(self) -> &'static str {
        match self {
            FpsSource::Configured => "fthr_frame_rate",
            FpsSource::Average => "avg_frame_rate",
            FpsSource::Real => "r_frame_rate",
        }
    }
}

/// What is known about a file's first video stream. Every field is
/// optional: a missing probe value is reported as missing, never guessed.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoMetadata {
    /// Duration in seconds.
    pub duration_seconds: Option<f64>,
    /// Width in pixels.
    pub width: Option<u32>,
    /// Height in pixels.
    pub height: Option<u32>,
    /// The frame rate to present (configured, else average, else real).
    pub average_fps: Option<f64>,
    /// The container's real (time-base) frame rate.
    pub real_fps: Option<f64>,
    /// Video bitrate in bits per second.
    pub video_bitrate_bps: Option<u64>,
    /// Total container bitrate in bits per second.
    pub total_bitrate_bps: Option<u64>,
    /// Which field supplied `average_fps`.
    pub fps_source: Option<FpsSource>,
}

/// Values at or above this cannot be a bitrate or a dimension; they are
/// treated as unknown instead of wrapping (the reference used unbounded
/// integers).
const MAX_INTEGER: f64 = 9_223_372_036_854_775_808.0;

fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                return None;
            }
            // Python's float() accepts a decimal or exponent form and the
            // words inf/nan; the latter are rejected below as non-finite.
            t.replace('_', "").parse::<f64>().ok()
        }
        _ => None,
    }
}

fn positive_float(value: &Value) -> Option<f64> {
    as_f64(value).filter(|v| v.is_finite() && *v > 0.0)
}

fn positive_int(value: &Value) -> Option<u64> {
    positive_float(value)
        .filter(|v| *v < MAX_INTEGER)
        .map(|v| v.trunc() as u64)
}

fn positive_u32(value: &Value) -> Option<u32> {
    positive_int(value).and_then(|v| u32::try_from(v).ok())
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `float(Fraction(text))`: an integer ratio, or a decimal with an
/// optional exponent.
fn parse_fraction(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let (sign, body) = match t.as_bytes()[0] {
        b'-' => (-1.0, &t[1..]),
        b'+' => (1.0, &t[1..]),
        _ => (1.0, t),
    };
    if let Some((n, d)) = body.split_once('/') {
        if !is_digits(n) || !is_digits(d) {
            return None;
        }
        let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
        if d == 0.0 {
            return None;
        }
        return Some(sign * n / d);
    }
    if !body
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-' | b'_'))
    {
        return None;
    }
    body.replace('_', "").parse::<f64>().ok().map(|v| sign * v)
}

/// A sane positive FPS from an ffprobe rational (`"30000/1001"`) or
/// decimal, or `None`. Rates above 1000 are time-base artefacts, not
/// presentation rates.
pub fn parse_frame_rate(value: &Value) -> Option<f64> {
    let text = match value {
        Value::Null => return None,
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if matches!(text.as_str(), "" | "N/A" | "0/0") {
        return None;
    }
    parse_fraction(&text).filter(|r| r.is_finite() && *r > 0.0 && *r <= 1000.0)
}

fn mapping(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

/// Parses the first real video stream from an ffprobe-shaped JSON
/// document. `file_size` feeds the total-bitrate fallback when the
/// container states neither a bitrate nor a size.
pub fn parse_ffprobe_video_metadata(
    document: &Value,
    file_size: Option<u64>,
) -> Option<VideoMetadata> {
    let doc = mapping(document)?;
    let streams = doc.get("streams")?.as_array()?;
    let video = streams
        .iter()
        .filter_map(mapping)
        .find(|s| s.get("codec_type").and_then(Value::as_str) == Some("video"))?;
    let empty = Map::new();
    let stream_tags = video.get("tags").and_then(mapping).unwrap_or(&empty);
    let format = doc.get("format").and_then(mapping).unwrap_or(&empty);
    let format_tags = format.get("tags").and_then(mapping).unwrap_or(&empty);
    let get = |m: &Map<String, Value>, k: &str| m.get(k).cloned().unwrap_or(Value::Null);

    let configured_fps = parse_frame_rate(&get(format_tags, "fthr_frame_rate"))
        .or_else(|| parse_frame_rate(&get(stream_tags, "fthr_frame_rate")));
    let average_fps = configured_fps.or_else(|| parse_frame_rate(&get(video, "avg_frame_rate")));
    let real_fps = parse_frame_rate(&get(video, "r_frame_rate"));
    let fps_source = if configured_fps.is_some() {
        Some(FpsSource::Configured)
    } else if average_fps.is_some() {
        Some(FpsSource::Average)
    } else if real_fps.is_some() {
        Some(FpsSource::Real)
    } else {
        None
    };
    let configured_bitrate = positive_int(&get(format_tags, "fthr_video_bitrate_bps"))
        .or_else(|| positive_int(&get(stream_tags, "fthr_video_bitrate_bps")));
    let presented_fps = average_fps.or(real_fps);

    let duration = positive_float(&get(video, "duration"))
        .or_else(|| positive_float(&get(format, "duration")));

    let mut total_bitrate = positive_int(&get(format, "bit_rate"));
    if total_bitrate.is_none()
        && let Some(duration) = duration
    {
        let size = positive_int(&get(format, "size")).or(file_size.filter(|s| *s > 0));
        if let Some(size) = size {
            // Explicitly the TOTAL container bitrate; never reused as video
            // bitrate because FTHR clips may carry many AAC stems.
            total_bitrate = Some((size as f64 * 8.0 / duration).round_ties_even() as u64);
        }
    }

    Some(VideoMetadata {
        duration_seconds: duration,
        width: positive_u32(&get(video, "width")),
        height: positive_u32(&get(video, "height")),
        average_fps: presented_fps,
        real_fps,
        video_bitrate_bps: configured_bitrate.or_else(|| positive_int(&get(video, "bit_rate"))),
        total_bitrate_bps: total_bitrate,
        fps_source,
    })
}

fn rational_text((n, d): (i32, i32)) -> String {
    format!("{n}/{d}")
}

fn tags_json(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    )
}

/// Builds the ffprobe-shaped document for a container description so the
/// file probe and the JSON parser share one rule set.
pub fn describe_as_ffprobe_json(desc: &raw::FormatDescription) -> Value {
    let streams: Vec<Value> = desc
        .streams
        .iter()
        .map(|s| {
            let mut m = Map::new();
            m.insert("index".into(), json!(s.index));
            m.insert(
                "codec_type".into(),
                json!(match s.kind {
                    StreamKind::Video => "video",
                    StreamKind::Audio => "audio",
                    StreamKind::Other => "data",
                }),
            );
            m.insert("codec_name".into(), json!(s.codec_name));
            if s.kind == StreamKind::Video {
                m.insert("width".into(), json!(s.width));
                m.insert("height".into(), json!(s.height));
                m.insert(
                    "avg_frame_rate".into(),
                    json!(rational_text(s.avg_frame_rate)),
                );
                m.insert("r_frame_rate".into(), json!(rational_text(s.r_frame_rate)));
            }
            if s.kind == StreamKind::Audio {
                m.insert("channels".into(), json!(s.channels));
                m.insert("sample_rate".into(), json!(s.sample_rate.to_string()));
            }
            if let Some(d) = s.duration_s {
                m.insert("duration".into(), json!(d));
            }
            if s.bit_rate > 0 {
                m.insert("bit_rate".into(), json!(s.bit_rate.to_string()));
            }
            m.insert("tags".into(), tags_json(&s.tags));
            Value::Object(m)
        })
        .collect();
    let mut format = Map::new();
    if let Some(d) = desc.duration_s {
        format.insert("duration".into(), json!(d));
    }
    if let Some(size) = desc.size {
        format.insert("size".into(), json!(size.to_string()));
    }
    if desc.bit_rate > 0 {
        format.insert("bit_rate".into(), json!(desc.bit_rate.to_string()));
    }
    format.insert("tags".into(), tags_json(&desc.tags));
    json!({"streams": streams, "format": Value::Object(format)})
}

/// Probes one file without guessing missing facts. `None` means the probe
/// was inconclusive (unreadable file, no video stream).
pub fn probe_video_metadata(path: &Path) -> Option<VideoMetadata> {
    let desc = match raw::describe(path) {
        Ok(d) => d,
        Err(e) => {
            tracing::debug!(path = %path.display(), "probe failed: {e}");
            return None;
        }
    };
    let file_size = std::fs::metadata(path).ok().map(|m| m.len());
    parse_ffprobe_video_metadata(&describe_as_ffprobe_json(&desc), file_size)
}

/// Whether the packet pts timeline has no hole larger than
/// `max(0.25 s, 4 frames)`. `None` when it cannot be judged (bad fps, fewer
/// than two packets, a non-finite pts).
pub fn evaluate_physical_video_timeline(pts_seconds: &[f64], expected_fps: f64) -> Option<bool> {
    if !expected_fps.is_finite()
        || expected_fps <= 0.0
        || expected_fps > 1000.0
        || pts_seconds.is_empty()
    {
        return None;
    }
    if pts_seconds.iter().any(|p| !p.is_finite()) {
        return None;
    }
    if pts_seconds.len() < 2 {
        return None;
    }
    let gaps: Vec<f64> = pts_seconds.windows(2).map(|w| w[1] - w[0]).collect();
    if gaps.iter().any(|g| *g <= 0.0) {
        return Some(false);
    }
    let largest = gaps.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let expected_duration = 1.0 / expected_fps;
    let bound = (expected_duration * 4.0).max(0.250);
    Some(largest.is_finite() && largest <= bound)
}

/// What one packet probe established for the finalisation gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CfrEvidence {
    /// `Some(true)` when every packet lasts `1/expected_fps`; `Some(false)`
    /// when a repair is needed; `None` when the probe was inconclusive.
    pub cfr: Option<bool>,
    /// Whether the pts timeline is free of large holes.
    pub physical_timeline_bounded: Option<bool>,
}

/// Packets beyond this are not read: a replay is bounded to a few hundred
/// seconds (the reference refused a 16 MiB ffprobe response instead).
pub const MAX_TIMING_PACKETS: usize = 500_000;

/// Probes duration and physical pts evidence in one pass.
pub fn probe_video_cfr_evidence(path: &Path, expected_fps: f64) -> CfrEvidence {
    let none = CfrEvidence {
        cfr: None,
        physical_timeline_bounded: None,
    };
    if !expected_fps.is_finite() || expected_fps <= 0.0 || expected_fps > 1000.0 {
        return none;
    }
    let packets = match raw::video_packet_timing(path, MAX_TIMING_PACKETS) {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => return none,
        Err(e) => {
            tracing::debug!(path = %path.display(), "packet probe failed: {e}");
            return none;
        }
    };
    let expected_duration = 1.0 / expected_fps;
    let tolerance = (expected_duration * 0.002).max(0.00005);
    let physical = packets
        .iter()
        .map(|p| {
            p.pts_s
                .map(|v| if v.is_finite() { v } else { f64::NAN })
                .unwrap_or(f64::NAN)
        })
        .collect::<Vec<_>>();
    let physical_timeline = evaluate_physical_video_timeline(&physical, expected_fps);
    for p in &packets {
        let Some(duration) = p
            .duration_s
            .map(|d| json!(d))
            .as_ref()
            .and_then(positive_float)
        else {
            return CfrEvidence {
                cfr: None,
                physical_timeline_bounded: physical_timeline,
            };
        };
        if (duration - expected_duration).abs() > tolerance {
            return CfrEvidence {
                cfr: Some(false),
                physical_timeline_bounded: physical_timeline,
            };
        }
    }
    if physical_timeline != Some(true) {
        return CfrEvidence {
            cfr: physical_timeline,
            physical_timeline_bounded: physical_timeline,
        };
    }
    CfrEvidence {
        cfr: Some(true),
        physical_timeline_bounded: Some(true),
    }
}

/// Convenience: `probe_video_cfr_evidence(...).cfr`.
pub fn probe_video_cfr(path: &Path, expected_fps: f64) -> Option<bool> {
    probe_video_cfr_evidence(path, expected_fps).cfr
}

fn trim_decimal(text: String) -> String {
    let t = text.trim_end_matches('0').trim_end_matches('.');
    t.to_string()
}

/// `"30 FPS"`, `"29.97 FPS"`, or `"Unavailable"`.
pub fn format_fps(value: Option<f64>) -> String {
    let Some(v) = value.filter(|v| v.is_finite() && *v > 0.0) else {
        return "Unavailable".into();
    };
    let rounded = v.round_ties_even();
    if (v - rounded).abs() < 0.005 {
        return format!("{rounded:.0} FPS");
    }
    format!("{} FPS", trim_decimal(format!("{v:.2}")))
}

/// `"6 Mbps"`, `"25.5 Mbps"`, `"800 kbps"`, or `"Unavailable"`.
pub fn format_bitrate(value_bps: Option<u64>) -> String {
    let Some(v) = value_bps.filter(|v| *v > 0) else {
        return "Unavailable".into();
    };
    if v >= 1_000_000 {
        return format!(
            "{} Mbps",
            trim_decimal(format!("{:.2}", v as f64 / 1_000_000.0))
        );
    }
    format!("{:.0} kbps", v as f64 / 1000.0)
}
