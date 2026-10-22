//! The Performance tab (`_make_performance_page`): export concurrency,
//! interface performance presets/options and audio processing.

use egui::{Pos2, Rect, Sense, Ui, Vec2};
use serde_json::json;

use super::space;
use crate::fonts::Weight;
use crate::state::AppState;
use crate::tokens::{Color, size};
use crate::widgets::buttons::{self, Kind, Opts};
use crate::widgets::combo::Combo;
use crate::widgets::{check, section};

/// `UI_OPTIONS`: `(key, default, title, impact, detail)`.
pub const UI_OPTIONS: [(&str, bool, &str, &str, &str); 8] = [
    (
        "clip_editor_enabled",
        true,
        "Full clip editor",
        "Very high",
        "Off: open clips in a simple video player with playback and volume controls, without the editing tools. Clips already open in the editor stay as they are.",
    ),
    (
        "clip_editor_live_preview",
        true,
        "Live edit preview",
        "High",
        "Off: show the original video while you edit, instead of showing your crops, stretching and color changes as you make them. Your edits are still saved and will appear in exported clips.",
    ),
    (
        "editor_color_preview_enabled",
        true,
        "Color effects in preview",
        "High",
        "Off: hide color changes while a clip plays. Cropping and stretching still appear, and exported clips keep your color changes.",
    ),
    (
        "timeline_preview_strip_enabled",
        true,
        "Seek bar preview",
        "High",
        "Off: hide the small preview pictures along the clip’s timeline.",
    ),
    (
        "library_thumbnails_enabled",
        true,
        "Library thumbnails",
        "Medium",
        "Off: hide preview pictures in your clip library. You can still find, open and manage clips by name.",
    ),
    (
        "settings_visual_previews_enabled",
        true,
        "Animated settings previews",
        "Medium",
        "Off: stop the moving camera and keyboard previews in Settings. This does not change what appears in recordings. After turning this back on, reopen Visuals to see the previews.",
    ),
    (
        "reduce_ui_animations",
        false,
        "Reduce interface animations",
        "Medium",
        "On: use fewer animations when you move the mouse over buttons or switch between screens.",
    ),
    (
        "pause_ui_in_background",
        true,
        "Pause interface in background",
        "Medium",
        "On: let the app do less work when you’re using another app or its window is minimized. Recording continues.",
    ),
];
/// Preset names in combo order.
pub const PRESETS: [&str; 4] = [
    "Full experience",
    "Balanced",
    "Low power",
    "Minimal / viewer only",
];
const EXPORT_HINT: &str = "Off by default: clips export one at a time. You can keep browsing and queue more clips. Simultaneous exports use more CPU, GPU, memory and disk bandwidth and can affect recording or games. Lowering the limit lets running jobs finish before starting more.";
const PRESET_HINT: &str = "Choose a preset, or adjust the options below to suit your PC. Impact shows how much each option may help your PC run more smoothly; results depend on how you use the app. These settings do not change recording or export quality.";

/// The option values of a preset.
pub fn preset_values(name: &str) -> Option<Vec<(&'static str, bool)>> {
    let defaults: Vec<(&str, bool)> = UI_OPTIONS.iter().map(|(k, d, ..)| (*k, *d)).collect();
    let with = |overrides: &[(&str, bool)]| -> Vec<(&'static str, bool)> {
        defaults
            .iter()
            .map(|(k, d)| {
                (
                    *k,
                    overrides
                        .iter()
                        .find(|(ok, _)| ok == k)
                        .map(|(_, v)| *v)
                        .unwrap_or(*d),
                )
            })
            .collect()
    };
    let balanced = [
        ("reduce_ui_animations", true),
        ("settings_visual_previews_enabled", false),
        ("timeline_preview_strip_enabled", false),
    ];
    let low = [
        ("reduce_ui_animations", true),
        ("settings_visual_previews_enabled", false),
        ("timeline_preview_strip_enabled", false),
        ("clip_editor_live_preview", false),
        ("editor_color_preview_enabled", false),
        ("library_thumbnails_enabled", false),
    ];
    let minimal = [
        ("reduce_ui_animations", true),
        ("settings_visual_previews_enabled", false),
        ("timeline_preview_strip_enabled", false),
        ("clip_editor_live_preview", false),
        ("editor_color_preview_enabled", false),
        ("library_thumbnails_enabled", false),
        ("clip_editor_enabled", false),
    ];
    match name {
        "Full experience" => Some(with(&[])),
        "Balanced" => Some(with(&balanced)),
        "Low power" => Some(with(&low)),
        "Minimal / viewer only" => Some(with(&minimal)),
        _ => None,
    }
}

/// `matching_preset`: the preset equal to the current options, else `Custom`.
pub fn matching_preset(state: &AppState) -> &'static str {
    let current: Vec<(&str, bool)> = UI_OPTIONS
        .iter()
        .map(|(k, d, ..)| (*k, state.flag(k, *d)))
        .collect();
    PRESETS
        .iter()
        .copied()
        .find(|p| preset_values(p).as_deref() == Some(current.as_slice()))
        .unwrap_or("Custom")
}

/// Writes a preset's options.
pub fn apply_preset(state: &mut AppState, name: &str) {
    if let Some(values) = preset_values(name) {
        for (k, v) in values {
            state.set_setting(k, json!(v));
        }
        state.tokens.reduce_motion = state.flag("reduce_ui_animations", false);
    }
}

/// The Performance page body (margins (0, 0, 12, 20)).
pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    section::flat_header(ui, &t, "Clip exports", "Clip exports");
    space(ui, 12.0);
    let mut parallel = state.flag("parallel_exports", false);
    if check::checkbox_with(
        ui,
        &t,
        &mut parallel,
        "Allow simultaneous exports",
        true,
        true,
        "QCheckBox[Allow simultaneous exports]",
    )
    .changed()
    {
        state.set_setting("parallel_exports", json!(parallel));
    }
    let limit = state
        .settings
        .get_i64("parallel_export_limit")
        .unwrap_or(2)
        .clamp(2, 4);
    let mut combo = Combo::new(
        (2..=4).map(|n| format!("{n} exports at once")).collect(),
        (limit - 2) as usize,
    );
    combo.enabled = parallel;
    let name = format!("_DropdownCombo[{}]", combo.current());
    if combo.ui(ui, &t, &mut state.icons, &name).changed() {
        state.set_setting("parallel_export_limit", json!(combo.selected as i64 + 2));
    }
    section::label_wrapped_fit(
        ui,
        &t,
        EXPORT_HINT,
        size::BODY,
        Weight::W400,
        Color::TextDim,
        w,
    );
    let opts = Opts {
        expand: true,
        ..Opts::default()
    };
    if buttons::button_with(
        ui,
        &t,
        None,
        Kind::Primary,
        "View export jobs",
        "QPushButton[View export jobs]",
        &opts,
    )
    .clicked()
    {
        state.export_jobs_requested = true;
    }
    space(ui, 20.0);
    section::flat_header(ui, &t, "Interface performance", "Interface performance");
    space(ui, 12.0);
    let current = matching_preset(state);
    let mut items: Vec<String> = PRESETS.iter().map(|s| s.to_string()).collect();
    items.push("Custom".into());
    let sel = items
        .iter()
        .position(|s| s == current)
        .unwrap_or(items.len() - 1);
    let mut combo = Combo::new(items.clone(), sel);
    let name = format!("_DropdownCombo[{}]", combo.current());
    if combo.ui(ui, &t, &mut state.icons, &name).changed() {
        apply_preset(state, &items[combo.selected]);
    }
    section::label_wrapped_fit(
        ui,
        &t,
        PRESET_HINT,
        size::BODY,
        Weight::W400,
        Color::TextDim,
        w,
    );
    space(ui, 12.0);
    for (key, default, title, impact, detail) in UI_OPTIONS {
        let mut on = state.flag(key, default);
        let label = format!("{title} · {impact} impact");
        if check::checkbox_with(
            ui,
            &t,
            &mut on,
            &label,
            true,
            true,
            &format!("{key}_performance_check"),
        )
        .changed()
        {
            state.set_setting(key, json!(on));
            if key == "reduce_ui_animations" {
                state.tokens.reduce_motion = on;
            }
        }
        section::label_wrapped_fit(ui, &t, detail, size::BODY, Weight::W400, Color::TextDim, w);
        space(ui, 12.0);
    }
    space(ui, 16.0);
    section::hsep(ui, &t, "performanceHsep");
    space(ui, 20.0);
    section::flat_header(ui, &t, "Audio Processing", "Audio Processing");
    space(ui, 12.0);
    let mut audio = state.flag("audio_capture_enabled", true);
    if check::checkbox_with(
        ui,
        &t,
        &mut audio,
        "Enable audio capture · Medium impact",
        true,
        true,
        "QCheckBox[Enable audio capture · Medium impact]",
    )
    .changed()
    {
        state.set_setting("audio_capture_enabled", json!(audio));
        state.engine_restart_requested = true;
    }
    space(ui, 8.0);
    // Video encoding (NVIDIA preset) only when the active encoder supports presets.
    let encoder_key = state
        .settings
        .get_str("encoder_pref")
        .unwrap_or("auto")
        .to_owned();
    let supports = state
        .encoder_capabilities
        .as_ref()
        .and_then(|c| c.iter().find(|c| c.key == encoder_key))
        .map(|c| c.supports_presets)
        .unwrap_or(false);
    if supports {
        section::flat_header(ui, &t, "Video Encoding", "Video Encoding");
        space(ui, 12.0);
        let top = ui.cursor().min;
        section::label_in(
            ui,
            &t,
            Rect::from_min_size(top, Vec2::new(140.0, 38.0)),
            "NVIDIA preset",
            size::MICRO,
            Weight::W600,
            Color::Text,
            None,
        );
        let saved = state
            .settings
            .get_i64("encoder_preset")
            .unwrap_or(4)
            .clamp(1, 7);
        let labels = [
            "P1 — Fastest",
            "P2",
            "P3",
            "P4 — Balanced",
            "P5",
            "P6",
            "P7 — Best Quality",
        ];
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + 152.0, top.y),
            Vec2::new(w - 152.0, 38.0),
        )));
        let mut combo = Combo::new(
            labels.iter().map(|s| s.to_string()).collect(),
            (saved - 1) as usize,
        );
        let name = format!("_DropdownCombo[{}]", combo.current());
        if combo.ui(&mut c, &t, &mut state.icons, &name).changed() {
            state.set_setting("encoder_preset", json!(combo.selected as i64 + 1));
            state.encoder_dirty = true;
        }
        ui.allocate_exact_size(Vec2::new(w, 38.0), Sense::hover());
        if state.encoder_dirty {
            space(ui, 8.0);
            if buttons::primary(ui, &t, "Apply", "QPushButton[Apply]").clicked() {
                state.encoder_dirty = false;
                state.engine_restart_requested = true;
            }
        }
    }
    space(ui, 28.0);
    section::hsep(ui, &t, "performanceHsep2");
    space(ui, 20.0);
}
