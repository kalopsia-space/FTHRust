//! Capture-setting limits, the settings-to-engine configuration mapping, the
//! Linux encoded-ring budget and the requested/active configuration tracker.
//!
//! Ports `reference/FTHR_UI/core/capture_settings.py` plus the resolution and
//! bitrate tables from `main.py`. Invalid values are errors, never clamped.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::settings::SettingsStore;

/// Clip lengths offered by the UI, in seconds.
pub const CLIP_LENGTHS: [u32; 11] = [5, 10, 15, 30, 45, 60, 90, 120, 180, 240, 300];
/// Longest clip (and replay buffer) the engine supports, in seconds.
pub const MAX_CLIP_SECONDS: u32 = 900;
/// Frame rates offered by the UI.
pub const FPS_VALUES: [u32; 8] = [30, 60, 90, 120, 144, 165, 180, 240];
/// Smallest encoded-ring budget the Linux engine accepts, in MiB.
pub const RING_BUDGET_MIN_MB: u64 = 64;
/// Largest encoded-ring budget the Linux engine accepts, in MiB.
pub const RING_BUDGET_MAX_MB: u64 = 16384;

fn validate_range(value: i64, name: &str, min: i64, max: i64) -> Result<u32> {
    if !(min..=max).contains(&value) {
        return Err(CoreError::Invalid(format!(
            "{name} must be between {min} and {max}"
        )));
    }
    Ok(value as u32)
}

/// Validates a clip length in seconds (5..=900).
pub fn validate_clip_length(value: i64) -> Result<u32> {
    validate_range(value, "normal clip length", 5, MAX_CLIP_SECONDS as i64)
}

/// Validates a capture frame rate (15..=240).
pub fn validate_fps(value: i64) -> Result<u32> {
    validate_range(value, "frame rate", 15, 240)
}

/// Validates a custom bitrate in kbps (500..=200000).
pub fn validate_custom_bitrate_kbps(value: i64) -> Result<u32> {
    validate_range(value, "custom bitrate", 500, 200_000)
}

/// Validates an encoder preset index (1..=7).
pub fn validate_encoder_preset(value: i64) -> Result<u8> {
    validate_range(value, "encoder preset", 1, 7).map(|v| v as u8)
}

/// Returns the valid custom clip lengths from a settings value, de-duplicated
/// and sorted; anything that is not a list of valid integers is ignored.
pub fn custom_clip_lengths(values: &Value) -> Vec<u32> {
    let Some(items) = values.as_array() else {
        return Vec::new();
    };
    let mut out = BTreeSet::new();
    for item in items {
        if let Some(n) = item.as_i64()
            && let Ok(valid) = validate_clip_length(n)
        {
            out.insert(valid);
        }
    }
    out.into_iter().collect()
}

/// Replay buffer length: the longest configured clip plus a two-second
/// safety margin, capped at [`MAX_CLIP_SECONDS`].
pub fn compute_buffer_seconds(normal_seconds: u32, custom_lengths: &[u32]) -> Result<u32> {
    let normal = validate_clip_length(normal_seconds as i64)?;
    let longest = custom_lengths
        .iter()
        .filter(|&&v| validate_clip_length(v as i64).is_ok())
        .fold(normal, |acc, &v| acc.max(v));
    Ok(MAX_CLIP_SECONDS.min(longest + 2))
}

/// Total physical memory in MiB from `/proc/meminfo`, or 0 when unknown.
fn total_memory_mb() -> u64 {
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
        return 0;
    };
    text.lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse::<u64>().ok())
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}

/// How much capture time the encoded ring keeps, in milliseconds: the clip
/// plus one GOP (`2 × fps` frames), so a clip can start on a keyframe, plus
/// half a second of margin.
pub fn ring_window_ms(buffer_seconds: u32) -> u64 {
    (u64::from(buffer_seconds) + 2) * 1000 + 500
}

/// Byte budget in MiB for the Linux engine's encoded replay ring.
///
/// Sized for the configured bitrate over `buffer_seconds + 5` with 25 %
/// headroom plus 64 MiB, never more than half of physical memory, clamped to
/// [`RING_BUDGET_MIN_MB`]..=[`RING_BUDGET_MAX_MB`]. `total_memory_mb` of
/// `None` reads the machine's memory.
pub fn linux_ring_budget_mb(
    bitrate_kbps: u32,
    buffer_seconds: u32,
    total_memory_mb: Option<u64>,
) -> u64 {
    let bitrate = u64::from(bitrate_kbps.max(1));
    let seconds = u64::from(buffer_seconds.max(1)) + 5;
    let expected_mb = (bitrate * 1000 / 8 * seconds) as f64 / (1024.0 * 1024.0);
    let mut budget = (expected_mb * 1.25) as u64 + 64;
    let total = total_memory_mb.unwrap_or_else(self::total_memory_mb);
    if total > 0 {
        budget = budget.min(total / 2);
    }
    budget.clamp(RING_BUDGET_MIN_MB, RING_BUDGET_MAX_MB)
}

/// Pixel dimensions for a resolution setting; `(0, 0)` means source size.
pub fn resolution_dims(name: &str) -> Option<(u32, u32)> {
    match name {
        "480p" => Some((854, 480)),
        "720p" => Some((1280, 720)),
        "1080p" => Some((1920, 1080)),
        "1440p" => Some((2560, 1440)),
        "source" => Some((0, 0)),
        _ => None,
    }
}

/// The `bitrate_level` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitrateLevel {
    /// Lowest preset for the resolution.
    Low,
    /// Default preset for the resolution.
    Medium,
    /// Highest preset for the resolution.
    High,
    /// The user-entered `custom_bitrate_kbps`.
    Custom,
}

impl BitrateLevel {
    /// Parses the settings string; unknown values fail closed to `Medium`.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Self::Low,
            "high" => Self::High,
            "custom" => Self::Custom,
            _ => Self::Medium,
        }
    }
}

/// AV1 presets as a percentage of the H.264 preset, per level (low, medium,
/// high): the lowest AV1 VA-API bitrate whose VMAF matched H.264 VA-API on
/// three 1080p120 game clips (`tools/bitrate-match`).
pub const AV1_PRESET_PERCENT: [u32; 3] = [55, 70, 90];

/// `kbps` (an H.264 preset at `level`) scaled to the matched AV1 bitrate.
pub fn av1_preset_kbps(kbps: u32, level: BitrateLevel) -> u32 {
    let percent = match level {
        BitrateLevel::Low => AV1_PRESET_PERCENT[0],
        BitrateLevel::Medium => AV1_PRESET_PERCENT[1],
        BitrateLevel::High => AV1_PRESET_PERCENT[2],
        BitrateLevel::Custom => 100,
    };
    kbps * percent / 100
}

/// The preset for `codec`: H.264's for every codec but AV1, which uses the
/// VMAF-matched [`AV1_PRESET_PERCENT`] of it.
pub fn codec_preset_kbps(resolution: &str, level: BitrateLevel, codec: Codec) -> Option<u32> {
    let kbps = bitrate_preset_kbps(resolution, level)?;
    Some(if codec == Codec::Av1 {
        av1_preset_kbps(kbps, level)
    } else {
        kbps
    })
}

/// Preset bitrate in kbps for a resolution and level; `None` for an unknown
/// resolution or the custom level.
pub fn bitrate_preset_kbps(resolution: &str, level: BitrateLevel) -> Option<u32> {
    let (low, medium, high) = match resolution {
        "480p" => (2500, 5000, 10000),
        "720p" => (5000, 12000, 20000),
        "1080p" => (10000, 25000, 50000),
        "1440p" => (15000, 35000, 60000),
        "source" => (10000, 25000, 50000),
        _ => return None,
    };
    match level {
        BitrateLevel::Low => Some(low),
        BitrateLevel::Medium => Some(medium),
        BitrateLevel::High => Some(high),
        BitrateLevel::Custom => None,
    }
}

/// Video codec preference.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Codec {
    /// Let the engine choose.
    #[default]
    Auto,
    /// H.264 / AVC.
    H264,
    /// H.265 / HEVC.
    Hevc,
    /// AV1.
    Av1,
}

impl Codec {
    /// Parses the `codec_pref` setting; unknown values fail closed to `Auto`.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "h264" => Self::H264,
            "hevc" => Self::Hevc,
            "av1" => Self::Av1,
            _ => Self::Auto,
        }
    }

    /// The settings / engine string for this codec.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
        }
    }
}

/// Where the engine keeps the replay buffer's video.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BufferLocation {
    /// In RAM.
    #[default]
    Memory,
    /// In segment files in the cache directory; only metadata in RAM.
    Disk,
}

impl BufferLocation {
    /// Parses the `replay_buffer_location` setting; unknown values fail
    /// closed to `Memory`.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "disk" => Self::Disk,
            _ => Self::Memory,
        }
    }

    /// The settings string for this location.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Disk => "disk",
        }
    }
}

/// How a scaled capture is fitted to the target size.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Scaling {
    /// Stretch to the target size.
    #[default]
    Stretch,
    /// Preserve aspect ratio.
    Fit,
}

impl Scaling {
    /// Parses the `scaling_mode` setting; unknown values fail closed to `Stretch`.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "fit" => Self::Fit,
            _ => Self::Stretch,
        }
    }

    /// The settings / engine string for this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stretch => "stretch",
            Self::Fit => "fit",
        }
    }
}

/// Encoder backend preference.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EncoderPref {
    /// Let the engine choose.
    #[default]
    Auto,
    /// NVIDIA NVENC.
    Nvenc,
    /// VA-API (stored under the legacy settings key `amf`).
    Vaapi,
    /// Intel Quick Sync.
    Qsv,
    /// Software encoding.
    Software,
}

impl EncoderPref {
    /// Parses the `encoder_pref` setting; `amf` means VA-API on Linux and
    /// unknown values fail closed to `Auto`.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "nvenc" => Self::Nvenc,
            "amf" | "vaapi" => Self::Vaapi,
            "qsv" => Self::Qsv,
            "software" => Self::Software,
            _ => Self::Auto,
        }
    }

    /// The settings string for this preference (`amf` for VA-API, for
    /// compatibility with the reference's files).
    pub fn as_settings_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Nvenc => "nvenc",
            Self::Vaapi => "amf",
            Self::Qsv => "qsv",
            Self::Software => "software",
        }
    }
}

/// Normalised crop rectangle (fractions of the source frame).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct CropRect {
    /// Left edge, 0..1.
    pub x: f64,
    /// Top edge, 0..1.
    pub y: f64,
    /// Width, 0..1.
    pub w: f64,
    /// Height, 0..1.
    pub h: f64,
}

/// The configuration handed to the capture engine.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default)]
pub struct CaptureConfig {
    /// Capture frame rate.
    pub fps: u32,
    /// Replay buffer length in seconds.
    pub buffer_seconds: u32,
    /// Target width, 0 for source size.
    pub width: u32,
    /// Target height, 0 for source size.
    pub height: u32,
    /// Video bitrate in kbps.
    pub bitrate_kbps: u32,
    /// Codec preference.
    pub codec: Codec,
    /// Encoder preset index (1..=7).
    pub preset: u8,
    /// Capture output name, empty for automatic.
    pub monitor: String,
    /// Scaling mode.
    pub scaling: Scaling,
    /// Whether system audio is captured.
    pub audio_enabled: bool,
    /// Encoder backend preference.
    pub encoder: EncoderPref,
    /// Keep system and microphone audio as separate streams.
    pub separate_audio_enabled: bool,
    /// Microphone endpoint id, empty for the default.
    pub microphone_endpoint_id: String,
    /// Playback endpoint id, empty for the default.
    pub playback_endpoint_id: String,
    /// Downmix the microphone to mono.
    pub microphone_mono: bool,
    /// The user's normal clip length in seconds.
    pub normal_clip_seconds: u32,
    /// Crop rectangle when a quick crop is active.
    pub crop: Option<CropRect>,
    /// Where the replay buffer lives.
    #[serde(default)]
    pub buffer_location: BufferLocation,
}

impl CaptureConfig {
    /// Builds the engine configuration from the settings store, validating
    /// every numeric value the way the reference does.
    pub fn from_settings(s: &SettingsStore) -> Result<Self> {
        let fps = validate_fps(s.get_i64("framerate").unwrap_or(60))?;
        let normal = validate_clip_length(s.get_i64("clip_length").unwrap_or(30))?;
        let custom = s
            .get("custom_clip_lengths")
            .map(custom_clip_lengths)
            .unwrap_or_default();
        let buffer_seconds = compute_buffer_seconds(normal, &custom)?;
        let resolution = s.get_str("resolution").unwrap_or("source").to_string();
        let (width, height) = resolution_dims(&resolution).unwrap_or((0, 0));
        let level = BitrateLevel::parse(s.get_str("bitrate_level").unwrap_or("medium"));
        let codec = Codec::parse(s.get_str("codec_pref").unwrap_or("auto"));
        let bitrate_kbps = if level == BitrateLevel::Custom {
            validate_custom_bitrate_kbps(s.get_i64("custom_bitrate_kbps").unwrap_or(25_000))?
        } else {
            codec_preset_kbps(&resolution, level, codec).unwrap_or(25_000)
        };
        let preset = validate_encoder_preset(s.get_i64("encoder_preset").unwrap_or(4))?;
        let crop = s.get("quick_crop").and_then(crop_from_quick_crop);
        Ok(Self {
            fps,
            buffer_seconds,
            width,
            height,
            bitrate_kbps,
            codec,
            preset,
            monitor: s.get_str("capture_monitor").unwrap_or("").to_string(),
            scaling: Scaling::parse(s.get_str("scaling_mode").unwrap_or("stretch")),
            buffer_location: BufferLocation::parse(
                s.get_str("replay_buffer_location").unwrap_or("memory"),
            ),
            audio_enabled: s.get_bool("audio_capture_enabled").unwrap_or(true),
            encoder: EncoderPref::parse(s.get_str("encoder_pref").unwrap_or("auto")),
            separate_audio_enabled: normalize_audio_capture_mode(
                s.get_str("audio_capture_mode").unwrap_or("combined"),
            ) == "separated",
            microphone_endpoint_id: s.get_str("mic_device_id").unwrap_or("").to_string(),
            playback_endpoint_id: s.get_str("playback_device_id").unwrap_or("").to_string(),
            microphone_mono: s.get_bool("mic_downmix_mono").unwrap_or(false),
            normal_clip_seconds: normal,
            crop,
        })
    }
}

/// Normalises the `audio_capture_mode` setting to `combined` or `separated`;
/// unknown values fail closed to `combined`.
pub fn normalize_audio_capture_mode(value: &str) -> &'static str {
    if value.trim().eq_ignore_ascii_case("separated") {
        "separated"
    } else {
        "combined"
    }
}

fn crop_from_quick_crop(value: &Value) -> Option<CropRect> {
    let obj = value.as_object()?;
    let num = |k: &str| obj.get(k).and_then(Value::as_f64);
    let (x, y, w, h) = (num("x")?, num("y")?, num("w")?, num("h")?);
    let (src_w, src_h) = (num("src_w")?, num("src_h")?);
    if src_w <= 0.0 || src_h <= 0.0 || w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(CropRect {
        x: x / src_w,
        y: y / src_h,
        w: w / src_w,
        h: h / src_h,
    })
}

/// Where a requested configuration stands relative to the running engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyStatus {
    /// The active configuration is what the engine runs.
    Active,
    /// A new configuration has been requested but not yet applied.
    Requested,
    /// The engine is restarting with the requested configuration.
    Applying,
    /// The last apply failed; `active` is still the previous configuration.
    Failed,
}

/// Tracks the requested and active capture configurations.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureConfigTracker {
    active: Option<CaptureConfig>,
    requested: Option<CaptureConfig>,
    status: ApplyStatus,
    error: String,
}

impl CaptureConfigTracker {
    /// Starts with an optional active configuration (also the requested one).
    pub fn new(active: Option<CaptureConfig>) -> Self {
        Self {
            requested: active.clone(),
            active,
            status: ApplyStatus::Active,
            error: String::new(),
        }
    }

    /// Records a new requested configuration.
    pub fn request(&mut self, config: CaptureConfig) {
        self.requested = Some(config);
        self.status = ApplyStatus::Requested;
        self.error.clear();
    }

    /// Marks the requested configuration as being applied.
    pub fn begin_apply(&mut self) -> Result<()> {
        if self.requested.is_none() {
            return Err(CoreError::Invalid(
                "no capture configuration has been requested".into(),
            ));
        }
        self.status = ApplyStatus::Applying;
        self.error.clear();
        Ok(())
    }

    /// Promotes the requested configuration to active.
    pub fn succeed(&mut self) -> Result<()> {
        let Some(requested) = self.requested.clone() else {
            return Err(CoreError::Invalid(
                "no capture configuration has been requested".into(),
            ));
        };
        self.active = Some(requested);
        self.status = ApplyStatus::Active;
        self.error.clear();
        Ok(())
    }

    /// Records a failed apply; the previous active configuration stands.
    pub fn fail(&mut self, error: &str) {
        self.status = ApplyStatus::Failed;
        self.error = error.to_string();
    }

    /// Records that no running engine owns the previous active configuration.
    pub fn deactivate(&mut self, error: &str) {
        self.active = None;
        self.status = ApplyStatus::Failed;
        self.error = error.to_string();
    }

    /// The configuration the engine is known to run, if any.
    pub fn active(&self) -> Option<&CaptureConfig> {
        self.active.as_ref()
    }

    /// The most recently requested configuration, if any.
    pub fn requested(&self) -> Option<&CaptureConfig> {
        self.requested.as_ref()
    }

    /// The current apply status.
    pub fn status(&self) -> ApplyStatus {
        self.status
    }

    /// The last apply error, empty when none.
    pub fn error(&self) -> &str {
        &self.error
    }
}
