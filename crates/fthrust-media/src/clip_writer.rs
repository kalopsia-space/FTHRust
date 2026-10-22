//! Writes a replay snapshot as a plain MP4 through the transactional save.
//!
//! Ports `save_clip.cpp`: pre-roll packets before `presentation_start_pts`
//! get negative pts so the clip presents from t=0 while the prior keyframe
//! stays decodable; audio is native AAC at 128 kbps; an audio setup failure
//! degrades to video-only and is reported to the caller (the reference
//! stayed silent about it).

use std::path::{Path, PathBuf};

use crate::ffi::aac::AacEncoder;
use crate::ffi::mux::Muxer;
use crate::ring::Snapshot;
use crate::transactional_save::{self, SaveError};

/// The video side of a clip.
#[derive(Clone, Copy, Debug)]
pub struct ClipVideo<'a> {
    /// The ring snapshot to write.
    pub snapshot: &'a Snapshot,
    /// Codec of the packets.
    pub codec_id: ffmpeg_next::codec::Id,
    /// Global header from the encoder.
    pub extradata: &'a [u8],
    /// Frame width.
    pub width: u32,
    /// Frame height.
    pub height: u32,
    /// Configured bitrate, recorded as metadata.
    pub bitrate_kbps: u32,
    /// Whether the clip's audio layout is "separated" (metadata only).
    pub separate_audio: bool,
}

/// Interleaved float32 PCM for the clip's audio track.
#[derive(Clone, Copy, Debug)]
pub struct ClipAudio<'a> {
    /// Interleaved samples.
    pub samples: &'a [f32],
    /// Sample rate.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u32,
    /// Stream title (`title` and `handler_name` tags), e.g. `Default Mix`.
    pub title: &'a str,
}

/// What a successful write produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipWriteResult {
    /// The published path.
    pub path: PathBuf,
    /// Video payload bytes written.
    pub bytes_written: u64,
    /// Whether an audio track was written.
    pub audio_included: bool,
    /// Why a track was dropped, when one was requested but failed to set up.
    pub audio_error: Option<String>,
    /// Titles of the tracks written, in stream order.
    pub audio_tracks: Vec<String>,
}

/// Writes the clip to `final_path` transactionally. `progress` receives the
/// cumulative video bytes written.
pub fn write_clip(
    final_path: &Path,
    video: &ClipVideo<'_>,
    tracks: &[ClipAudio<'_>],
    progress: Option<&dyn Fn(u64)>,
) -> Result<ClipWriteResult, SaveError> {
    let mut outcome = ClipWriteResult {
        path: final_path.to_path_buf(),
        bytes_written: 0,
        audio_included: false,
        audio_error: None,
        audio_tracks: Vec::new(),
    };
    let result = transactional_save::run(final_path, |temporary| {
        write_temporary(temporary, video, tracks, progress, &mut outcome).map_err(|e| e.to_string())
    });
    result.map(|path| ClipWriteResult { path, ..outcome })
}

fn write_temporary(
    path: &Path,
    video: &ClipVideo<'_>,
    tracks: &[ClipAudio<'_>],
    progress: Option<&dyn Fn(u64)>,
    outcome: &mut ClipWriteResult,
) -> crate::Result<()> {
    crate::init()?;
    let snapshot = video.snapshot;
    if snapshot.packets.is_empty() {
        return Err(crate::MediaError::Invalid(
            "Nothing to save: the replay buffer contains no video packets".into(),
        ));
    }
    let fps = snapshot.fps.max(1);
    let mut mux = Muxer::new(path, false)?;
    mux.set_metadata(
        "comment",
        if video.separate_audio {
            "fthr-audio-mode=separated"
        } else {
            "fthr-audio-mode=combined"
        },
    )?;
    mux.set_metadata("fthr_frame_rate", &format!("{fps}/1"))?;
    mux.set_metadata(
        "fthr_video_bitrate_bps",
        &(i64::from(video.bitrate_kbps) * 1000).to_string(),
    )?;
    let video_index = mux.add_video_stream(
        video.codec_id.into(),
        video.width,
        video.height,
        fps,
        fps as i32,
        i64::from(video.bitrate_kbps) * 1000,
        video.extradata,
    )?;

    // Every track's encoder and stream are set up before the header; a
    // track that fails is skipped and named, the others are still written.
    let mut encoders: Vec<(AacEncoder, usize, &ClipAudio<'_>)> = Vec::new();
    let mut errors = Vec::new();
    for track in tracks.iter().filter(|t| !t.samples.is_empty()) {
        let setup = AacEncoder::new(track.sample_rate, track.channels, mux.wants_global_header())
            .and_then(|enc| {
                mux.add_audio_stream(&enc, track.title)
                    .map(|index| (enc, index))
            });
        match setup {
            Ok((enc, index)) => encoders.push((enc, index, track)),
            Err(e) => {
                tracing::warn!(
                    track = track.title,
                    "audio track setup failed; skipping it: {e}"
                );
                errors.push(format!("{}: {e}", track.title));
            }
        }
    }
    if !errors.is_empty() {
        outcome.audio_error = Some(errors.join("; "));
    }

    mux.write_header()?;

    let mut bytes_out = 0u64;
    for p in &snapshot.packets {
        if p.data.is_empty() {
            continue;
        }
        mux.write_packet(
            video_index,
            &p.data,
            p.pts - snapshot.presentation_start_pts,
            p.dts - snapshot.presentation_start_pts,
            1,
            p.keyframe,
            (1, fps as i32),
        )
        .map_err(|e| {
            crate::MediaError::Invalid(format!(
                "Failed while writing video packets to the temporary clip: {e}"
            ))
        })?;
        bytes_out += p.data.len() as u64;
        if let Some(cb) = progress {
            cb(bytes_out);
        }
    }
    outcome.bytes_written = bytes_out;

    for (mut enc, index, track) in encoders {
        let rate = enc.sample_rate() as i32;
        let mut write_err: Option<crate::MediaError> = None;
        let mut sink = |pkt: crate::ffi::codec::RawPacket| {
            if write_err.is_none()
                && let Err(e) =
                    mux.write_packet(index, &pkt.data, pkt.pts, pkt.dts, 0, true, (1, rate))
            {
                write_err = Some(e);
            }
        };
        enc.encode(track.samples, &mut sink).map_err(|e| {
            crate::MediaError::Invalid(format!(
                "Failed while encoding audio for the temporary clip: {e}"
            ))
        })?;
        enc.flush(&mut sink).map_err(|e| {
            crate::MediaError::Invalid(format!(
                "Failed while finalizing audio for the temporary clip: {e}"
            ))
        })?;
        if let Some(e) = write_err {
            return Err(crate::MediaError::Invalid(format!(
                "Failed while writing audio packets to the temporary clip: {e}"
            )));
        }
        outcome.audio_included = true;
        outcome.audio_tracks.push(track.title.to_string());
    }

    mux.finish()?;
    Ok(())
}
