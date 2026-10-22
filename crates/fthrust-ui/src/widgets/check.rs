//! `checkbox_qss` / `radiobutton_qss`: 14×14 indicator, 1 px TEXT border,
//! BG fill, r4 (radio r7), hover ACCENT border, checked ACCENT; label 17 px
//! with 10 px spacing; the row is 26 px tall.

use egui::{
    Align, Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use super::{galley, paint_text};
use crate::fidelity;
use crate::fonts::Weight;
use crate::tokens::{Color, Tokens, size};

const ROW_H: f32 = 26.0;
const BOX: f32 = 14.0;
const SPACING: f32 = 10.0;

#[allow(clippy::too_many_arguments)]
fn indicator(
    ui: &mut Ui,
    t: &Tokens,
    checked: bool,
    label: &str,
    radius: f32,
    enabled: bool,
    name: &str,
    expand: bool,
) -> (Response, bool) {
    let g = galley(ui, label, size::BODY_L, Weight::W400, Color32::WHITE);
    let text_w = g.size().x;
    let w = if expand {
        ui.available_width()
    } else {
        (BOX + SPACING + text_w).ceil()
    };
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), sense);
    let hovered = enabled && resp.hovered();
    let box_rect = Rect::from_min_size(
        Pos2::new(rect.min.x, rect.min.y + ((ROW_H - BOX) / 2.0).floor()),
        Vec2::splat(BOX),
    );
    let (fill, border) = if checked {
        (Color::Accent, Color::Accent)
    } else if hovered {
        (Color::Bg, Color::Accent)
    } else {
        (Color::Bg, Color::Text)
    };
    let cr = CornerRadius::same(radius as u8);
    ui.painter().rect_filled(box_rect, cr, t.color(fill));
    ui.painter().rect_stroke(
        box_rect,
        cr,
        Stroke::new(1.0, t.color(border)),
        StrokeKind::Inside,
    );
    let text_color = if enabled {
        t.color(Color::Text)
    } else {
        t.color(Color::TextMuted)
    };
    let text_rect = Rect::from_min_max(Pos2::new(rect.min.x + BOX + SPACING, rect.min.y), rect.max);
    paint_text(ui, g, text_rect, Align::Min, 0.0, text_color);
    let mut marker = fidelity::mark(ui, name, rect)
        .text(label, size::BODY_L, Weight::W400)
        .sample("accent", box_rect.center(), fill);
    if radius < 7.0 {
        // A radio's r7 indicator is a circle: no straight edge to sample.
        marker = marker.sample(
            "edge",
            Pos2::new(box_rect.min.x, box_rect.center().y),
            border,
        );
    }
    marker.done();
    (resp, hovered)
}

/// A checkbox row; returns the response (`changed()` when toggled).
pub fn checkbox(ui: &mut Ui, t: &Tokens, on: &mut bool, label: &str, name: &str) -> Response {
    checkbox_with(ui, t, on, label, true, false, name)
}

/// [`checkbox`] with enabled and expand flags.
pub fn checkbox_with(
    ui: &mut Ui,
    t: &Tokens,
    on: &mut bool,
    label: &str,
    enabled: bool,
    expand: bool,
    name: &str,
) -> Response {
    let (mut resp, _) = indicator(ui, t, *on, label, 4.0, enabled, name, expand);
    if enabled && resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp
}

/// A radio row; `clicked()` means "select me".
pub fn radio(ui: &mut Ui, t: &Tokens, selected: bool, label: &str, name: &str) -> Response {
    indicator(ui, t, selected, label, 7.0, true, name, false).0
}
