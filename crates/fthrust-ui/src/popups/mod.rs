//! The four top-bar popups (`_PopupPanel` subclasses): capture settings,
//! source, game detection and hotkeys. Each exposes `content(ui, state)`
//! for the frame and the tests, and `draw_all` shows the open one under its
//! toolbar button.

pub mod capture_settings;
pub mod game_detection;
pub mod hotkeys;
pub mod source;

use egui::Ui;

use crate::state::AppState;
use crate::widgets::popup::Panel;

/// Shows whichever popup is open, anchored to its toolbar button.
pub fn draw_all(ui: &mut Ui, state: &mut AppState) {
    let anchors = state.toolbar_anchors;
    let t = state.tokens.clone();
    if state.popups.capture.open {
        let mut st = std::mem::take(&mut state.popups.capture);
        Panel::new("capture_popup", anchors[0], capture_settings::MIN_WIDTH).show(
            ui,
            &t,
            &mut st,
            |ui| capture_settings::content(ui, state),
        );
        state.popups.capture = st;
    }
    if state.popups.source.open {
        let mut st = std::mem::take(&mut state.popups.source);
        Panel::new("source_popup", anchors[1], source::MIN_WIDTH)
            .show(ui, &t, &mut st, |ui| source::content(ui, state));
        state.popups.source = st;
    }
    if state.popups.game.open {
        let mut st = std::mem::take(&mut state.popups.game);
        Panel::new("game_popup", anchors[2], game_detection::MIN_WIDTH).show(
            ui,
            &t,
            &mut st,
            |ui| game_detection::content(ui, state),
        );
        state.popups.game = st;
    }
    if state.popups.hotkeys.open {
        let mut st = std::mem::take(&mut state.popups.hotkeys);
        Panel::new("hotkey_popup", anchors[3], hotkeys::MIN_WIDTH)
            .show(ui, &t, &mut st, |ui| hotkeys::content(ui, state));
        state.popups.hotkeys = st;
        if !state.popups.hotkeys.open {
            state.hotkey_recording = None;
        }
    }
}

/// Lays `label` (92 px, 12 px 600 TEXT) and a control in one row with 16 px
/// spacing; the label cell is as tall as the control.
pub(crate) fn labelled_row(
    ui: &mut Ui,
    state: &mut AppState,
    label: &str,
    label_w: f32,
    control_h: f32,
    control: impl FnOnce(&mut Ui, &mut AppState),
) {
    let t = state.tokens.clone();
    let origin = ui.cursor().min;
    let w = ui.available_width();
    let label_rect = egui::Rect::from_min_size(origin, egui::Vec2::new(label_w, control_h));
    crate::widgets::section::label_in(
        ui,
        &t,
        label_rect,
        label,
        crate::tokens::size::MICRO,
        crate::fonts::Weight::W600,
        crate::tokens::Color::Text,
        None,
    );
    let control_rect = egui::Rect::from_min_size(
        egui::Pos2::new(origin.x + label_w + 16.0, origin.y),
        egui::Vec2::new(w - label_w - 16.0, control_h),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(control_rect));
    control(&mut child, state);
    ui.allocate_rect(
        egui::Rect::from_min_size(origin, egui::Vec2::new(w, control_h)),
        egui::Sense::hover(),
    );
}
