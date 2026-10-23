//! Encoder choice for export: H.264 through the capture encoder (its
//! NVENC / VA-API / software candidates), FFV1 for lossless, AAC or FLAC
//! for audio.

use fthrust_core::capture_config::{Codec, EncoderPref};

use super::Quality;
use crate::encoder::EncoderConfig;

/// Bitrate the reference uses for software H.264 (`software_video_args`):
/// 16000 kbps fast, 32000 kbps quality. Hardware gets the same target.
pub fn bitrate_kbps(quality: Quality, preset_kbps: Option<u32>) -> u32 {
    preset_kbps.unwrap_or(match quality {
        Quality::Fast => 16_000,
        _ => 32_000,
    })
}

/// The capture-encoder config for an export frame size.
pub fn h264_config(
    width: u32,
    height: u32,
    fps: u32,
    quality: Quality,
    preset_kbps: Option<u32>,
    software: bool,
) -> EncoderConfig {
    EncoderConfig {
        src_width: width,
        src_height: height,
        enc_width: width,
        enc_height: height,
        fps,
        bitrate_kbps: bitrate_kbps(quality, preset_kbps),
        codec: Codec::H264,
        encoder: if software {
            EncoderPref::Software
        } else {
            EncoderPref::Auto
        },
        preset: if quality == Quality::Fast { 1 } else { 5 },
    }
}

/// The DRM driver behind the first render node (`amdgpu`, `i915`, `xe`…).
fn render_driver() -> Option<String> {
    let link = std::fs::read_link("/sys/class/drm/renderD128/device/driver").ok()?;
    Some(link.file_name()?.to_string_lossy().into_owned())
}

/// The status line for the encoder that opened.
pub fn status_for_encoder(name: &str) -> &'static str {
    if name.contains("nvenc") {
        "Exporting · NVIDIA hardware"
    } else if name.contains("vaapi") || name.contains("qsv") {
        match render_driver().as_deref() {
            Some("i915" | "xe") => "Exporting · Intel hardware",
            _ => "Exporting · AMD hardware",
        }
    } else if name == "ffv1" {
        "Exporting · lossless"
    } else {
        "Exporting · software"
    }
}

/// Whether an encoder name is a hardware one.
pub fn is_hardware(name: &str) -> bool {
    ["nvenc", "vaapi", "qsv", "amf"]
        .iter()
        .any(|h| name.contains(h))
}
