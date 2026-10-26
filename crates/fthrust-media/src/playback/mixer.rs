//! The in-process mixer: one demuxer and decoder per track, resampled to
//! 48 kHz stereo f32, scaled by per-track gains and the master gain, optionally rate
//! changed with pitch preservation through a chain of `atempo` filters.

use std::collections::VecDeque;
use std::path::Path;

use ffmpeg_next::format::Sample;
use ffmpeg_next::format::sample::Type as SampleType;
use ffmpeg_next::media::Type;
use ffmpeg_next::{codec, filter, format, frame};

use super::{CANONICAL_CHANNELS, CANONICAL_SAMPLE_RATE, FRAMES_PER_BLOCK, SourceSpec};
use crate::audio_decode::frame_to_stereo;
use crate::{MediaError, Result};

/// The slowest supported rate.
pub const MIN_RATE: f32 = 0.25;
/// The fastest supported rate.
pub const MAX_RATE: f32 = 4.0;

/// Stateful linear resampler (native rate → canonical), continuous across
/// chunks.
struct Resampler {
    ratio: f64,
    phase: f64,
    last: [f32; 2],
    primed: bool,
}

impl Resampler {
    fn new(native_rate: u32) -> Self {
        Resampler {
            ratio: f64::from(native_rate) / f64::from(CANONICAL_SAMPLE_RATE),
            phase: 0.0,
            last: [0.0; 2],
            primed: false,
        }
    }

    fn passthrough(&self) -> bool {
        (self.ratio - 1.0).abs() < 1e-9
    }

    /// Converts interleaved stereo `input` at the native rate, appending to `out`.
    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.passthrough() {
            out.extend_from_slice(input);
            return;
        }
        let frames = input.len() / 2;
        if frames == 0 {
            return;
        }
        // Virtual input: last sample from the previous chunk then this chunk.
        let get = |i: isize, ch: usize| -> f32 {
            if i < 0 {
                self.last[ch]
            } else {
                input[i as usize * 2 + ch]
            }
        };
        let start_offset = if self.primed { -1.0 } else { 0.0 };
        let mut pos = self.phase + start_offset;
        let end = frames as f64 - 1.0;
        while pos <= end {
            let i = pos.floor();
            let frac = (pos - i) as f32;
            let i0 = i as isize;
            let i1 = (i + 1.0).min(end) as isize;
            for ch in 0..2 {
                let a = get(i0, ch);
                let b = get(i1, ch);
                out.push(a + (b - a) * frac);
            }
            pos += self.ratio;
        }
        // Carry the position relative to the chunk's last frame (which
        // becomes virtual index -1 of the next chunk).
        self.phase = pos - end;
        self.last = [input[(frames - 1) * 2], input[(frames - 1) * 2 + 1]];
        self.primed = true;
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.last = [0.0; 2];
        self.primed = false;
    }
}

struct Track {
    /// This track's own demuxer, reading only its stream: tracks never wait
    /// on each other or on the video, however the file is interleaved.
    ictx: format::context::Input,
    index: usize,
    decoder: codec::decoder::Audio,
    time_base: f64,
    resampler: Option<Resampler>,
    /// Decoded canonical frames not yet mixed.
    pending: VecDeque<f32>,
    /// Frames to drop after a seek (the decoded audio before the target).
    skip_frames: usize,
    eof: bool,
}

struct Tempo {
    graph: filter::Graph,
    pending: VecDeque<f32>,
    pts: i64,
}

/// The mixer.
pub struct Mixer {
    tracks: Vec<Track>,
    failed: Vec<usize>,
    /// Canonical frames consumed since `seek_base_ms`.
    consumed_frames: u64,
    seek_base_ms: u64,
    rate: f32,
    preserve_pitch: bool,
    tempo: Option<Tempo>,
    /// Number of entries in the `open` source list (gains index space).
    source_slots: usize,
}

impl Mixer {
    /// Opens `path` with the given tracks; tracks that cannot be opened are
    /// reported by [`Mixer::failed_sources`] and mixed as silence.
    pub fn open(path: &Path, sources: &[SourceSpec]) -> Result<Mixer> {
        crate::init()?;
        let mut tracks = Vec::new();
        let mut failed = Vec::new();
        for (i, spec) in sources.iter().enumerate() {
            let mut ictx = format::input(path)?;
            let opened = ictx.stream(spec.container_index).and_then(|stream| {
                if stream.parameters().medium() != Type::Audio {
                    return None;
                }
                let ctx = codec::context::Context::from_parameters(stream.parameters()).ok()?;
                let decoder = ctx.decoder().audio().ok()?;
                let tb = stream.time_base();
                Some((
                    decoder,
                    f64::from(tb.numerator()) / f64::from(tb.denominator()),
                ))
            });
            let opened = opened.map(|(decoder, time_base)| {
                crate::ffi::probe::keep_only_stream(&mut ictx, spec.container_index);
                Track {
                    ictx,
                    index: spec.container_index,
                    decoder,
                    time_base,
                    resampler: None,
                    pending: VecDeque::new(),
                    skip_frames: 0,
                    eof: false,
                }
            });
            match opened {
                Some(t) => tracks.push(t),
                None => failed.push(i),
            }
        }
        if tracks.is_empty() {
            return Err(MediaError::Invalid(
                "This clip has no decodable audio stream.".into(),
            ));
        }
        // Keep the gains array aligned with `sources`: failed slots stay as
        // placeholders by index.
        let mixer = Mixer {
            tracks,
            failed,
            consumed_frames: 0,
            seek_base_ms: 0,
            rate: 1.0,
            preserve_pitch: true,
            tempo: None,
            source_slots: sources.len(),
        };
        Ok(mixer)
    }

    /// Sources (by index into the `open` list) that could not be opened.
    pub fn failed_sources(&self) -> Vec<usize> {
        self.failed.clone()
    }

    /// Tracks that opened.
    pub fn source_count(&self) -> usize {
        self.tracks.len()
    }

    /// Source time of the next frame to be returned.
    pub fn position_ms(&self) -> u64 {
        self.seek_base_ms + self.consumed_frames * 1000 / u64::from(CANONICAL_SAMPLE_RATE)
    }

    /// Configures the rate; `Ok(true)` when the `atempo` chain is active.
    pub fn set_rate(&mut self, rate: f32, preserve_pitch: bool) -> Result<bool> {
        let rate = if rate.is_finite() {
            rate.clamp(MIN_RATE, MAX_RATE)
        } else {
            1.0
        };
        self.rate = rate;
        self.preserve_pitch = preserve_pitch;
        self.tempo = None;
        if preserve_pitch && (rate - 1.0).abs() > 1e-6 {
            self.tempo = Some(build_tempo(rate)?);
            return Ok(true);
        }
        Ok(false)
    }

    /// Moves to `position_ms`.
    pub fn seek(&mut self, position_ms: u64) -> Result<()> {
        let ts = (position_ms as i64) * i64::from(ffmpeg_next::ffi::AV_TIME_BASE) / 1000;
        for t in &mut self.tracks {
            if let Err(e) = t.ictx.seek(ts, ..ts) {
                tracing::debug!("audio seek failed, decoding from the start: {e}");
                let _ = t.ictx.seek(0, ..0);
            }
            t.decoder.flush();
            t.pending.clear();
            t.eof = false;
            t.skip_frames = 0;
            if let Some(r) = t.resampler.as_mut() {
                r.reset();
            }
        }
        self.seek_base_ms = position_ms;
        self.consumed_frames = 0;
        if let Some(t) = &self.tempo {
            let _ = t;
            self.tempo = Some(build_tempo(self.rate)?);
        }
        // Decode up to the target so the first frame returned is at `position_ms`.
        self.prime_to(position_ms)
    }

    fn prime_to(&mut self, position_ms: u64) -> Result<()> {
        // Each track learns its first decoded pts after the seek and drops
        // the frames before the target.
        let target_s = position_ms as f64 / 1000.0;
        for t in &mut self.tracks {
            while !t.eof && t.pending.is_empty() {
                decode_some(t, Some(target_s))?;
            }
        }
        Ok(())
    }

    /// The largest decoded-but-unmixed buffer across tracks, in frames
    /// (for tests).
    pub fn pending_frames(&self) -> usize {
        self.tracks
            .iter()
            .map(|t| t.pending.len() / CANONICAL_CHANNELS)
            .max()
            .unwrap_or(0)
    }

    /// Produces `frames` mixed canonical frames at the source rate (no tempo).
    fn mix_block(&mut self, frames: usize, gains: &[f32], master: f32) -> Result<Option<Vec<f32>>> {
        // Fill every live track up to `frames`.
        for t in &mut self.tracks {
            while !t.eof && t.pending.len() < frames * CANONICAL_CHANNELS {
                decode_some(t, None)?;
            }
        }
        let available = self
            .tracks
            .iter()
            .filter(|t| !t.pending.is_empty())
            .map(|t| t.pending.len() / CANONICAL_CHANNELS)
            .max()
            .unwrap_or(0);
        if available == 0 {
            return Ok(None);
        }
        let n = available.min(frames);
        let mut out = vec![0.0f32; n * CANONICAL_CHANNELS];
        // Gains are indexed by the original source list; map through the
        // live tracks in order, skipping failed slots.
        let mut live = 0usize;
        for (slot, _) in (0..self.source_slots).enumerate() {
            if self.failed.contains(&slot) {
                continue;
            }
            let gain = gains.get(slot).copied().unwrap_or(1.0) * master;
            let t = &mut self.tracks[live];
            live += 1;
            let take = (n * CANONICAL_CHANNELS).min(t.pending.len());
            for (i, s) in t.pending.drain(..take).enumerate() {
                out[i] += s * gain;
            }
        }
        Ok(Some(out))
    }

    /// Returns `frames` output frames mixed with `gains` (per `open` slot)
    /// and `master`; `Ok(None)` at the end of every track.
    pub fn pull(&mut self, frames: usize, gains: &[f32], master: f32) -> Result<Option<Vec<f32>>> {
        if self.tempo.is_none() {
            let block = self.mix_block(frames, gains, master)?;
            if let Some(b) = &block {
                self.consumed_frames += (b.len() / CANONICAL_CHANNELS) as u64;
            }
            return Ok(block);
        }
        let mut out: Vec<f32> = Vec::with_capacity(frames * CANONICAL_CHANNELS);
        let mut source_eof = false;
        while out.len() < frames * CANONICAL_CHANNELS {
            let tempo = self.tempo.as_mut().expect("tempo");
            if !tempo.pending.is_empty() {
                let take = (frames * CANONICAL_CHANNELS - out.len()).min(tempo.pending.len());
                out.extend(tempo.pending.drain(..take));
                continue;
            }
            if source_eof {
                break;
            }
            let block = self.mix_block(FRAMES_PER_BLOCK, gains, master)?;
            let tempo = self.tempo.as_mut().expect("tempo");
            match block {
                Some(b) => {
                    push_tempo(tempo, &b)?;
                }
                None => {
                    source_eof = true;
                    flush_tempo(tempo)?;
                }
            }
        }
        if out.is_empty() {
            return Ok(None);
        }
        // The position follows what reached the output: output frames times
        // the rate (the reference's `source_frames`), not what the filter
        // chain has buffered.
        let out_frames = (out.len() / CANONICAL_CHANNELS) as f64;
        self.consumed_frames += (out_frames * f64::from(self.rate)).round() as u64;
        Ok(Some(out))
    }
}

/// Demuxes one of `t`'s packets and decodes it into its pending buffer.
/// With `trim_before`, decoded audio before that time is dropped.
fn decode_some(t: &mut Track, trim_before: Option<f64>) -> Result<()> {
    let next = t.ictx.packets().next().map(|(s, p)| (s.index(), p));
    let Some((index, packet)) = next else {
        let _ = t.decoder.send_eof();
        drain(t, trim_before)?;
        t.eof = true;
        return Ok(());
    };
    if index != t.index || t.decoder.send_packet(&packet).is_err() {
        return Ok(());
    }
    drain(t, trim_before)
}

fn drain(t: &mut Track, trim_before: Option<f64>) -> Result<()> {
    let mut decoded = frame::Audio::empty();
    while t.decoder.receive_frame(&mut decoded).is_ok() {
        if t.resampler.is_none() {
            t.resampler = Some(Resampler::new(decoded.rate()));
        }
        let native = frame_to_stereo(&decoded)?;
        let mut canonical = Vec::with_capacity(native.len());
        t.resampler
            .as_mut()
            .expect("resampler")
            .push(&native, &mut canonical);
        if let Some(target) = trim_before {
            let pts_s = decoded
                .pts()
                .or(decoded.timestamp())
                .map_or(0.0, |p| p as f64 * t.time_base);
            if pts_s < target {
                let drop = ((target - pts_s) * f64::from(CANONICAL_SAMPLE_RATE)).round() as usize
                    * CANONICAL_CHANNELS;
                if drop >= canonical.len() {
                    continue;
                }
                canonical.drain(..drop);
            }
        }
        t.pending.extend(canonical);
    }
    Ok(())
}

fn build_tempo(rate: f32) -> Result<Tempo> {
    let mut graph = filter::Graph::new();
    let abuffer = filter::find("abuffer")
        .ok_or_else(|| MediaError::Invalid("abuffer filter missing".into()))?;
    let abuffersink = filter::find("abuffersink")
        .ok_or_else(|| MediaError::Invalid("abuffersink filter missing".into()))?;
    if filter::find("atempo").is_none() {
        return Err(MediaError::Invalid(
            "FFmpeg build does not provide the atempo audio filter".into(),
        ));
    }
    let args = format!(
        "time_base=1/{r}:sample_rate={r}:sample_fmt=flt:channel_layout=stereo",
        r = CANONICAL_SAMPLE_RATE
    );
    graph.add(&abuffer, "in", &args)?;
    graph.add(&abuffersink, "out", "")?;
    // Chain stages so each stays within atempo's comfortable [0.5, 2.0].
    let stages = if rate > 1.0 {
        (rate.log2().ceil() as usize).max(1)
    } else {
        ((1.0 / rate).log2().ceil() as usize).max(1)
    };
    let per_stage = f64::from(rate).powf(1.0 / stages as f64);
    let spec = (0..stages)
        .map(|_| format!("atempo={per_stage:.6}"))
        .collect::<Vec<_>>()
        .join(",");
    graph.output("in", 0)?.input("out", 0)?.parse(&spec)?;
    graph.validate()?;
    Ok(Tempo {
        graph,
        pending: VecDeque::new(),
        pts: 0,
    })
}

fn push_tempo(tempo: &mut Tempo, block: &[f32]) -> Result<()> {
    let frames = block.len() / CANONICAL_CHANNELS;
    let mut f = frame::Audio::new(
        Sample::F32(SampleType::Packed),
        frames,
        ffmpeg_next::ChannelLayout::STEREO,
    );
    f.set_rate(CANONICAL_SAMPLE_RATE);
    f.set_pts(Some(tempo.pts));
    tempo.pts += frames as i64;
    f.plane_mut::<f32>(0)[..block.len()].copy_from_slice(block);
    tempo
        .graph
        .get("in")
        .ok_or_else(|| MediaError::Invalid("tempo graph lost its source".into()))?
        .source()
        .add(&f)?;
    drain_tempo(tempo)
}

fn flush_tempo(tempo: &mut Tempo) -> Result<()> {
    if let Some(mut src) = tempo.graph.get("in") {
        let _ = src.source().flush();
    }
    drain_tempo(tempo)
}

fn drain_tempo(tempo: &mut Tempo) -> Result<()> {
    let mut out = frame::Audio::empty();
    let mut sink = tempo
        .graph
        .get("out")
        .ok_or_else(|| MediaError::Invalid("tempo graph lost its sink".into()))?;
    while sink.sink().frame(&mut out).is_ok() {
        let samples = frame_to_stereo(&out)?;
        tempo.pending.extend(samples);
    }
    Ok(())
}
