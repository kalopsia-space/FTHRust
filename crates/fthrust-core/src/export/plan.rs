//! Size-preset planning (`core/export_profiles.py` `plan_export`).

use crate::export_presets::ExportPreset;

const MIB: f64 = 1_048_576.0;
const LADDER: [u32; 8] = [2160, 1440, 1080, 900, 720, 540, 480, 360];

/// What the planner needs to know about the source.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaFacts {
    /// File size.
    pub size_bytes: u64,
    /// Duration.
    pub duration_s: f64,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Frame rate.
    pub fps: f64,
    /// Video codec name.
    pub video_codec: String,
    /// Has an audio stream.
    pub has_audio: bool,
    /// Audio codec name.
    pub audio_codec: String,
    /// Audio bitrate.
    pub audio_kbps: u32,
}

/// The plan for one preset.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportPlan {
    /// Planned duration.
    pub duration_s: f64,
    /// Byte budget.
    pub target_bytes: u64,
    /// Output width.
    pub target_width: u32,
    /// Output height.
    pub target_height: u32,
    /// Output frame rate.
    pub target_fps: f64,
    /// Video bitrate.
    pub video_kbps: u32,
    /// Audio bitrate.
    pub audio_kbps: u32,
    /// Re-encode video.
    pub reencode_video: bool,
    /// Re-encode audio.
    pub reencode_audio: bool,
    /// The change lines.
    pub changes: Vec<String>,
}

impl ExportPlan {
    /// `ExportPlan.summary`.
    pub fn summary(&self) -> String {
        if self.changes.is_empty() {
            "Original already fits; no re-encoding needed.".into()
        } else {
            self.changes.join(" · ")
        }
    }
}

fn fit_inside(sw: u32, sh: u32, bw: u32, bh: u32) -> (u32, u32) {
    let scale = 1.0f64
        .min(f64::from(bw) / f64::from(sw.max(1)))
        .min(f64::from(bh) / f64::from(sh.max(1)));
    let w = ((f64::from(sw) * scale) as u32 & !1).max(2);
    let h = ((f64::from(sh) * scale) as u32 & !1).max(2);
    (w, h)
}

fn minimum_video_kbps(w: u32, h: u32, fps: f64) -> u32 {
    ((f64::from(w) * f64::from(h) * fps.min(60.0) * 0.035 / 1000.0) as u32).max(180)
}

/// Plans FPS, bitrate and, only when needed, a lower resolution.
pub fn plan_export(src: &MediaFacts, preset: &ExportPreset, duration_s: Option<f64>) -> ExportPlan {
    let duration = duration_s.unwrap_or(src.duration_s).max(0.1);
    let target_bytes = (preset.target_size_mb * MIB * preset.safety_margin) as u64;
    let requested_fps = preset.target_fps.filter(|f| *f > 0.0).unwrap_or(src.fps);
    let target_fps = src.fps.min(requested_fps);
    let (mut tw, mut th) = fit_inside(
        src.width,
        src.height,
        preset.target_width.unwrap_or(src.width),
        preset.target_height.unwrap_or(src.height),
    );
    let audio_kbps = if src.has_audio {
        preset.audio_bitrate_kbps
    } else {
        0
    };
    let budget = ((target_bytes as f64 * 8.0 / duration / 1000.0) as i64).max(64);
    let video_kbps = (budget - i64::from(audio_kbps) - 12).max(120) as u32;
    if preset.target_width.is_none() && src.size_bytes > target_bytes {
        let aspect = if src.height > 0 {
            f64::from(src.width) / f64::from(src.height)
        } else {
            16.0 / 9.0
        };
        let (mut cw, mut ch) = (src.width, src.height);
        for h in LADDER {
            if h >= ch {
                continue;
            }
            if video_kbps >= minimum_video_kbps(cw, ch, target_fps) {
                break;
            }
            (cw, ch) = fit_inside(
                src.width,
                src.height,
                ((f64::from(h) * aspect) as u32).max(2),
                h,
            );
        }
        (tw, th) = (cw, ch);
    }
    let codec = src.video_codec.to_lowercase();
    let pcodec = preset.video_codec.to_lowercase();
    let codec_change = pcodec != "source" && pcodec != codec;
    let fps_change = target_fps < src.fps - 0.01;
    let size_change = src.size_bytes > target_bytes;
    let scale_change = (tw, th) != (src.width, src.height);
    let mut reencode_video = codec_change || fps_change || size_change || scale_change;
    if !size_change
        && !fps_change
        && !scale_change
        && (pcodec == "source" || pcodec == codec || pcodec == "h264")
        && (codec == "h264" || codec == "avc1")
    {
        reencode_video = false;
    }
    let reencode_audio = src.has_audio
        && reencode_video
        && (src.audio_codec.to_lowercase() != "aac" || src.audio_kbps > audio_kbps + 8);
    let mut changes = Vec::new();
    if fps_change {
        changes.push(format!("{:.0}→{:.0} FPS", src.fps, target_fps));
    }
    if scale_change {
        changes.push(format!("{}×{}→{tw}×{th}", src.width, src.height));
    }
    if size_change {
        changes.push(format!(
            "{:.1} MB→≤{:.1} MB",
            src.size_bytes as f64 / MIB,
            target_bytes as f64 / MIB
        ));
    }
    if reencode_video && !size_change {
        changes.push("H.264 video".into());
    }
    if reencode_audio {
        changes.push(format!("AAC {audio_kbps} kbps audio"));
    }
    ExportPlan {
        duration_s: duration,
        target_bytes,
        target_width: tw,
        target_height: th,
        target_fps,
        video_kbps,
        audio_kbps,
        reencode_video,
        reencode_audio,
        changes,
    }
}
