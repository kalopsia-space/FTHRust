//! Page bodies. The clips page is plan 5; the settings page body is filled
//! by the `settings` module (Task 12).

use egui::Ui;

use crate::state::AppState;
use crate::tokens::Color;

/// Clips page placeholder (plan 5 replaces it with the library).
pub fn clips_placeholder(ui: &mut Ui, state: &mut AppState) {
    let rect = ui.max_rect();
    ui.painter()
        .rect_filled(rect, 0.0, state.tokens.color(Color::Bg));
    crate::fidelity::mark(ui, "body", rect)
        .fill(Color::Bg)
        .done();
}

/// Settings page.
pub fn settings_page(ui: &mut Ui, state: &mut AppState) {
    let rect = ui.max_rect();
    ui.painter()
        .rect_filled(rect, 0.0, state.tokens.color(Color::Bg));
    crate::fidelity::mark(ui, "settingsPage", rect)
        .fill(Color::Bg)
        .done();
}
