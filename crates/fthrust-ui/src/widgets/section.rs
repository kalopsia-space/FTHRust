//! Section furniture: `_flat_section_header`, `_settings_hsep`,
//! `label_uppercase`, `label_body`, `label_display`.

use egui::{Align, Color32, Pos2, Rect, Response, Sense, Ui, Vec2};

use super::{galley, paint_text};
use crate::fidelity;
use crate::fonts::{self, Weight};
use crate::tokens::{Color, Tokens, geom, size};

/// A label in `color`, `px`, `weight`; sized to its text (Qt `QLabel`).
/// The marker is named `QLabel[<text>]` unless `name` is given.
pub fn label(
    ui: &mut Ui,
    t: &Tokens,
    text: &str,
    px: f32,
    weight: Weight,
    color: Color,
    name: Option<&str>,
) -> Response {
    let g = galley(ui, text, px, weight, t.color(color));
    let size = Vec2::new(
        g.size().x.round(),
        fonts::qt_wrapped_line_height(px, weight),
    );
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    paint_text(ui, g, rect, Align::Min, 0.0, t.color(color));
    let name = name
        .map(str::to_owned)
        .unwrap_or_else(|| format!("QLabel[{}]", crate::fidelity::marker_label(text)));
    fidelity::mark(ui, name, rect).text(text, px, weight).done();
    resp
}

/// A label in a fixed-size cell (Qt layout-stretched label): text left,
/// vertically centred.
#[allow(clippy::too_many_arguments)]
pub fn label_in(
    ui: &mut Ui,
    t: &Tokens,
    rect: Rect,
    text: &str,
    px: f32,
    weight: Weight,
    color: Color,
    name: Option<&str>,
) {
    let g = galley(ui, text, px, weight, t.color(color));
    paint_text(ui, g, rect, Align::Min, 0.0, t.color(color));
    let name = name
        .map(str::to_owned)
        .unwrap_or_else(|| format!("QLabel[{}]", crate::fidelity::marker_label(text)));
    fidelity::mark(ui, name, rect).text(text, px, weight).done();
}

/// A word-wrapped label of `width` (Qt `setWordWrap(True)`), optionally
/// with QSS `padding-top`; the height follows Qt's wrapped-label rule.
#[allow(clippy::too_many_arguments)]
pub fn label_wrapped(
    ui: &mut Ui,
    t: &Tokens,
    text: &str,
    px: f32,
    weight: Weight,
    color: Color,
    width: f32,
    padding_top: f32,
) -> Rect {
    let (_, text_h) = super::qt_wrapped_label_height(ui, text, px, weight, width);
    let h = text_h + padding_top;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
    let g = ui.painter().layout(
        text.to_owned(),
        fonts::id(px, weight),
        t.color(color),
        width,
    );
    let y = rect.min.y + padding_top + ((text_h - g.size().y) / 2.0).floor().max(0.0);
    ui.painter()
        .galley(Pos2::new(rect.min.x, y), g, t.color(color));
    fidelity::mark(
        ui,
        format!("QLabel[{}]", crate::fidelity::marker_label(text)),
        rect,
    )
    .text(text, px, weight)
    .done();
    rect
}

/// A word-wrapped label laid out at its real `width` (labels inside
/// `widgetResizable` scroll pages: Qt uses `heightForWidth`, not the
/// trial-width hint).
pub fn label_wrapped_fit(
    ui: &mut Ui,
    t: &Tokens,
    text: &str,
    px: f32,
    weight: Weight,
    color: Color,
    width: f32,
) -> Rect {
    let name = format!("QLabel[{}]", crate::fidelity::marker_label(text));
    wrapped_fit(ui, t, text, px, weight, color, width, name)
}

/// A one-line `label` while the text fits the space left in the row, else
/// word-wrapped to that space (status lines that can grow long).
pub fn label_fit(
    ui: &mut Ui,
    t: &Tokens,
    text: &str,
    px: f32,
    weight: Weight,
    color: Color,
    name: &str,
) -> Rect {
    let width = ui.available_width();
    if galley(ui, text, px, weight, t.color(color))
        .size()
        .x
        .round()
        <= width
    {
        return label(ui, t, text, px, weight, color, Some(name)).rect;
    }
    wrapped_fit(ui, t, text, px, weight, color, width, name.to_owned())
}

#[allow(clippy::too_many_arguments)]
fn wrapped_fit(
    ui: &mut Ui,
    t: &Tokens,
    text: &str,
    px: f32,
    weight: Weight,
    color: Color,
    width: f32,
    name: String,
) -> Rect {
    let line = fonts::qt_wrapped_line_height(px, weight);
    let g = ui.painter().layout(
        text.to_owned(),
        fonts::id(px, weight),
        t.color(color),
        width,
    );
    let rows = g.rows.len().max(1) as f32;
    let h = rows * line;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
    let y = rect.min.y + ((h - g.size().y) / 2.0).floor().max(0.0);
    ui.painter()
        .galley(Pos2::new(rect.min.x, y), g, t.color(color));
    fidelity::mark(ui, name, rect).text(text, px, weight).done();
    rect
}

/// A label that takes the available width (Qt expanding label), text left.
pub fn label_wide(
    ui: &mut Ui,
    t: &Tokens,
    text: &str,
    px: f32,
    weight: Weight,
    color: Color,
    name: Option<&str>,
) -> Response {
    let g = galley(ui, text, px, weight, t.color(color));
    let size = Vec2::new(
        ui.available_width(),
        fonts::qt_wrapped_line_height(px, weight),
    );
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    paint_text(ui, g, rect, Align::Min, 0.0, t.color(color));
    let name = name
        .map(str::to_owned)
        .unwrap_or_else(|| format!("QLabel[{}]", crate::fidelity::marker_label(text)));
    fidelity::mark(ui, name, rect).text(text, px, weight).done();
    resp
}

/// Body text, 15 px 400 (`label_body`).
pub fn label_body(ui: &mut Ui, t: &Tokens, text: &str, color: Color) -> Response {
    label(ui, t, text, size::BODY, Weight::W400, color, None)
}

/// Compact semibold label, 14 px 600 (`label_uppercase`).
pub fn label_uppercase(ui: &mut Ui, t: &Tokens, text: &str, color: Color) -> Response {
    label(ui, t, text, size::LABEL, Weight::W600, color, None)
}

/// Display heading, 600 at `px` (`label_display`).
pub fn label_display(ui: &mut Ui, t: &Tokens, text: &str, px: f32, color: Color) -> Response {
    label(ui, t, text, px, Weight::W600, color, None)
}

/// Default 17 px label (Qt app font `SIZE_BODY_L`, used for setting rows).
pub fn label_row(ui: &mut Ui, t: &Tokens, text: &str) -> Response {
    label(ui, t, text, size::BODY_L, Weight::W400, Color::Text, None)
}

/// `_flat_section_header`: H3 20 px 600 title, 12 px gap, 1 px SHELL_DIVIDER
/// rule to the right edge.
pub fn flat_header(ui: &mut Ui, t: &Tokens, title: &str, name: &str) -> Rect {
    let g = galley(ui, title, size::H3, Weight::W600, t.color(Color::Text));
    let h = g.size().y.round();
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let text_rect = paint_text(
        ui,
        g.clone(),
        Rect::from_min_size(rect.min, Vec2::new(g.size().x.ceil(), h)),
        Align::Min,
        0.0,
        t.color(Color::Text),
    );
    let rule_y = rect.min.y + (h / 2.0).floor();
    let rule_x0 = text_rect.max.x.ceil() + geom::ROW_GAP;
    super::rule_h(
        ui,
        rule_x0,
        rect.max.x,
        rule_y,
        t.color(Color::ShellDivider),
    );
    fidelity::mark(
        ui,
        format!("QLabel[{name}]"),
        Rect::from_min_size(rect.min, Vec2::new(g.size().x.ceil(), h)),
    )
    .text(title, size::H3, Weight::W600)
    .done();
    fidelity::mark(
        ui,
        format!("{name}/rule"),
        Rect::from_min_max(
            Pos2::new(rule_x0, rule_y),
            Pos2::new(rect.max.x, rule_y + 1.0),
        ),
    )
    .sample(
        "fill",
        Pos2::new((rule_x0 + rect.max.x) / 2.0, rule_y),
        Color::ShellDivider,
    )
    .done();
    rect
}

/// `_settings_hsep`: 1 px SHELL_DIVIDER line across the width.
pub fn hsep(ui: &mut Ui, t: &Tokens, name: &str) -> Rect {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, t.color(Color::ShellDivider));
    fidelity::mark(ui, name, rect)
        .sample(
            "fill",
            Pos2::new(rect.center().x, rect.min.y),
            Color::ShellDivider,
        )
        .done();
    rect
}

/// A 1 px vertical SHELL_DIVIDER line of `height`.
pub fn vsep(ui: &mut Ui, t: &Tokens, height: f32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, height), Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, t.color(Color::ShellDivider));
    rect
}

/// Fills `rect` with a colour (panel backgrounds).
pub fn fill(ui: &Ui, rect: Rect, color: Color32) {
    ui.painter().rect_filled(rect, 0.0, color);
}
