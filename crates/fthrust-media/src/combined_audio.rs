//! Which native audio streams a combined track needs, and the `amix`
//! filter that builds it. Ports `core/combined_audio_policy.py`: only add
//! sources absent from Default Mix so nothing is counted twice.

use std::path::Path;

use serde_json::Value;

use crate::ffi::probe::{self as raw, StreamKind};
use crate::{MediaError, Result};

/// An audio stream with both its container and audio-ordinal indexes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioStreamDescriptor {
    /// Index among all container streams.
    pub container_index: usize,
    /// Index among the audio streams only (`0:a:N`).
    pub audio_index: usize,
    /// `title` tag.
    pub title: Option<String>,
    /// `handler_name` tag.
    pub handler_name: Option<String>,
}

const SYSTEM_LABELS: [&str; 3] = ["system audio", "default mix", "system"];
const MICROPHONE_LABELS: [&str; 3] = ["microphone", "mic", "mic audio"];

fn source_type(entry: &Value) -> String {
    entry
        .get("source_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase()
}

fn manifest_stream_indexes(manifest: &Value) -> Vec<i64> {
    let mut out = Vec::new();
    let Some(sources) = manifest.get("sources").and_then(Value::as_array) else {
        return out;
    };
    for entry in sources {
        if !entry.is_object() || !matches!(source_type(entry).as_str(), "system" | "microphone") {
            continue;
        }
        if let Some(index) = entry.get("stream_index").and_then(Value::as_i64)
            && !out.contains(&index)
        {
            out.push(index);
        }
    }
    out
}

fn labels(stream: &AudioStreamDescriptor) -> Vec<String> {
    [&stream.title, &stream.handler_name]
        .into_iter()
        .flatten()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Audio ordinals for a combined track. A manifest is authoritative
/// (application-only stems select nothing because Default Mix already
/// has them); legacy clips use stream labels, falling back to every
/// stream only when none is labelled.
pub fn select_combined_audio_streams(
    manifest: Option<&Value>,
    streams: &[AudioStreamDescriptor],
) -> Vec<usize> {
    if let Some(manifest) = manifest {
        let indexes = manifest_stream_indexes(manifest);
        return streams
            .iter()
            .filter(|s| i64::try_from(s.container_index).is_ok_and(|i| indexes.contains(&i)))
            .map(|s| s.audio_index)
            .collect();
    }
    let labelled: Vec<usize> = streams
        .iter()
        .filter(|s| {
            labels(s).iter().any(|l| {
                SYSTEM_LABELS.contains(&l.as_str()) || MICROPHONE_LABELS.contains(&l.as_str())
            })
        })
        .map(|s| s.audio_index)
        .collect();
    if !labelled.is_empty() {
        return labelled;
    }
    streams.iter().map(|s| s.audio_index).collect()
}

/// The production filter for a combined native audio track. Each input is
/// normalised with `aresample=first_pts=0` so streams with different first
/// timestamps keep their shared timeline; a single input stays `anull`.
pub fn build_combined_audio_filter(audio_indexes: &[usize]) -> Result<String> {
    if audio_indexes.is_empty() {
        return Err(MediaError::Invalid(
            "at least one audio stream is required".into(),
        ));
    }
    let inputs: String = audio_indexes.iter().map(|i| format!("[0:a:{i}]")).collect();
    if audio_indexes.len() == 1 {
        return Ok(format!("{inputs}anull[aout]"));
    }
    let normalized: Vec<String> = audio_indexes
        .iter()
        .enumerate()
        .map(|(position, index)| {
            format!("[0:a:{index}]aresample=first_pts=0:async=0[audio_input_{position}]")
        })
        .collect();
    let mix_inputs: String = (0..audio_indexes.len())
        .map(|p| format!("[audio_input_{p}]"))
        .collect();
    let mix = format!(
        "{mix_inputs}amix=inputs={}:normalize=0:duration=longest:dropout_transition=0,alimiter=limit=0.98:level=disabled[aout]",
        audio_indexes.len()
    );
    Ok(format!("{};{mix}", normalized.join(";")))
}

/// Stable audio ordinals from ffprobe-shaped stream objects.
pub fn descriptors_from_ffprobe_streams(streams: &[Value]) -> Vec<AudioStreamDescriptor> {
    let mut out = Vec::new();
    let mut audio_index = 0;
    for stream in streams {
        if !stream.is_object() || stream.get("codec_type").and_then(Value::as_str) != Some("audio")
        {
            continue;
        }
        let Some(container_index) = stream.get("index").and_then(Value::as_u64) else {
            continue;
        };
        let tag = |k: &str| {
            stream
                .get("tags")
                .filter(|t| t.is_object())
                .and_then(|t| t.get(k))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        out.push(AudioStreamDescriptor {
            container_index: container_index as usize,
            audio_index,
            title: tag("title"),
            handler_name: tag("handler_name"),
        });
        audio_index += 1;
    }
    out
}

/// The audio streams of a file, in container order.
pub fn descriptors_from_file(path: &Path) -> Result<Vec<AudioStreamDescriptor>> {
    let desc = raw::describe(path)?;
    let mut out = Vec::new();
    for (audio_index, s) in desc
        .streams
        .iter()
        .filter(|s| s.kind == StreamKind::Audio)
        .enumerate()
    {
        let tag = |k: &str| {
            s.tags
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        out.push(AudioStreamDescriptor {
            container_index: s.index,
            audio_index,
            title: tag("title"),
            handler_name: tag("handler_name"),
        });
    }
    Ok(out)
}
