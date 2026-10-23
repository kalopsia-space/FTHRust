//! Decodes a sound file (any format FFmpeg reads) to 48 kHz stereo f32
//! PCM for notification cues and previews. Sample-format conversion and
//! the rate change are done here (linear interpolation; cues are short),
//! which keeps the decoder path free of libswresample's frame checks.

use std::path::Path;

use ffmpeg_next::format::Sample;
use ffmpeg_next::format::sample::Type as SampleType;
use ffmpeg_next::media::Type;
use ffmpeg_next::{codec, format, frame};

use crate::{MediaError, Result};

/// Output sample rate.
pub const RATE: u32 = 48_000;
/// Output channels.
pub const CHANNELS: u16 = 2;
/// Longest cue kept (30 s).
pub const MAX_SECONDS: u32 = 30;

/// Interleaved stereo f32 samples at 48 kHz.
#[derive(Clone, Debug, PartialEq)]
pub struct Pcm {
    /// Always 48000.
    pub rate: u32,
    /// Always 2.
    pub channels: u16,
    /// Interleaved samples (`len` is a multiple of 2).
    pub samples: Vec<f32>,
}

/// Reads one decoded frame as stereo f32 at the source rate (mono is
/// duplicated, extra channels dropped).
fn frame_to_stereo(decoded: &frame::Audio) -> Result<Vec<f32>> {
    let channels = usize::from(decoded.channels().max(1));
    let n = decoded.samples();
    let planar = decoded.is_planar();
    let fetch = |plane: usize, i: usize| -> f32 {
        // `(plane, index)` in planar layout, `(0, index*channels + plane)` packed.
        let (p, idx) = if planar {
            (plane, i)
        } else {
            (0, i * channels + plane)
        };
        match decoded.format() {
            Sample::U8(_) => f32::from(decoded.plane::<u8>(p)[idx]) / 128.0 - 1.0,
            Sample::I16(_) => f32::from(decoded.plane::<i16>(p)[idx]) / 32768.0,
            Sample::I32(_) => decoded.plane::<i32>(p)[idx] as f32 / 2_147_483_648.0,
            Sample::I64(_) => 0.0,
            Sample::F32(_) => decoded.plane::<f32>(p)[idx],
            Sample::F64(_) => decoded.plane::<f64>(p)[idx] as f32,
            Sample::None => 0.0,
        }
    };
    if matches!(decoded.format(), Sample::None) {
        return Err(MediaError::Invalid("unknown sample format".into()));
    }
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let l = fetch(0, i);
        let r = if channels >= 2 { fetch(1, i) } else { l };
        out.push(l.clamp(-1.0, 1.0));
        out.push(r.clamp(-1.0, 1.0));
    }
    Ok(out)
}

/// Linear resampling of interleaved stereo from `from` to `to` Hz.
fn resample_stereo(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || input.len() < 4 {
        return input.to_vec();
    }
    let frames = input.len() / 2;
    let out_frames = ((frames as u64) * u64::from(to) / u64::from(from)) as usize;
    let step = f64::from(from) / f64::from(to);
    let mut out = Vec::with_capacity(out_frames * 2);
    for i in 0..out_frames {
        let pos = i as f64 * step;
        let i0 = (pos.floor() as usize).min(frames - 1);
        let i1 = (i0 + 1).min(frames - 1);
        let t = (pos - i0 as f64) as f32;
        for ch in 0..2 {
            let a = input[i0 * 2 + ch];
            let b = input[i1 * 2 + ch];
            out.push(a + (b - a) * t);
        }
    }
    out
}

/// Decodes `path` to PCM, capped at [`MAX_SECONDS`].
pub fn decode_to_pcm(path: &Path) -> Result<Pcm> {
    crate::init()?;
    let mut ictx = format::input(path)?;
    let stream = ictx
        .streams()
        .best(Type::Audio)
        .ok_or_else(|| MediaError::Invalid("no audio stream".into()))?;
    let index = stream.index();
    let ctx = codec::context::Context::from_parameters(stream.parameters())?;
    let mut decoder = ctx.decoder().audio()?;
    let max_frames = (RATE * MAX_SECONDS) as usize;
    let mut native: Vec<f32> = Vec::new();
    let mut rate = 0u32;
    let mut decoded = frame::Audio::empty();
    let enough = |native: &Vec<f32>, rate: u32| -> bool {
        rate > 0 && native.len() / 2 >= (rate as usize) * MAX_SECONDS as usize
    };
    for (s, packet) in ictx.packets() {
        if s.index() != index {
            continue;
        }
        decoder.send_packet(&packet)?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            if rate == 0 {
                rate = decoded.rate();
            }
            native.extend(frame_to_stereo(&decoded)?);
        }
        if enough(&native, rate) {
            break;
        }
    }
    if !enough(&native, rate) {
        decoder.send_eof()?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            if rate == 0 {
                rate = decoded.rate();
            }
            native.extend(frame_to_stereo(&decoded)?);
        }
    }
    if native.is_empty() || rate == 0 {
        return Err(MediaError::Invalid("no decodable audio".into()));
    }
    let mut samples = resample_stereo(&native, rate, RATE);
    samples.truncate(max_frames * usize::from(CHANNELS));
    let _ = SampleType::Packed;
    Ok(Pcm {
        rate: RATE,
        channels: CHANNELS,
        samples,
    })
}
