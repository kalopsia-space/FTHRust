//! The 64 px capture toolbar (`topClusterMain` inside
//! `captureToolbarScroll`): Record, capture summary, Source, Game detection,
//! Hotkeys (stretch 1:3:2:2:1) and the status label; margins (14, 6, 14, 6),
//! spacing 8.

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use fthrust_core::settings::SettingsStore;

use crate::fidelity;
use crate::fonts::Weight;
use crate::motion;
use crate::state::{AppState, StatusStyle};
use crate::tokens::{Color, Tokens, geom, size};
use crate::widgets::buttons::top_bar_button;
use crate::widgets::{galley, lerp, paint_text};

/// Which popup a toolbar button asked to toggle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum PopupKind {
    Capture,
    Source,
    Game,
    Hotkeys,
}

/// Toolbar result.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolbarResponse {
    /// Popup toggled this frame.
    pub toggled: Option<PopupKind>,
    /// Record clicked.
    pub record: bool,
    /// Anchor rects for the popups (capture, source, game, hotkeys).
    pub anchors: [Rect; 4],
}

impl Default for ToolbarResponse {
    fn default() -> Self {
        ToolbarResponse {
            toggled: None,
            record: false,
            anchors: [Rect::NOTHING; 4],
        }
    }
}

const BITRATE_PRESETS: [(&str, [(&str, i64); 3]); 5] = [
    ("480p", [("low", 2500), ("medium", 5000), ("high", 10000)]),
    ("720p", [("low", 5000), ("medium", 12000), ("high", 20000)]),
    (
        "1080p",
        [("low", 10000), ("medium", 25000), ("high", 50000)],
    ),
    (
        "1440p",
        [("low", 15000), ("medium", 35000), ("high", 60000)],
    ),
    (
        "source",
        [("low", 10000), ("medium", 25000), ("high", 50000)],
    ),
];

/// `BITRATE_PRESETS[res][quality]` (falls back to `source` / `high`).
pub fn preset_bitrate(resolution: &str, quality: &str) -> i64 {
    let res = resolution.to_ascii_lowercase();
    let row = BITRATE_PRESETS
        .iter()
        .find(|(k, _)| *k == res)
        .or_else(|| BITRATE_PRESETS.iter().find(|(k, _)| *k == "source"))
        .unwrap();
    row.1
        .iter()
        .find(|(q, _)| *q == quality)
        .map(|(_, v)| *v)
        .unwrap_or(row.1[2].1)
}

/// `_clip_length_label`: `30s`, `1.5m`, `2m`.
pub fn clip_length_label(seconds: i64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let value = format!("{:.2}", seconds as f64 / 60.0);
    let value = value.trim_end_matches('0').trim_end_matches('.');
    format!("{value}m")
}

/// Resolution display label (`_RES_LABELS`).
pub fn resolution_label(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "480p" => "480p".into(),
        "720p" => "720p".into(),
        "1080p" => "1080p".into(),
        "1440p" => "1440p".into(),
        "source" => "Source".into(),
        _ => key.to_owned(),
    }
}

/// Thousands separators (Python `{:,}`).
pub fn thousands(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// The effective bitrate for the clip profile.
pub fn clip_bitrate(settings: &SettingsStore) -> i64 {
    let res = settings
        .get_str("resolution")
        .unwrap_or("source")
        .to_owned();
    let quality = settings
        .get_str("bitrate_level")
        .unwrap_or("medium")
        .to_owned();
    if quality == "custom" {
        let fallback = preset_bitrate(&res, "high");
        settings
            .get_i64("custom_bitrate_kbps")
            .filter(|v| (500..=200_000).contains(v))
            .unwrap_or(fallback)
    } else {
        preset_bitrate(&res, &quality)
    }
}

/// `CaptureSettingsPopup.get_summary`: `30s // 60 fps // Source // 25,000 kbps`.
pub fn capture_summary(settings: &SettingsStore) -> String {
    let clip = settings.get_i64("clip_length").unwrap_or(30);
    let fps = settings.get_i64("framerate").unwrap_or(60);
    let res = settings.get_str("resolution").unwrap_or("source");
    format!(
        "{} // {fps} fps // {} // {} kbps",
        clip_length_label(clip),
        resolution_label(res),
        thousands(clip_bitrate(settings))
    )
}

/// `GameDetectionPopup.get_summary`.
pub fn game_summary(settings: &SettingsStore, available: bool) -> String {
    if !available {
        return "Game detection unavailable".into();
    }
    if settings.get_bool("game_detection_enabled").unwrap_or(false) {
        "Game detection: On".into()
    } else {
        "Game detection: Off".into()
    }
}

/// Python `str.capitalize()`.
pub fn status_display(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// Status label colour per style.
pub fn status_color(style: StatusStyle) -> Color {
    match style {
        StatusStyle::Active => Color::Accent,
        StatusStyle::Idle => Color::TextMuted,
        StatusStyle::Warning => Color::Error,
    }
}

/// Qt box-layout widths for stretch factors over `avail`: each item gets
/// at least its hint; the rest is shared by stretch with cumulative
/// rounding (`qGeomCalc`).
pub fn stretch_widths(avail: f32, stretch: &[u32], hints: &[f32]) -> Vec<f32> {
    let n = stretch.len();
    let mut fixed = vec![false; n];
    let mut widths = vec![0.0f32; n];
    loop {
        let fixed_sum: f32 = (0..n).filter(|i| fixed[*i]).map(|i| hints[i]).sum();
        let free = (avail - fixed_sum).max(0.0);
        let stretch_sum: f32 = (0..n)
            .filter(|i| !fixed[*i])
            .map(|i| stretch[i] as f32)
            .sum();
        let mut changed = false;
        for i in 0..n {
            if fixed[i] {
                continue;
            }
            let share = if stretch_sum > 0.0 {
                free * stretch[i] as f32 / stretch_sum
            } else {
                0.0
            };
            if share < hints[i] {
                fixed[i] = true;
                changed = true;
            }
        }
        if !changed {
            // Cumulative rounding over the shared items, in order.
            let mut cum = 0.0f32;
            let mut prev_round = 0.0f32;
            for i in 0..n {
                if fixed[i] {
                    widths[i] = hints[i].round();
                    continue;
                }
                cum += free * stretch[i] as f32 / stretch_sum;
                let r = cum.round();
                widths[i] = r - prev_round;
                prev_round = r;
            }
            return widths;
        }
    }
}

fn hint_width(ui: &Ui, text: &str) -> f32 {
    galley(ui, text, size::TOOLBAR, Weight::W400, Color32::WHITE)
        .size()
        .x
        .ceil()
        + 14.0
        + 30.0
}

/// Draws the toolbar in the available width (64 px tall).
pub fn ui(ui: &mut Ui, state: &mut AppState) -> ToolbarResponse {
    let t = state.tokens.clone();
    let origin = ui.cursor().min;
    let width = ui.available_width();
    let bar = Rect::from_min_size(origin, Vec2::new(width, geom::CAPTURE_BAR_H));
    ui.painter().rect_filled(bar, 0.0, t.color(Color::Surface3));
    fidelity::mark(ui, "captureToolbarScroll", bar)
        .sample(
            "fill",
            Pos2::new(bar.min.x + 4.0, bar.center().y),
            Color::Surface3,
        )
        .done();
    fidelity::mark(ui, "topClusterMain", bar)
        .sample(
            "fill",
            Pos2::new(bar.min.x + 4.0, bar.center().y),
            Color::Surface3,
        )
        .done();
    let inner = Rect::from_min_max(
        bar.min + Vec2::new(14.0, 6.0),
        bar.max - Vec2::new(14.0, 6.0),
    );
    let status_text = status_display(&state.status.text);
    let status_galley = galley(ui, &status_text, size::LABEL, Weight::W400, Color32::WHITE);
    let status_w = status_galley.size().x.ceil().max(110.0);
    let summary = capture_summary(&state.settings);
    let game = game_summary(&state.settings, state.game_detection_available);
    let labels = [
        state.source_label.clone(),
        game.clone(),
        "Hotkeys".to_owned(),
    ];
    let record_hint = galley(ui, "Record", size::H3, Weight::W600, Color32::WHITE)
        .size()
        .x
        .ceil()
        + 24.0
        + 2.0;
    let hints = [
        record_hint,
        hint_width(ui, &summary),
        hint_width(ui, &labels[0]),
        hint_width(ui, &labels[1]),
        hint_width(ui, &labels[2]),
    ];
    let avail = inner.width() - status_w - 8.0 * 5.0;
    let widths = stretch_widths(avail, &[1, 3, 2, 2, 1], &hints);
    let mut x = inner.min.x;
    let mut out = ToolbarResponse::default();
    // Record (`_manual_record_button_qss(False)`; disabled until plan 5 wires it).
    let record_rect = Rect::from_min_size(
        Pos2::new(x, inner.min.y + 4.0),
        Vec2::new(widths[0], geom::CAPTURE_CONTROL_H),
    );
    out.record = record_button(
        ui,
        &t,
        record_rect,
        &state.record_label,
        state.record_active,
        state.record_enabled,
    );
    x += widths[0] + 8.0;
    let names = ["cap_btn", "source_btn", "game_detection_btn", "hotkey_btn"];
    let texts = [
        summary.clone(),
        labels[0].clone(),
        labels[1].clone(),
        labels[2].clone(),
    ];
    let opens = [
        state.popups.capture.open,
        state.popups.source.open,
        state.popups.game.open,
        state.popups.hotkeys.open,
    ];
    let kinds = [
        PopupKind::Capture,
        PopupKind::Source,
        PopupKind::Game,
        PopupKind::Hotkeys,
    ];
    for i in 0..4 {
        let rect = Rect::from_min_size(
            Pos2::new(x, inner.min.y + 4.0),
            Vec2::new(widths[i + 1], geom::CAPTURE_CONTROL_H),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let marker = format!("TopBarButton[{}]", crate::fidelity::marker_label(&texts[i]));
        let r = top_bar_button(
            &mut child,
            &t,
            &mut state.icons,
            &texts[i],
            opens[i],
            widths[i + 1],
            &marker,
        );
        let _ = names[i];
        if r.clicked() {
            out.toggled = Some(kinds[i]);
        }
        out.anchors[i] = rect;
        x += widths[i + 1] + 8.0;
    }
    // Status label: right-aligned, full inner height.
    let status_rect = Rect::from_min_max(Pos2::new(inner.max.x - status_w, inner.min.y), inner.max);
    let color = t.color(status_color(state.status.style));
    paint_text(ui, status_galley, status_rect, Align::Max, 2.0, color);
    fidelity::mark(ui, "statusLabel", status_rect)
        .text(&status_text, size::LABEL, Weight::W400)
        .done();
    ui.allocate_rect(bar, Sense::hover());
    out
}

fn record_button(
    ui: &mut Ui,
    t: &Tokens,
    rect: Rect,
    label: &str,
    active: bool,
    enabled: bool,
) -> bool {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(rect, ui.scope_id().with("record_btn"), sense);
    let hovered = enabled && resp.hovered();
    let pressed = enabled && resp.is_pointer_button_down_on();
    let blend = motion::hover(ui.ctx(), resp.id.with("hover"), hovered, t.reduce_motion);
    // `_manual_record_button_qss(active)`: ACCENT/BG/ACCENT idle, DELETE/TEXT/ERROR active.
    let (base_bg, base_fg, border, hover_bg, hover_fg, pressed_bg) = if active {
        (
            Color::Delete,
            Color::Text,
            Color::Error,
            Color::ErrorSoft,
            Color::Error,
            Color::Delete,
        )
    } else {
        (
            Color::Accent,
            Color::Bg,
            Color::Accent,
            Color::AccentSoft,
            Color::Accent,
            Color::AccentDim,
        )
    };
    let (bg, fg) = if pressed {
        (t.color(pressed_bg), t.color(hover_fg))
    } else {
        (
            lerp(t.color(base_bg), t.color(hover_bg), blend),
            lerp(
                t.color(if enabled { base_fg } else { Color::TextMuted }),
                t.color(hover_fg),
                blend,
            ),
        )
    };
    let radius = CornerRadius::same(geom::RADIUS as u8);
    ui.painter().rect_filled(rect, radius, bg);
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, t.color(border)),
        StrokeKind::Inside,
    );
    let g = galley(ui, label, size::H3, Weight::W600, fg);
    paint_text(ui, g, rect, Align::Center, 0.0, fg);
    fidelity::mark(ui, "QPushButton[Record]", rect)
        .edge(border)
        .top(border)
        .text(label, size::H3, Weight::W600)
        .done();
    resp.clicked()
}
