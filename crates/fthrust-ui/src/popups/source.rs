//! `SourcePopup`: desktop / window mode and the monitor (or window) choice.

use egui::{Pos2, Rect, Ui, Vec2};
use serde_json::json;

use crate::fonts::Weight;
use crate::icons::Icon;
use crate::state::AppState;
use crate::tokens::{Color, size};
use crate::widgets::buttons::{IconButtonStyle, icon_button};
use crate::widgets::combo::Combo;
use crate::widgets::section;

/// `setMinimumWidth(300)` (the panel minimum).
pub const MIN_WIDTH: f32 = 300.0;
const MARGINS: (f32, f32, f32, f32) = (16.0, 14.0, 16.0, 14.0);
const SPACING: f32 = 12.0;

/// `f'{name}  ({w}×{h} @ {hz}Hz)'`.
pub fn monitor_label(name: &str, w: u32, h: u32, hz: u32) -> String {
    format!("{name}  ({w}×{h} @ {hz}Hz)")
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
    section::label_wide(
        &mut body,
        &t,
        "Capture source",
        size::MICRO,
        Weight::W600,
        Color::Text,
        None,
    );
    let mode = state
        .settings
        .get_str("capture_mode")
        .unwrap_or("desktop")
        .to_owned();
    let mut combo = Combo::new(
        vec!["Desktop".into(), "Window / Game".into()],
        if mode == "window" { 1 } else { 0 },
    );
    let name = format!("_DropdownCombo[{}]", combo.current());
    if combo.ui(&mut body, &t, &mut state.icons, &name).changed() {
        state.set_setting(
            "capture_mode",
            json!(if combo.selected == 1 {
                "window"
            } else {
                "desktop"
            }),
        );
        state.restart_pending = true;
    }
    if mode == "window" {
        // Window row: combo (min 240) + 28×28 refresh button, spacing 8.
        let top = body.cursor().min;
        let combo_w = (w - 8.0 - 28.0).max(240.0);
        let mut c = body.new_child(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(top, Vec2::new(combo_w, 38.0))),
        );
        let items: Vec<String> = if state.running_windows.is_empty() {
            vec![String::new()]
        } else {
            state.running_windows.clone()
        };
        let mut wc = Combo::new(
            items,
            state
                .running_window_index
                .min(state.running_windows.len().saturating_sub(1)),
        )
        .with_width(combo_w);
        let name = format!("_DropdownCombo[{}]", wc.current());
        if wc.ui(&mut c, &t, &mut state.icons, &name).changed() {
            state.running_window_index = wc.selected;
            state.restart_pending = true;
        }
        let brect = Rect::from_min_size(
            Pos2::new(top.x + combo_w + 8.0, top.y + 5.0),
            Vec2::splat(28.0),
        );
        let mut b = body.new_child(egui::UiBuilder::new().max_rect(brect));
        let style = IconButtonStyle {
            size: Vec2::splat(28.0),
            bg: Some(Color::Bg),
            hover_bg: None,
            pressed_bg: None,
            border: Some(Color::Text),
            hover_border: Some(Color::Accent),
            radius: 0.0,
            hover_tint: Some(Color::Accent),
            enabled: true,
        };
        if icon_button(
            &mut b,
            &t,
            &mut state.icons,
            Icon::Refresh,
            14.0,
            style,
            "sourceRefreshBtn",
        )
        .clicked()
        {
            state.refresh_windows_requested = true;
        }
        body.allocate_exact_size(Vec2::new(w, 38.0), egui::Sense::hover());
    } else {
        let saved = state
            .settings
            .get_str("capture_monitor")
            .unwrap_or("")
            .to_owned();
        let mut items = vec![("First Screen (Default)".to_owned(), String::new())];
        for m in &state.monitors {
            items.push((
                monitor_label(&m.name, m.width, m.height, m.refresh_hz),
                m.name.clone(),
            ));
        }
        let selected = items.iter().position(|(_, n)| *n == saved).unwrap_or(0);
        let mut mc = Combo::new(items.iter().map(|(l, _)| l.clone()).collect(), selected);
        let name = format!("_DropdownCombo[{}]", mc.current());
        if mc.ui(&mut body, &t, &mut state.icons, &name).changed() {
            state.set_setting("capture_monitor", json!(items[mc.selected].1));
            state.restart_pending = true;
        }
    }
    let bottom = body.cursor().min.y - SPACING + MARGINS.3;
    ui.allocate_rect(
        Rect::from_min_max(outer.min, Pos2::new(outer.max.x, bottom)),
        egui::Sense::hover(),
    );
}
