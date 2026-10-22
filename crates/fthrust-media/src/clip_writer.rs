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
    /// Why audio was dropped, when it was requested but failed to set up.
    pub audio_error: Option<String>,
}

/// Writes the clip to `final_path` transactionally. `progress` receives the
/// cumulative video bytes written.
pub fn write_clip(
    final_path: &Path,
    video: &ClipVideo<'_>,
    audio: Option<&ClipAudio<'_>>,
    progress: Option<&dyn Fn(u64)>,
) -> Result<ClipWriteResult, SaveError> {
    let mut outcome = ClipWriteResult {
        path: final_path.to_path_buf(),
        bytes_written: 0,
        audio_included: false,
        audio_error: None,
    };
    let result = transactional_save::run(final_path, |temporary| {
        write_temporary(temporary, video, audio, progress, &mut outcome).map_err(|e| e.to_string())
    });
    result.map(|path| ClipWriteResult { path, ..outcome })
}

fn write_temporary(
    path: &Path,
    video: &ClipVideo<'_>,
    audio: Option<&ClipAudio<'_>>,
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
        i64::from(video.bitrate_kbps) * 1000,
        video.extradata,
    )?;

    let mut aac: Option<(AacEncoder, usize)> = None;
    if let Some(a) = audio.filter(|a| !a.samples.is_empty()) {
        match AacEncoder::new(a.sample_rate, a.channels, mux.wants_global_header()) {
            Ok(enc) => match mux.add_audio_stream(&enc) {
                Ok(index) => aac = Some((enc, index)),
                Err(e) => {
                    tracing::warn!("audio stream setup failed; saving video only: {e}");
                    outcome.audio_error = Some(e.to_string());
                }
            },
            Err(e) => {
                tracing::warn!("AAC setup failed; saving video only: {e}");
                outcome.audio_error = Some(e.to_string());
            }
        }
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

    if let (Some((mut enc, index)), Some(a)) = (aac, audio) {
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
        enc.encode(a.samples, &mut sink).map_err(|e| {
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
    }

    mux.finish()?;
    Ok(())
}
