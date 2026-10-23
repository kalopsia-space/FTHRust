//! Page bodies. The clips page is plan 5; the settings page lives in the
//! `settings` module.

use egui::Ui;

use crate::state::AppState;
use crate::tokens::Color;

/// Clips page placeholder (kept for tests that render an empty body).
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
    crate::settings::ui(ui, state);
}
