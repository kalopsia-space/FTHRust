//! `GameDetectionPopup`: detection toggle, switch mode, desktop fallback,
//! manual game rules and the blocked list.

use egui::{Pos2, Rect, Ui, Vec2};
use serde_json::{Value, json};

use crate::fonts::Weight;
use crate::icons::Icon;
use crate::state::AppState;
use crate::tokens::{Color, size};
use crate::widgets::buttons::{self, IconButtonStyle, Kind, Opts, icon_button, segment_button};
use crate::widgets::combo::Combo;
use crate::widgets::{section, toggle};

/// `setMinimumWidth(420)`.
pub const MIN_WIDTH: f32 = 420.0;
const MARGINS: (f32, f32, f32, f32) = (18.0, 14.0, 18.0, 16.0);
const SPACING: f32 = 12.0;

/// A rule's display label (`_rebuild_rules`).
pub fn rule_label(rule: &Value) -> String {
    let title = rule
        .get("title_contains")
        .and_then(Value::as_str)
        .unwrap_or("");
    let exe = rule
        .get("exe_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            rule.get("exe_path").and_then(Value::as_str).and_then(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
            })
        })
        .unwrap_or_default();
    let folder = rule
        .get("folder_path")
        .and_then(Value::as_str)
        .and_then(|p| {
            std::path::Path::new(p)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    if !title.is_empty() && !exe.is_empty() {
        return format!("{title} · {exe}");
    }
    [title.to_owned(), exe, folder]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "Manual game".into())
}

fn divider(ui: &mut Ui, state: &AppState) {
    // A `QFrame.HLine` with `max-height: 1px`: a 3 px frame with the line in the middle.
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 3.0), egui::Sense::hover());
    crate::widgets::rule_h(
        ui,
        rect.min.x,
        rect.max.x,
        rect.min.y + 1.0,
        state.tokens.color(Color::Border),
    );
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
    let w = inner.width();
    let mut enabled = state.flag("game_detection_enabled", false);
    // Header: label + stretch + toggle (20 px row).
    let top = body.cursor().min;
    let tw = crate::widgets::galley(
        &body,
        "Game detection",
        size::MICRO,
        Weight::W600,
        egui::Color32::WHITE,
    )
    .size()
    .x
    .round();
    section::label_in(
        &mut body,
        &t,
        Rect::from_min_size(top, Vec2::new(tw, 20.0)),
        "Game detection",
        size::MICRO,
        Weight::W600,
        Color::Text,
        None,
    );
    let mut tg = body.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + w - 38.0, top.y),
        Vec2::new(38.0, 20.0),
    )));
    if toggle::switch(&mut tg, &t, &mut enabled, "gameDetectionSwitch").changed() {
        state.set_setting("game_detection_enabled", json!(enabled));
    }
    body.allocate_exact_size(Vec2::new(w, 20.0), egui::Sense::hover());
    divider(&mut body, state);
    section::label_wide(
        &mut body,
        &t,
        "Switch mode",
        size::MICRO,
        Weight::W600,
        Color::Text,
        None,
    );
    let mode = state
        .settings
        .get_str("game_detection_mode")
        .unwrap_or("auto")
        .to_ascii_lowercase();
    let mode = if mode == "prompt" { "prompt" } else { "auto" };
    let top = body.cursor().min;
    let half_a = ((w - 8.0) / 2.0).ceil();
    let half_b = w - 8.0 - half_a;
    let mut a = body.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(top, Vec2::new(half_a, 30.0))),
    );
    if segment_button(
        &mut a,
        &t,
        "AUTO-SWITCH",
        mode == "auto",
        enabled,
        half_a,
        "QToolButton[AUTO-SWITCH]",
    )
    .clicked()
    {
        state.set_setting("game_detection_mode", json!("auto"));
    }
    let mut b = body.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + half_a + 8.0, top.y),
        Vec2::new(half_b, 30.0),
    )));
    if segment_button(
        &mut b,
        &t,
        "PROMPT FIRST",
        mode == "prompt",
        enabled,
        half_b,
        "QToolButton[PROMPT FIRST]",
    )
    .clicked()
    {
        state.set_setting("game_detection_mode", json!("prompt"));
    }
    body.allocate_exact_size(Vec2::new(w, 30.0), egui::Sense::hover());
    // Fallback row.
    let top = body.cursor().min;
    let tw = crate::widgets::galley(
        &body,
        "Fall back to desktop",
        size::MICRO,
        Weight::W600,
        egui::Color32::WHITE,
    )
    .size()
    .x
    .round();
    section::label_in(
        &mut body,
        &t,
        Rect::from_min_size(top, Vec2::new(tw, 20.0)),
        "Fall back to desktop",
        size::MICRO,
        Weight::W600,
        Color::TextDim,
        None,
    );
    let mut fallback = state.flag("game_detection_fallback_desktop", false);
    let mut tg = body.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + w - 38.0, top.y),
        Vec2::new(38.0, 20.0),
    )));
    if toggle::switch_with(&mut tg, &t, &mut fallback, enabled, "gameFallbackSwitch").changed() {
        state.set_setting("game_detection_fallback_desktop", json!(fallback));
    }
    body.allocate_exact_size(Vec2::new(w, 20.0), egui::Sense::hover());
    divider(&mut body, state);
    section::label_wide(
        &mut body,
        &t,
        "Manual games",
        size::MICRO,
        Weight::W600,
        Color::Text,
        None,
    );
    let items: Vec<String> = if state.running_windows.is_empty() {
        vec![String::new()]
    } else {
        state.running_windows.clone()
    };
    let mut combo = Combo::new(
        items,
        state
            .running_window_index
            .min(state.running_windows.len().saturating_sub(1)),
    );
    let name = format!("QComboBox[{}]", combo.current());
    if combo.ui(&mut body, &t, &mut state.icons, &name).changed() {
        state.running_window_index = combo.selected;
    }
    // Three outline buttons, spacing 12.
    let top = body.cursor().min;
    let bw = ((w - 24.0) / 3.0).floor();
    let labels = ["Refresh windows", "Add manual game", "Block detection"];
    let mut clicked = [false; 3];
    for (i, label) in labels.iter().enumerate() {
        let x = top.x + i as f32 * (bw + 12.0);
        let width = if i == 2 { w - 2.0 * (bw + 12.0) } else { bw };
        let mut child = body.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(x, top.y),
            Vec2::new(width, 41.0),
        )));
        let opts = Opts {
            min_width: width,
            ..Opts::default()
        };
        clicked[i] = buttons::button_with(
            &mut child,
            &t,
            None,
            Kind::Outline,
            label,
            &format!("QPushButton[{label}]"),
            &opts,
        )
        .clicked();
    }
    body.allocate_exact_size(Vec2::new(w, 41.0), egui::Sense::hover());
    if clicked[0] {
        state.refresh_windows_requested = true;
    }
    let selected_window = state
        .running_windows
        .get(state.running_window_index)
        .cloned();
    if clicked[1]
        && let Some(win) = &selected_window
    {
        let mut rules = state
            .settings
            .get("game_detection_custom_games")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        rules.push(
            json!({"title_contains": win, "exe_name": "", "exe_path": "", "folder_path": ""}),
        );
        state.set_setting("game_detection_custom_games", Value::Array(rules));
    }
    if clicked[2]
        && let Some(win) = &selected_window
    {
        let mut rules = state
            .settings
            .get("game_detection_blacklist")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        rules.push(json!({"title_contains": win, "exe_name": ""}));
        state.set_setting("game_detection_blacklist", Value::Array(rules));
    }
    // Rules (spacing 4).
    let rules = state
        .settings
        .get("game_detection_custom_games")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let blocked = state
        .settings
        .get("game_detection_blacklist")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let top = body.cursor().min;
    let mut rl = body.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(top, Vec2::new(w, f32::INFINITY))),
    );
    rl.spacing_mut().item_spacing = Vec2::new(0.0, 4.0);
    let mut remove_rule: Option<usize> = None;
    let mut remove_blocked: Option<usize> = None;
    if rules.is_empty() {
        section::label_wide(
            &mut rl,
            &t,
            "No manual game rules.",
            size::MICRO,
            Weight::W400,
            Color::TextMuted,
            None,
        );
    }
    for (i, rule) in rules.iter().enumerate() {
        let row_top = rl.cursor().min;
        section::label_in(
            &mut rl,
            &t,
            Rect::from_min_size(row_top, Vec2::new(w - 24.0 - 12.0, 24.0)),
            &rule_label(rule),
            size::MICRO,
            Weight::W400,
            Color::TextDim,
            None,
        );
        let mut c = rl.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(row_top.x + w - 24.0, row_top.y),
            Vec2::splat(24.0),
        )));
        let style = IconButtonStyle {
            size: Vec2::splat(24.0),
            bg: None,
            hover_bg: None,
            pressed_bg: None,
            border: Some(Color::BorderHi),
            hover_border: Some(Color::Accent),
            radius: 6.0,
            hover_tint: Some(Color::Accent),
            enabled: true,
        };
        if icon_button(
            &mut c,
            &t,
            &mut state.icons,
            Icon::Close,
            14.0,
            style,
            &format!("ruleRemove#{i}"),
        )
        .clicked()
        {
            remove_rule = Some(i);
        }
        rl.allocate_exact_size(Vec2::new(w, 24.0), egui::Sense::hover());
    }
    section::label_wide(
        &mut rl,
        &t,
        "Blocked from game detection",
        size::BODY,
        Weight::W400,
        Color::Text,
        None,
    );
    if blocked.is_empty() {
        section::label_wide(
            &mut rl,
            &t,
            "No blocked applications.",
            size::BODY,
            Weight::W400,
            Color::Text,
            None,
        );
    }
    for (i, rule) in blocked.iter().enumerate() {
        let name = rule
            .get("exe_name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| rule.get("title_contains").and_then(Value::as_str))
            .unwrap_or("");
        let row_top = rl.cursor().min;
        section::label_in(
            &mut rl,
            &t,
            Rect::from_min_size(row_top, Vec2::new(w - 36.0, 24.0)),
            name,
            size::BODY,
            Weight::W400,
            Color::Text,
            None,
        );
        let mut c = rl.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(row_top.x + w - 24.0, row_top.y),
            Vec2::splat(24.0),
        )));
        let style = IconButtonStyle {
            size: Vec2::splat(24.0),
            bg: None,
            hover_bg: None,
            pressed_bg: None,
            border: Some(Color::BorderHi),
            hover_border: Some(Color::Accent),
            radius: 6.0,
            hover_tint: Some(Color::Accent),
            enabled: true,
        };
        if icon_button(
            &mut c,
            &t,
            &mut state.icons,
            Icon::Close,
            14.0,
            style,
            &format!("blockedRemove#{i}"),
        )
        .clicked()
        {
            remove_blocked = Some(i);
        }
        rl.allocate_exact_size(Vec2::new(w, 24.0), egui::Sense::hover());
    }
    let rules_h = rl.cursor().min.y - 4.0 - top.y;
    body.allocate_exact_size(Vec2::new(w, rules_h), egui::Sense::hover());
    if let Some(i) = remove_rule {
        let mut r = rules.clone();
        r.remove(i);
        state.set_setting("game_detection_custom_games", Value::Array(r));
    }
    if let Some(i) = remove_blocked {
        let mut r = blocked.clone();
        r.remove(i);
        state.set_setting("game_detection_blacklist", Value::Array(r));
    }
    let bottom = body.cursor().min.y - SPACING + MARGINS.3;
    ui.allocate_rect(
        Rect::from_min_max(outer.min, Pos2::new(outer.max.x, bottom)),
        egui::Sense::hover(),
    );
}
