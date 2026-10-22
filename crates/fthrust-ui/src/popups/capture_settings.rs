//! `CaptureSettingsPopup`: clip / recording profiles, clip length, custom
//! lengths, framerate, resolution, quality, custom bitrate and Apply.

use egui::{Pos2, Rect, Ui, Vec2};
use fthrust_core::capture_config::{CLIP_LENGTHS, FPS_VALUES, custom_clip_lengths};
use serde_json::json;

use super::labelled_row;
use crate::fonts::Weight;
use crate::shell::toolbar::{clip_length_label, preset_bitrate};
use crate::state::{AppState, CaptureProfile, DialogKind};
use crate::tokens::{Color, size};
use crate::widgets::buttons::{self, Kind, Opts, segment_button};
use crate::widgets::combo::{self, Combo};
use crate::widgets::input::number_spin;
use crate::widgets::section;

/// `setMinimumWidth(460)`.
pub const MIN_WIDTH: f32 = 460.0;
const MARGINS: (f32, f32, f32, f32) = (20.0, 16.0, 20.0, 16.0);
const SPACING: f32 = 12.0;
const LABEL_W: f32 = 92.0;
const RES_KEYS: [&str; 5] = ["480p", "720p", "1080p", "1440p", "source"];
const RES_LABELS: [&str; 5] = ["480p", "720p", "1080p", "1440p", "Source"];
const QUAL_KEYS: [&str; 4] = ["low", "medium", "high", "custom"];
const QUAL_LABELS: [&str; 4] = ["Low", "Medium", "High", "Custom"];

fn combo_row(
    ui: &mut Ui,
    state: &mut AppState,
    label: &str,
    items: Vec<String>,
    selected: usize,
) -> Option<usize> {
    let mut picked = None;
    labelled_row(ui, state, label, LABEL_W, combo::HEIGHT, |ui, state| {
        let t = state.tokens.clone();
        let mut c = Combo::new(items, selected);
        let name = format!("_DropdownCombo[{}]", c.current());
        let r = c.ui(ui, &t, &mut state.icons, &name);
        if r.changed() {
            picked = Some(c.selected);
        }
    });
    picked
}

/// Items for a value combo: the preset list plus the stored value when it
/// is not in the list (shown without being rewritten).
fn values_with(stored: i64, presets: &[u32]) -> Vec<i64> {
    let mut v: Vec<i64> = presets.iter().map(|x| *x as i64).collect();
    if !v.contains(&stored) {
        v.push(stored);
        v.sort_unstable();
    }
    v
}

/// The popup body.
pub fn content(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let outer = ui.max_rect();
    let inner = Rect::from_min_max(
        outer.min + Vec2::new(MARGINS.0, MARGINS.1),
        Pos2::new(outer.max.x - MARGINS.2, f32::INFINITY),
    );
    let mut body = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    body.spacing_mut().item_spacing = Vec2::new(0.0, SPACING);
    // Profile row.
    let w = inner.width();
    let half = ((w - 8.0) / 2.0).floor();
    let row_top = body.cursor().min;
    let mut r1 = body.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(row_top, Vec2::new(half, 30.0))),
    );
    if segment_button(
        &mut r1,
        &t,
        "Clips",
        state.capture_profile == CaptureProfile::Clips,
        true,
        half,
        "QToolButton[Clips]",
    )
    .clicked()
    {
        state.capture_profile = CaptureProfile::Clips;
    }
    let mut r2 = body.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(row_top.x + half + 8.0, row_top.y),
        Vec2::new(w - half - 8.0, 30.0),
    )));
    if segment_button(
        &mut r2,
        &t,
        "Recording",
        state.capture_profile == CaptureProfile::Recording,
        true,
        w - half - 8.0,
        "QToolButton[Recording]",
    )
    .clicked()
    {
        state.capture_profile = CaptureProfile::Recording;
    }
    body.allocate_exact_size(Vec2::new(w, 30.0), egui::Sense::hover());
    match state.capture_profile {
        CaptureProfile::Clips => clips_fields(&mut body, state),
        CaptureProfile::Recording => recording_fields(&mut body, state),
    }
    // Apply row (right-aligned), visible while a restart is pending on the clips profile.
    if state.restart_pending && state.capture_profile == CaptureProfile::Clips {
        let top = body.cursor().min;
        let mut row = body.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(top, Vec2::new(w, 41.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Min)),
        );
        if buttons::primary(&mut row, &t, "Apply", "QPushButton[Apply]").clicked() {
            state.restart_pending = false;
            state.engine_restart_requested = true;
        }
        body.allocate_exact_size(Vec2::new(w, 41.0), egui::Sense::hover());
    }
    let bottom = body.cursor().min.y - SPACING + MARGINS.3;
    ui.allocate_rect(
        Rect::from_min_max(outer.min, Pos2::new(outer.max.x, bottom)),
        egui::Sense::hover(),
    );
}

fn clips_fields(body: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = body.available_width();
    let cur_clip = state.settings.get_i64("clip_length").unwrap_or(30);
    let clip_values = values_with(cur_clip, &CLIP_LENGTHS);
    let labels: Vec<String> = clip_values.iter().map(|s| clip_length_label(*s)).collect();
    let sel = clip_values.iter().position(|v| *v == cur_clip).unwrap_or(0);
    if let Some(i) = combo_row(body, state, "Clip length", labels, sel) {
        state.set_setting("clip_length", json!(clip_values[i]));
    }
    // Add custom clip length.
    let opts = Opts {
        expand: true,
        ..Opts::default()
    };
    if buttons::button_with(
        body,
        &t,
        None,
        Kind::Outline,
        "Add custom clip length…",
        "QPushButton[Add custom clip length…]",
        &opts,
    )
    .clicked()
    {
        state.dialogs.push_modal(DialogKind::CustomClipLength {
            seconds: cur_clip as u32,
        });
    }
    let customs = custom_clip_lengths(state.settings.get_or_default("custom_clip_lengths"));
    if !customs.is_empty() {
        let list_h = (customs.len() as f32 * 48.0).min(150.0);
        let top = body.cursor().min;
        let mut list = body.new_child(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(top, Vec2::new(w, list_h))),
        );
        list.spacing_mut().item_spacing = Vec2::ZERO;
        let mut remove: Option<u32> = None;
        for seconds in &customs {
            let row_top = list.cursor().min;
            let row = Rect::from_min_size(row_top, Vec2::new(w, 48.0));
            let text = format!("Custom {}", clip_length_label(*seconds as i64));
            section::label_in(
                &mut list,
                &t,
                Rect::from_min_size(row.min, Vec2::new(w - 90.0, 48.0)),
                &text,
                size::MICRO,
                Weight::W600,
                Color::Text,
                None,
            );
            let mut btn = list.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_size(
                        Pos2::new(row.max.x - 90.0, row.min.y + 3.0),
                        Vec2::new(90.0, 41.0),
                    ))
                    .layout(egui::Layout::right_to_left(egui::Align::Min)),
            );
            if buttons::outline(
                &mut btn,
                &t,
                "Remove",
                &format!("QPushButton[Remove]#{seconds}"),
            )
            .clicked()
            {
                remove = Some(*seconds);
            }
            list.allocate_exact_size(Vec2::new(w, 48.0), egui::Sense::hover());
        }
        body.allocate_exact_size(Vec2::new(w, list_h), egui::Sense::hover());
        section::label_wrapped(
            body,
            &t,
            "Set a separate shortcut for each length in Hotkeys.",
            size::MICRO,
            Weight::W400,
            Color::TextMuted,
            w,
            0.0,
        );
        if let Some(s) = remove {
            let kept: Vec<u32> = customs.iter().copied().filter(|v| *v != s).collect();
            state.set_setting("custom_clip_lengths", json!(kept));
            state.hotkeys.sync_custom_clips(&kept);
            let _ = state.hotkeys.save(&state.paths);
            state.hotkeys_changed = true;
            state.restart_pending = true;
        }
    }
    if cur_clip >= 120 {
        section::label_wrapped(
            body,
            &t,
            "Longer clips can use a lot of memory, especially at high quality.",
            size::MICRO,
            Weight::W400,
            Color::TextMuted,
            w,
            0.0,
        );
    }
    let cur_fps = state.settings.get_i64("framerate").unwrap_or(60);
    let fps_values = values_with(cur_fps, &FPS_VALUES);
    let sel = fps_values.iter().position(|v| *v == cur_fps).unwrap_or(1);
    if let Some(i) = combo_row(
        body,
        state,
        "Framerate",
        fps_values.iter().map(|v| v.to_string()).collect(),
        sel,
    ) {
        state.set_setting("framerate", json!(fps_values[i]));
        state.restart_pending = true;
    }
    let cur_res = state
        .settings
        .get_str("resolution")
        .unwrap_or("source")
        .to_ascii_lowercase();
    let sel = RES_KEYS.iter().position(|k| *k == cur_res).unwrap_or(4);
    if let Some(i) = combo_row(
        body,
        state,
        "Resolution",
        RES_LABELS.iter().map(|s| s.to_string()).collect(),
        sel,
    ) {
        state.set_setting("resolution", json!(RES_KEYS[i]));
        state.restart_pending = true;
    }
    let cur_qual = state
        .settings
        .get_str("bitrate_level")
        .unwrap_or("medium")
        .to_owned();
    let sel = QUAL_KEYS.iter().position(|k| *k == cur_qual).unwrap_or(2);
    if let Some(i) = combo_row(
        body,
        state,
        "Quality",
        QUAL_LABELS.iter().map(|s| s.to_string()).collect(),
        sel,
    ) {
        state.set_setting("bitrate_level", json!(QUAL_KEYS[i]));
        if QUAL_KEYS[i] == "custom" {
            let v = state
                .settings
                .get_i64("custom_bitrate_kbps")
                .unwrap_or(preset_bitrate(&cur_res, "high"));
            state.set_setting("custom_bitrate_kbps", json!(v));
        }
        state.restart_pending = true;
    }
    bitrate_row(
        body,
        state,
        "custom_bitrate_kbps",
        &cur_res,
        cur_qual == "custom",
    );
}

fn recording_fields(body: &mut Ui, state: &mut AppState) {
    let base_fps = state.settings.get_i64("framerate").unwrap_or(60);
    let cur_fps = state
        .settings
        .get_i64("recording_framerate")
        .unwrap_or(base_fps);
    let fps_values = values_with(cur_fps, &FPS_VALUES);
    let sel = fps_values.iter().position(|v| *v == cur_fps).unwrap_or(1);
    if let Some(i) = combo_row(
        body,
        state,
        "Framerate",
        fps_values.iter().map(|v| v.to_string()).collect(),
        sel,
    ) {
        state.set_setting("recording_framerate", json!(fps_values[i]));
    }
    let base_res = state
        .settings
        .get_str("resolution")
        .unwrap_or("source")
        .to_ascii_lowercase();
    let cur_res = state
        .settings
        .get_str("recording_resolution")
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or(base_res);
    let sel = RES_KEYS.iter().position(|k| *k == cur_res).unwrap_or(4);
    if let Some(i) = combo_row(
        body,
        state,
        "Resolution",
        RES_LABELS.iter().map(|s| s.to_string()).collect(),
        sel,
    ) {
        state.set_setting("recording_resolution", json!(RES_KEYS[i]));
    }
    let base_qual = state
        .settings
        .get_str("bitrate_level")
        .unwrap_or("medium")
        .to_owned();
    let cur_qual = state
        .settings
        .get_str("recording_bitrate_level")
        .map(str::to_owned)
        .unwrap_or(base_qual);
    let sel = QUAL_KEYS.iter().position(|k| *k == cur_qual).unwrap_or(2);
    if let Some(i) = combo_row(
        body,
        state,
        "Quality",
        QUAL_LABELS.iter().map(|s| s.to_string()).collect(),
        sel,
    ) {
        state.set_setting("recording_bitrate_level", json!(QUAL_KEYS[i]));
    }
    bitrate_row(
        body,
        state,
        "recording_custom_bitrate_kbps",
        &cur_res,
        cur_qual == "custom",
    );
}

fn bitrate_row(body: &mut Ui, state: &mut AppState, key: &'static str, res: &str, enabled: bool) {
    let fallback = preset_bitrate(res, "high");
    let mut value = state
        .settings
        .get_i64(key)
        .filter(|v| (500..=200_000).contains(v))
        .unwrap_or(fallback);
    let before = value;
    labelled_row(body, state, "Custom bitrate", LABEL_W, 34.0, |ui, state| {
        let t = state.tokens.clone();
        if enabled {
            number_spin(
                ui,
                &t,
                &mut state.icons,
                &mut value,
                500..=200_000,
                500,
                " kbps",
                "fthrNumberSpin",
            );
        } else {
            number_spin_disabled(ui, state, value);
        }
    });
    if enabled && value != before {
        state.set_setting(key, json!(value));
        state.restart_pending = true;
    }
}

fn number_spin_disabled(ui: &mut Ui, state: &mut AppState, value: i64) {
    let t = state.tokens.clone();
    let mut v = value;
    let mut muted = t.clone();
    muted.reduce_motion = true;
    // Disabled: SURFACE_1 field, BORDER border, TEXT_MUTED text, steppers disabled.
    let rect = Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), 34.0));
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(6), t.color(Color::Surface1));
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(6),
        egui::Stroke::new(1.0, t.color(Color::Border)),
        egui::StrokeKind::Inside,
    );
    let text = format!("{v} kbps");
    let g = crate::widgets::galley(
        ui,
        &text,
        size::BODY,
        Weight::W600,
        t.color(Color::TextMuted),
    );
    crate::widgets::paint_text(
        ui,
        g,
        Rect::from_min_max(rect.min, Pos2::new(rect.max.x - 64.0, rect.max.y)),
        egui::Align::Max,
        10.0,
        t.color(Color::TextMuted),
    );
    for (i, (icon, name)) in [
        (crate::icons::Icon::Decrease, "fthrStepDown"),
        (crate::icons::Icon::Increase, "fthrStepUp"),
    ]
    .into_iter()
    .enumerate()
    {
        let brect = Rect::from_min_size(
            Pos2::new(rect.max.x - 57.0 + i as f32 * 28.0, rect.min.y + 1.0),
            Vec2::new(28.0, 32.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(brect));
        let style = crate::widgets::buttons::IconButtonStyle {
            size: Vec2::new(28.0, 32.0),
            bg: Some(Color::Surface1),
            hover_bg: None,
            pressed_bg: None,
            border: None,
            hover_border: None,
            radius: 4.0,
            hover_tint: None,
            enabled: false,
        };
        crate::widgets::buttons::icon_button(
            &mut child,
            &muted,
            &mut state.icons,
            icon,
            14.0,
            style,
            name,
        );
        crate::widgets::rule_v(
            ui,
            brect.min.x,
            brect.min.y,
            brect.max.y,
            t.color(Color::Border),
        );
    }
    crate::fidelity::mark(ui, "fthrNumberSpin", rect)
        .sample(
            "fill",
            Pos2::new(rect.min.x + 5.0, rect.center().y),
            Color::Surface1,
        )
        .edge(Color::Border)
        .done();
    ui.allocate_rect(rect, egui::Sense::hover());
    v += 0;
    let _ = v;
}
