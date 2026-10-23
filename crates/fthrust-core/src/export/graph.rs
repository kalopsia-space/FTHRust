//! The export graph (`ClipViewer._build_export_cmd`): several kept ranges
//! are trimmed and concatenated in one filter graph; a single range seeks
//! and either stream-copies or filters. The plan names its inputs `[0:v]`
//! and `[0:a:N]` exactly as the reference's `-filter_complex` does.

use super::filters::{
    MixSource, mix_filter, normalise_speed, speed_audio_chain, video_filter_chain,
};
use super::naming::speed_adjusted_duration;
use super::watermark::{WatermarkStyle, filters as watermark_filters};
use crate::editor::snapshot::Effects;

/// What to export.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportSpec {
    /// Kept ranges in source seconds.
    pub ranges_s: Vec<(f64, f64)>,
    /// Crop in source pixels.
    pub crop: Option<(u32, u32, u32, u32)>,
    /// Colour and effects.
    pub effects: Effects,
    /// Stretch ratio.
    pub stretch: f64,
    /// Speed.
    pub speed: f64,
    /// Preserve pitch.
    pub preserve_pitch: bool,
    /// The clip's audio sources (the editor's mix rows).
    pub sources: Vec<MixSource>,
    /// Master volume 0–100.
    pub master_percent: u8,
    /// The clip has manifest-identified tracks (`_audio_tracks`): mix them.
    pub has_manifest_tracks: bool,
    /// The clip has any audio stream (`_playback_sources` non-empty).
    pub has_audio: bool,
    /// The capture-card watermark, when enabled.
    pub watermark: Option<WatermarkStyle>,
    /// Audio bitrate for re-encoded audio.
    pub audio_bitrate_kbps: u32,
    /// Share preset filters appended after the chain.
    pub extra_video_filters: Vec<String>,
    /// Share preset: re-encode video even without filters.
    pub force_video_encode: bool,
    /// Share preset: re-encode audio even without filters.
    pub force_audio_encode: bool,
}

/// A stream selection (`-map`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MapRef {
    /// A graph output pad.
    Label(String),
    /// `0:v:0`.
    FirstVideo,
    /// `0:v?`.
    AllVideoOptional,
    /// `0:a?`.
    AllAudioOptional,
}

/// Video output handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoOut {
    /// Encode with the quality's encoder.
    Encode,
    /// Copy packets.
    Copy,
}

/// Audio output handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioOut {
    /// AAC (FLAC for lossless) at this rate.
    Encode {
        /// Bitrate.
        kbps: u32,
    },
    /// Copy packets.
    Copy,
    /// No audio stream in the output.
    None,
}

/// A runnable export: the in-process equivalent of the reference command.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphPlan {
    /// Input seek (`-ss`).
    pub seek_s: Option<f64>,
    /// Output length (`-t`).
    pub duration_s: Option<f64>,
    /// `-filter_complex`.
    pub filter_graph: Option<String>,
    /// Video map.
    pub video_map: MapRef,
    /// Audio map.
    pub audio_map: Option<MapRef>,
    /// Video handling.
    pub video: VideoOut,
    /// Audio handling.
    pub audio: AudioOut,
    /// `-af` (speed without a mix).
    pub audio_filter: Option<String>,
    /// `-shortest`.
    pub shortest: bool,
}

/// Builds the plan for `spec`.
pub fn plan(spec: &ExportSpec) -> Result<GraphPlan, String> {
    let ranges: Vec<(f64, f64)> = spec
        .ranges_s
        .iter()
        .filter(|(s, e)| e - s >= 0.001)
        .map(|&(s, e)| (s.max(0.0), e.max(0.0)))
        .collect();
    if ranges.is_empty() {
        return Err("at least one non-empty segment is required".into());
    }
    let speed = normalise_speed(spec.speed);
    let speed_audio = speed_audio_chain(speed, spec.preserve_pitch);
    let mut video_filters =
        video_filter_chain(spec.crop, &spec.effects, spec.stretch, false, speed);
    video_filters.extend(spec.extra_video_filters.iter().cloned());
    let watermark = spec.watermark.as_ref();
    let use_video_filter = !video_filters.is_empty() || watermark.is_some();
    let use_audio_filter = spec.has_manifest_tracks;
    let kbps = spec.audio_bitrate_kbps;

    if ranges.len() > 1 {
        let mut f: Vec<String> = Vec::new();
        let mut video_labels = Vec::new();
        for (i, (s, e)) in ranges.iter().enumerate() {
            let mut chain = vec![
                format!("trim=start={s:.6}:end={e:.6}"),
                "setpts=PTS-STARTPTS".into(),
            ];
            chain.extend(video_filters.iter().cloned());
            f.push(format!("[0:v]{}[v{i}]", chain.join(",")));
            video_labels.push(format!("[v{i}]"));
        }
        let n = ranges.len();
        let mut audio_labels = Vec::new();
        let mut has_audio = spec.has_audio;
        if use_audio_filter {
            let (mix, out) = mix_filter(&spec.sources, spec.master_percent, "amixed");
            f.extend(mix);
            has_audio = out.is_some();
            if let Some(out) = out {
                let split: String = (0..n).map(|i| format!("[amix{i}]")).collect();
                f.push(format!("{out}asplit={n}{split}"));
                for (i, (s, e)) in ranges.iter().enumerate() {
                    f.push(format!(
                        "[amix{i}]atrim=start={s:.6}:end={e:.6},asetpts=PTS-STARTPTS[a{i}]"
                    ));
                    audio_labels.push(format!("[a{i}]"));
                }
            }
        } else if has_audio {
            let split: String = (0..n).map(|i| format!("[adirect{i}]")).collect();
            f.push(format!("[0:a:0]asplit={n}{split}"));
            for (i, (s, e)) in ranges.iter().enumerate() {
                f.push(format!(
                    "[adirect{i}]atrim=start={s:.6}:end={e:.6},asetpts=PTS-STARTPTS[a{i}]"
                ));
                audio_labels.push(format!("[a{i}]"));
            }
        }
        let vlabel = if watermark.is_some() { "vbase" } else { "vout" };
        let with_audio = has_audio && !audio_labels.is_empty();
        if with_audio {
            let inputs: String = video_labels
                .iter()
                .zip(&audio_labels)
                .map(|(v, a)| format!("{v}{a}"))
                .collect();
            f.push(format!("{inputs}concat=n={n}:v=1:a=1[{vlabel}][aout]"));
        } else {
            f.push(format!(
                "{}concat=n={n}:v=1:a=0[{vlabel}]",
                video_labels.concat()
            ));
        }
        if let Some(w) = watermark {
            f.extend(watermark_filters(w, "vbase", "vout"));
        }
        let mut audio_map = None;
        if with_audio {
            audio_map = Some(MapRef::Label("aout".into()));
            if let Some(sa) = &speed_audio {
                f.push(format!("[aout]{sa}[aspeed]"));
                audio_map = Some(MapRef::Label("aspeed".into()));
            }
        }
        return Ok(GraphPlan {
            seek_s: None,
            duration_s: None,
            filter_graph: Some(f.join(";")),
            video_map: MapRef::Label("vout".into()),
            audio: if with_audio {
                AudioOut::Encode { kbps }
            } else {
                AudioOut::None
            },
            audio_map,
            video: VideoOut::Encode,
            audio_filter: None,
            shortest: with_audio,
        });
    }

    let (start, end) = ranges[0];
    let seek_s = Some(start);
    let duration_s = Some(speed_adjusted_duration(end - start, speed));
    let audio_present = spec.has_audio;
    if !use_video_filter && !use_audio_filter {
        if !spec.force_video_encode && !spec.force_audio_encode {
            return Ok(GraphPlan {
                seek_s,
                duration_s,
                filter_graph: None,
                video_map: MapRef::AllVideoOptional,
                audio_map: audio_present.then_some(MapRef::AllAudioOptional),
                video: VideoOut::Copy,
                audio: if audio_present {
                    AudioOut::Copy
                } else {
                    AudioOut::None
                },
                audio_filter: None,
                shortest: false,
            });
        }
        return Ok(GraphPlan {
            seek_s,
            duration_s,
            filter_graph: None,
            video_map: MapRef::FirstVideo,
            audio_map: audio_present.then_some(MapRef::AllAudioOptional),
            video: if spec.force_video_encode {
                VideoOut::Encode
            } else {
                VideoOut::Copy
            },
            audio: match (audio_present, spec.force_audio_encode) {
                (false, _) => AudioOut::None,
                (true, true) => AudioOut::Encode { kbps },
                (true, false) => AudioOut::Copy,
            },
            audio_filter: None,
            shortest: false,
        });
    }

    let mut f: Vec<String> = Vec::new();
    if use_video_filter {
        let base = if watermark.is_some() { "vbase" } else { "vout" };
        let chain = if video_filters.is_empty() {
            "null".to_owned()
        } else {
            video_filters.join(",")
        };
        f.push(format!("[0:v]{chain}[{base}]"));
        if let Some(w) = watermark {
            f.extend(watermark_filters(w, "vbase", "vout"));
        }
    }
    let mut audio_map = audio_present.then_some(MapRef::AllAudioOptional);
    let mut audio_filter = None;
    let audio;
    if use_audio_filter {
        let (mix, out) = mix_filter(&spec.sources, spec.master_percent, "aout");
        f.extend(mix);
        match out {
            // The reference maps `None` here; the output simply has no audio.
            None => {
                audio_map = None;
                audio = AudioOut::None;
            }
            Some(out) => {
                let label = out.trim_matches(|c| c == '[' || c == ']').to_owned();
                if let Some(sa) = &speed_audio {
                    f.push(format!("{out}{sa}[aspeed]"));
                    audio_map = Some(MapRef::Label("aspeed".into()));
                } else {
                    audio_map = Some(MapRef::Label(label));
                }
                audio = AudioOut::Encode { kbps };
            }
        }
    } else if !audio_present {
        audio_map = None;
        audio = AudioOut::None;
    } else if speed_audio.is_some() || spec.force_audio_encode {
        audio_filter = speed_audio.clone();
        audio = AudioOut::Encode { kbps };
    } else {
        audio = AudioOut::Copy;
    }
    Ok(GraphPlan {
        seek_s,
        duration_s,
        filter_graph: Some(f.join(";")),
        video_map: if use_video_filter {
            MapRef::Label("vout".into())
        } else {
            MapRef::FirstVideo
        },
        audio_map,
        video: if use_video_filter || spec.force_video_encode {
            VideoOut::Encode
        } else {
            VideoOut::Copy
        },
        audio,
        audio_filter,
        shortest: false,
    })
}
