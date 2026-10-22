//! Configuration normalisation: the positional-argument rules of the
//! reference engine (`engine_args.cpp`) applied to a typed config.

use crate::EngineConfig;

/// Default frame rate.
pub const DEFAULT_FPS: u32 = 60;
/// Maximum frame rate.
pub const MAX_FPS: u32 = 360;
/// Default replay length.
pub const DEFAULT_BUFFER_SECONDS: u32 = 30;
/// Maximum replay length.
pub const MAX_BUFFER_SECONDS: u32 = 900;
/// Default bitrate.
pub const DEFAULT_BITRATE_KBPS: u32 = 16000;
/// Bitrate range.
pub const BITRATE_RANGE_KBPS: (u32, u32) = (500, 200_000);
/// Default quality preset.
pub const DEFAULT_PRESET: u8 = 4;
/// Ring byte budget range (MiB) when non-zero.
pub const BUFFER_BUDGET_RANGE_MB: (u32, u32) = (64, 16384);
/// Largest explicit output size.
pub const MAX_SIZE: (u32, u32) = (7680, 4320);

fn clamp_field<T: Copy + PartialOrd + std::fmt::Display>(
    name: &str,
    value: T,
    default: T,
    zero: T,
    lo: T,
    hi: T,
    warnings: &mut Vec<String>,
) -> T {
    if value == zero {
        return default;
    }
    if value < lo || value > hi {
        let used = if value < lo { lo } else { hi };
        warnings.push(format!("{name} {value} is out of range; using {used}"));
        return used;
    }
    value
}

impl EngineConfig {
    /// Applies defaults for zero fields and clamps the rest, returning one
    /// warning line per clamped field (zero means "default" silently).
    pub fn normalized(&self) -> (EngineConfig, Vec<String>) {
        let mut warnings = Vec::new();
        let mut out = self.clone();
        let c = &mut out.capture;
        c.fps = clamp_field("fps", c.fps, DEFAULT_FPS, 0, 1, MAX_FPS, &mut warnings);
        c.buffer_seconds = clamp_field(
            "buffer_seconds",
            c.buffer_seconds,
            DEFAULT_BUFFER_SECONDS,
            0,
            1,
            MAX_BUFFER_SECONDS,
            &mut warnings,
        );
        c.bitrate_kbps = clamp_field(
            "bitrate_kbps",
            c.bitrate_kbps,
            DEFAULT_BITRATE_KBPS,
            0,
            BITRATE_RANGE_KBPS.0,
            BITRATE_RANGE_KBPS.1,
            &mut warnings,
        );
        c.preset = clamp_field("preset", c.preset, DEFAULT_PRESET, 0, 1, 7, &mut warnings);
        let (w, h) = (c.width, c.height);
        if (w == 0) != (h == 0) || w > MAX_SIZE.0 || h > MAX_SIZE.1 {
            warnings.push(format!("size {w}x{h} is out of range; using native"));
            c.width = 0;
            c.height = 0;
        }
        out.max_buffer_mb = clamp_field(
            "max_buffer_mb",
            self.max_buffer_mb,
            0,
            0,
            BUFFER_BUDGET_RANGE_MB.0,
            BUFFER_BUDGET_RANGE_MB.1,
            &mut warnings,
        );
        (out, warnings)
    }
}
