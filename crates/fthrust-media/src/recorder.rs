//! Continuous recording to a recoverable fragmented MP4.
//!
//! Every keyframe closes the previous fragment and flushes it to disk, so a
//! crash loses at most the GOP in progress. Ports the Windows reference's
//! `continuous_recording_writer.cpp` (the Linux reference had no recorder).

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::ffi::aac::AacEncoder;
use crate::ffi::mux::Muxer;
use crate::ring::EncodedPacket;
use crate::transactional_save::{RealFs, SaveFs};
use crate::{MediaError, Result};

/// The video stream's MP4 time base (90 kHz, like the reference).
pub const VIDEO_TIME_BASE_DEN: i32 = 90_000;

/// One AAC audio track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecorderAudio {
    /// Sample rate.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u32,
    /// Stream title (`title` and `handler_name` tags), e.g. `Default Mix`.
    pub title: String,
}

/// What a recording is started with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecorderConfig {
    /// Codec of the incoming packets.
    pub codec_id: ffmpeg_next::codec::Id,
    /// Global header from the encoder (required).
    pub extradata: Vec<u8>,
    /// Frame width.
    pub width: u32,
    /// Frame height.
    pub height: u32,
    /// Frame rate; incoming packet pts are in `1/fps`.
    pub fps: u32,
    /// Configured bitrate, recorded as metadata.
    pub bitrate_kbps: u32,
    /// AAC audio tracks, in stream order.
    pub audio: Vec<RecorderAudio>,
}

#[derive(Debug)]
struct AudioTrack {
    encoder: AacEncoder,
    index: usize,
    started: bool,
    /// The encoder pts written as 0.
    first_pts: Option<i64>,
}

/// An open recording.
#[derive(Debug)]
pub struct Recorder {
    mux: Muxer,
    /// The file claimed for the recording, kept open to start writeback of
    /// each committed fragment and to fsync the result.
    file: File,
    path: PathBuf,
    video_index: usize,
    fps: u32,
    first_video_pts: Option<i64>,
    video_packets: u64,
    audio: Vec<AudioTrack>,
    failed: bool,
}

impl Recorder {
    /// Opens `path`, writes the recoverable header and returns a recorder
    /// that accepts packets. When anything after claiming the name fails,
    /// the file is removed again.
    pub fn start(path: &Path, cfg: &RecorderConfig) -> Result<Self> {
        crate::init()?;
        if cfg.extradata.is_empty() {
            return Err(MediaError::Invalid(
                "The active replay stream is not ready for recording (no decoder configuration)"
                    .into(),
            ));
        }
        if cfg.fps == 0 || cfg.width == 0 || cfg.height == 0 {
            return Err(MediaError::Invalid(
                "recording size and fps must be non-zero".into(),
            ));
        }
        // Claim the name first so a recording never truncates an existing
        // file (the muxer opens with O_TRUNC).
        let file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(MediaError::Invalid(format!(
                    "recording path already exists: {}",
                    path.display()
                )));
            }
            Err(e) => return Err(MediaError::Io(e)),
        };
        Self::open(path, file, cfg).inspect_err(|_| {
            let _ = std::fs::remove_file(path);
        })
    }

    fn open(path: &Path, file: File, cfg: &RecorderConfig) -> Result<Self> {
        let mut mux = Muxer::new(path, true)?;
        mux.set_metadata("fthr_frame_rate", &format!("{}/1", cfg.fps))?;
        mux.set_metadata(
            "fthr_video_bitrate_bps",
            &(i64::from(cfg.bitrate_kbps) * 1000).to_string(),
        )?;
        let video_index = mux.add_video_stream(
            cfg.codec_id.into(),
            cfg.width,
            cfg.height,
            cfg.fps,
            VIDEO_TIME_BASE_DEN,
            i64::from(cfg.bitrate_kbps) * 1000,
            &cfg.extradata,
        )?;
        let mut audio = Vec::new();
        for a in &cfg.audio {
            let encoder = AacEncoder::new(a.sample_rate, a.channels, mux.wants_global_header())?;
            let index = mux.add_audio_stream(&encoder, &a.title)?;
            audio.push(AudioTrack {
                encoder,
                index,
                started: false,
                first_pts: None,
            });
        }
        mux.write_header()?;
        tracing::info!(
            path = %path.display(),
            audio_tracks = audio.len(),
            "recoverable fragmented MP4 started"
        );
        Ok(Self {
            mux,
            file,
            path: path.to_path_buf(),
            video_index,
            fps: cfg.fps,
            first_video_pts: None,
            video_packets: 0,
            audio,
            failed: false,
        })
    }

    /// Writes one video packet. Packets before the first keyframe are
    /// dropped; each later keyframe first commits the previous fragment.
    pub fn write_video(&mut self, packet: &EncodedPacket) -> Result<()> {
        self.check_live()?;
        if packet.data.is_empty() {
            return Ok(());
        }
        let first = match self.first_video_pts {
            Some(first) => {
                if packet.keyframe && self.video_packets > 0 {
                    self.guard(Muxer::flush_fragment).map_err(|e| {
                        MediaError::Invalid(format!(
                            "Could not commit a completed recording fragment: {e}"
                        ))
                    })?;
                    // The fragment is complete: let it reach the disk now
                    // instead of with the next writeback sweep.
                    crate::payload::start_writeback(&self.file);
                }
                first
            }
            None => {
                if !packet.keyframe {
                    return Ok(());
                }
                // Decoding starts at 0 (a fragmented MP4 cannot start
                // earlier); with a reorder delay the keyframe presents
                // `pts - dts` ticks in.
                let first = packet.dts.min(packet.pts);
                self.first_video_pts = Some(first);
                first
            }
        };
        let pts = packet.pts - first;
        let dts = packet.dts - first;
        let fps = self.fps as i32;
        let index = self.video_index;
        let bytes = packet.data.bytes()?;
        self.guard(|m| m.write_packet(index, &bytes, pts, dts, 1, packet.keyframe, (1, fps)))
            .map_err(|e| {
                MediaError::Invalid(format!(
                    "The recording file could not be written; completed fragments already on disk remain playable: {e}"
                ))
            })?;
        self.video_packets += 1;
        Ok(())
    }

    /// Audio tracks configured.
    pub fn audio_tracks(&self) -> usize {
        self.audio.len()
    }

    /// Encodes and writes interleaved float32 samples to audio track
    /// `track`. Audio arriving before the first video keyframe is ignored
    /// so every track starts with the video.
    pub fn write_audio(&mut self, track: usize, samples: &[f32]) -> Result<()> {
        self.check_live()?;
        if self.first_video_pts.is_none() || samples.is_empty() {
            return Ok(());
        }
        let Some(AudioTrack {
            encoder,
            index,
            started,
            first_pts,
        }) = self.audio.get_mut(track)
        else {
            return Err(MediaError::Invalid(format!(
                "the recording has no audio track {track}"
            )));
        };
        *started = true;
        let (index, rate) = (*index, encoder.sample_rate() as i32);
        let mux = &mut self.mux;
        let mut write_err = None;
        let result = encoder.encode(samples, |p| {
            // Rebase to the first packet so the track starts at dts 0 (the
            // moov is already on disk, so a nonzero start cannot be edited).
            // That packet carries the encoder's priming.
            let first = *first_pts.get_or_insert(p.pts);
            let pts = p.pts - first;
            if write_err.is_none()
                && let Err(e) = mux.write_packet(index, &p.data, pts, pts, 0, true, (1, rate))
            {
                write_err = Some(e);
            }
        });
        if let Err(e) = result.and(write_err.map_or(Ok(()), Err)) {
            self.failed = true;
            return Err(MediaError::Invalid(format!(
                "The recording file could not be written; completed fragments already on disk remain playable: {e}"
            )));
        }
        Ok(())
    }

    /// Flushes the last fragment, drains the audio encoders, writes the
    /// trailer, flushes the file to disk and returns its size in bytes.
    pub fn finish(mut self) -> Result<u64> {
        self.check_live()?;
        for track in std::mem::take(&mut self.audio) {
            let AudioTrack {
                mut encoder,
                index,
                started,
                mut first_pts,
            } = track;
            if !started {
                continue;
            }
            let rate = encoder.sample_rate() as i32;
            let mux = &mut self.mux;
            let mut write_err = None;
            encoder.flush(|p| {
                let first = *first_pts.get_or_insert(p.pts);
                let pts = p.pts - first;
                if write_err.is_none()
                    && let Err(e) = mux.write_packet(index, &p.data, pts, pts, 0, true, (1, rate))
                {
                    write_err = Some(e);
                }
            })?;
            if let Some(e) = write_err {
                self.failed = true;
                return Err(e);
            }
        }
        if self.video_packets > 0 {
            self.guard(Muxer::flush_fragment).map_err(|e| {
                MediaError::Invalid(format!("Could not flush the final recording fragment: {e}"))
            })?;
        }
        self.mux.finish()?;
        let synced = self
            .file
            .sync_all()
            .and_then(|()| match self.path.parent() {
                Some(dir) if !dir.as_os_str().is_empty() => RealFs.sync_directory(dir),
                _ => Ok(()),
            });
        synced.map_err(|e| {
            MediaError::Invalid(format!(
                "The recording was written but could not be flushed to disk: {e}"
            ))
        })?;
        Ok(std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0))
    }

    /// The file being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Video packets written so far.
    pub fn video_packets(&self) -> u64 {
        self.video_packets
    }

    fn check_live(&self) -> Result<()> {
        if self.failed {
            Err(MediaError::Invalid(
                "the recording already failed; completed fragments remain playable".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn guard(&mut self, f: impl FnOnce(&mut Muxer) -> Result<()>) -> Result<()> {
        let r = f(&mut self.mux);
        if r.is_err() {
            self.failed = true;
        }
        r
    }
}

impl Drop for Recorder {
    /// Commits the fragment in progress so an abandoned recorder still
    /// leaves every written GOP playable (no trailer).
    fn drop(&mut self) {
        if !self.failed && self.video_packets > 0 {
            let _ = self.mux.flush_fragment();
        }
    }
}
