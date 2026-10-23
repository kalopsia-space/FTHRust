//! Editor export (`_export_clip`, cv:8008; `_build_export_cmd`, cv:8039):
//! the edit becomes an [`ExportSpec`], is planned and queued into
//! `{clips root}/Exported`. A failure to start shows `EXPORT FAILED` in the
//! error bar; the job's own failure only shows in the jobs window.

use std::path::Path;

use fthrust_core::export::filters::{MixRole, MixSource};
use fthrust_core::export::graph::{ExportSpec, plan};
use fthrust_core::export::naming::speed_adjusted_duration;
use fthrust_core::export::normalize_quality;
use fthrust_core::export::watermark::{WatermarkStyle, ffmpeg_color};
use fthrust_media::export::{Job, Quality};

use crate::icons::Icon;
use crate::services::export::ExportQueue;
use crate::shell::error_bar::Level;
use crate::state::AppState;

/// The editor's audio bitrate (`audio_bitrate='320k'`).
pub const AUDIO_KBPS: u32 = 320;

/// The empty-edit failure text.
pub const NO_KEPT: &str = "The edit has no kept segments.";

/// The `export_quality` setting as a runner quality.
pub fn quality(state: &AppState) -> Quality {
    Quality::from_setting(normalize_quality(
        state
            .settings
            .get_str("export_quality")
            .unwrap_or("quality"),
    ))
}

fn hex(c: egui::Color32) -> String {
    ffmpeg_color(&format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()))
}

/// The capture-card watermark in the theme's colours.
pub fn watermark_style(state: &AppState) -> WatermarkStyle {
    let cc = &state.tokens.capture_card;
    let assets = crate::assets_dir();
    WatermarkStyle {
        bg: hex(cc.bg),
        accent: hex(cc.accent),
        text: hex(cc.text),
        divider: hex(cc.divider),
        stats_dim: hex(cc.stats_dim),
        font_path: assets
            .join("fonts")
            .join("Oswald-Bold.ttf")
            .to_string_lossy()
            .into_owned(),
        icon_path: Icon::Favicon
            .bundled_path(&assets)
            .to_string_lossy()
            .into_owned(),
    }
}

/// The open edit as an export spec (kept ranges in seconds; segments
/// shorter than 1 ms dropped).
pub fn spec_for(state: &AppState) -> Result<ExportSpec, String> {
    let e = state.editor.as_ref().ok_or("No clip is open.")?;
    let duration_s = e.duration_ms() as f64 / 1000.0;
    let ranges_s: Vec<(f64, f64)> = e
        .timeline
        .kept_ranges()
        .into_iter()
        .map(|(a, b)| (a * duration_s, b * duration_s))
        .filter(|(a, b)| b - a >= 0.001)
        .collect();
    if ranges_s.is_empty() {
        return Err(NO_KEPT.into());
    }
    let rows = e.source_rows();
    let indexes = e.source_audio_indexes();
    let sources: Vec<MixSource> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| MixSource {
            audio_index: indexes.get(i).copied().flatten(),
            available: r.available,
            role: MixRole::Direct,
            gain_percent: r.gain,
            muted: r.muted,
        })
        .collect();
    let watermark = state
        .settings
        .get_bool("watermark_enabled")
        .unwrap_or(false)
        .then(|| watermark_style(state));
    Ok(ExportSpec {
        ranges_s,
        crop: e.crop,
        effects: e.effects,
        stretch: e.stretch,
        speed: e.speed,
        preserve_pitch: e.preserve_pitch,
        has_manifest_tracks: rows.iter().any(|r| r.id != "default"),
        has_audio: !rows.is_empty(),
        sources,
        master_percent: e.master_volume,
        watermark,
        audio_bitrate_kbps: AUDIO_KBPS,
        extra_video_filters: Vec::new(),
        force_video_encode: false,
        force_audio_encode: false,
    })
}

fn fail(state: &mut AppState, detail: &str) {
    state.error_bar.push_error(
        "EXPORT FAILED",
        detail,
        Level::Warning,
        Vec::new(),
        None,
        false,
    );
}

/// "Export" / Ctrl+E: plans the edit and queues it.
pub fn export_now(state: &mut AppState) {
    state.export_queue.configure(&state.settings);
    let spec = match spec_for(state) {
        Ok(s) => s,
        Err(e) => return fail(state, &e),
    };
    let graph = match plan(&spec) {
        Ok(g) => g,
        Err(e) => return fail(state, &e),
    };
    let Some(source) = state.editor.as_ref().map(|e| e.path.clone()) else {
        return;
    };
    let dir = state.library.clips_root.join("Exported");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return fail(state, &format!("Cannot create {}: {e}", dir.display()));
    }
    let kept: f64 = spec.ranges_s.iter().map(|(a, b)| b - a).sum();
    let expected = speed_adjusted_duration(kept, spec.speed);
    let quality = quality(state);
    let src = source.clone();
    let decode_from_s = first_range_start(&spec);
    state.export_queue.submit(
        source,
        &dir,
        quality,
        Box::new(move |output: &Path| Job {
            source: src,
            output: output.to_path_buf(),
            plan: graph,
            quality,
            video_bitrate_kbps: None,
            max_rate_kbps: None,
            expected_duration_s: expected,
            target_size_mb: None,
            output_fps: None,
            decode_from_s,
            inactivity_s: None,
            fail_first_encoder: false,
        }),
        expected,
    );
    state.export_jobs_label = None;
}

/// Where a multi-range export may start decoding (its trims keep source
/// timestamps); a single range seeks itself.
pub fn first_range_start(spec: &ExportSpec) -> f64 {
    if spec.ranges_s.len() > 1 {
        spec.ranges_s[0].0
    } else {
        0.0
    }
}

/// `Export jobs` plus the pending / failed / completed suffix.
pub fn jobs_label(queue: &ExportQueue) -> String {
    format!("Export jobs{}", ExportQueue::label_suffix(queue.counts()))
}

/// [`jobs_label`], recomputed at most once a second (the reference's
/// 1000 ms timer).
pub fn jobs_label_cached(state: &mut AppState, now_s: f64) -> String {
    match &state.export_jobs_label {
        Some((label, at)) if now_s - at < 1.0 && now_s >= *at => label.clone(),
        _ => {
            let label = jobs_label(&state.export_queue);
            state.export_jobs_label = Some((label.clone(), now_s));
            label
        }
    }
}
