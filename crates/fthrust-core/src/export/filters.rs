//! Filter chains (`_video_filter_chain`, `_atempo_chain`,
//! `_pitch_changed_chain`, `ffmpeg_mix_filter`).

use crate::editor::snapshot::Effects;

/// Limiter ceiling of the export mix.
const LIMITER_CEILING: &str = "0.98";

/// `_normalise_speed`: 0.25..2, non-finite or non-positive → 1.
pub fn normalise_speed(speed: f64) -> f64 {
    let s = if speed.is_finite() && speed > 0.0 {
        speed
    } else {
        1.0
    };
    s.clamp(0.25, 2.0)
}

/// `_speed_text`: six decimals with trailing zeros and the dot stripped.
pub fn speed_text(speed: f64) -> String {
    let s = format!("{:.6}", normalise_speed(speed));
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Pitch-preserving tempo stages, each within 0.5..2.
pub fn atempo_chain(speed: f64) -> Option<String> {
    let mut remaining = normalise_speed(speed);
    let mut out: Vec<String> = Vec::new();
    while remaining > 2.0 + 1e-6 {
        out.push("atempo=2".into());
        remaining /= 2.0;
    }
    while remaining < 0.5 - 1e-6 {
        out.push("atempo=0.5".into());
        remaining /= 0.5;
    }
    if (remaining - 1.0).abs() > 1e-6 {
        out.push(format!("atempo={}", speed_text(remaining)));
    }
    (!out.is_empty()).then(|| out.join(","))
}

/// Pitch follows speed: resample at a shifted rate.
pub fn pitch_changed_chain(speed: f64) -> Option<String> {
    let s = normalise_speed(speed);
    if (s - 1.0).abs() <= 1e-6 {
        return None;
    }
    // Python's round() rounds half to even.
    let rate = (48_000.0 * s).round_ties_even() as i64;
    Some(format!("aresample=48000,asetrate={rate},aresample=48000"))
}

/// `_speed_audio_chain`.
pub fn speed_audio_chain(speed: f64, preserve_pitch: bool) -> Option<String> {
    if preserve_pitch {
        atempo_chain(speed)
    } else {
        pitch_changed_chain(speed)
    }
}

/// The export video chain, in the reference's order.
pub fn video_filter_chain(
    crop: Option<(u32, u32, u32, u32)>,
    effects: &Effects,
    stretch: f64,
    ensure_even: bool,
    speed: f64,
) -> Vec<String> {
    let mut f = Vec::new();
    if let Some((x, y, w, h)) = crop {
        f.push(format!("crop={}:{}:{x}:{y}", w & !1, h & !1));
    }
    let e = effects.clamped();
    if e.exposure != 0 || e.contrast != 0 {
        let brightness = f64::from(e.exposure) * 0.003;
        let contrast = 1.0 + f64::from(e.contrast) * 0.006;
        let points: Vec<String> = [0.0f64, 0.5, 1.0]
            .iter()
            .map(|s| {
                let v = ((s - 0.5) * contrast + 0.5 + brightness).clamp(0.0, 1.0);
                format!("{s:.3}/{v:.3}")
            })
            .collect();
        f.push(format!("curves=all={}", points.join(" ")));
    }
    if e.saturation != 0 {
        f.push(format!("hue=s={:.3}", 1.0 + f64::from(e.saturation) * 0.01));
    }
    if e.temperature != 0 {
        let b = f64::from(e.temperature) * 0.003;
        f.push(format!("colorbalance=rs={b:.3}:bs={:.3}", -b));
    }
    if e.sharpness != 0 {
        f.push(format!(
            "unsharp=5:5:{:.3}:5:5:0.0",
            f64::from(e.sharpness) * 0.015
        ));
    }
    if e.vignette != 0 {
        f.push(format!(
            "vignette=PI/{:.2}",
            20.0 - f64::from(e.vignette) * 0.15
        ));
    }
    let ratio = if stretch.is_finite() && stretch != 0.0 {
        stretch
    } else {
        1.0
    }
    .clamp(0.25, 4.0);
    if (ratio - 1.0).abs() > 0.005 {
        f.push(format!("scale=trunc(iw*{ratio:.4}/2)*2:trunc(ih/2)*2"));
        f.push("setsar=1".into());
    } else if ensure_even {
        f.push("scale=trunc(iw/2)*2:trunc(ih/2)*2".into());
    }
    let s = normalise_speed(speed);
    if (s - 1.0).abs() > 1e-6 {
        f.push(format!("setpts=PTS/{}", speed_text(s)));
    }
    f
}

/// How a source enters the mix (`mix_role`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixRole {
    /// The hidden recorded desktop mix: always unity.
    Base,
    /// An application stem applied as `gain - 1` against the base.
    Delta,
    /// Microphones, imported tracks: the slider gain.
    Direct,
}

/// One audio source of the clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixSource {
    /// Audio stream index (`0:a:N`); `None` when the container lacks it.
    pub audio_index: Option<u32>,
    /// The track decodes.
    pub available: bool,
    /// Mix role.
    pub role: MixRole,
    /// Slider gain 0–100.
    pub gain_percent: u8,
    /// Muted.
    pub muted: bool,
}

fn source_gain(s: &MixSource) -> f64 {
    if !s.available {
        return 0.0;
    }
    if s.role == MixRole::Base {
        return 1.0;
    }
    let gain = if s.muted {
        0.0
    } else {
        f64::from(s.gain_percent.min(100)) / 100.0
    };
    if s.role == MixRole::Delta {
        gain - 1.0
    } else {
        gain
    }
}

/// `ffmpeg_mix_filter`: per-source volume, amix, equal-power headroom, the
/// limiter and the master gain into `[label]`.
pub fn mix_filter(
    sources: &[MixSource],
    master_percent: u8,
    label: &str,
) -> (Vec<String>, Option<String>) {
    let selected: Vec<&MixSource> = sources
        .iter()
        .filter(|s| s.audio_index.is_some() && s.available)
        .collect();
    if selected.is_empty() {
        return (Vec::new(), None);
    }
    let mut f = Vec::new();
    let mut inputs = String::new();
    for (i, s) in selected.iter().enumerate() {
        f.push(format!(
            "[0:a:{}]volume={:.6}[src{i}]",
            s.audio_index.unwrap_or(0),
            source_gain(s)
        ));
        inputs.push_str(&format!("[src{i}]"));
    }
    let active = selected
        .iter()
        .filter(|s| source_gain(s).abs() > 1e-6)
        .count();
    let headroom = 1.0 / (active.max(1) as f64).sqrt();
    let master = f64::from(master_percent.min(100)) / 100.0;
    f.push(format!(
        "{inputs}amix=inputs={}:normalize=0,volume={headroom:.6},alimiter=limit={LIMITER_CEILING}:level=disabled,volume={master:.6}[{label}]",
        selected.len()
    ));
    (f, Some(format!("[{label}]")))
}
