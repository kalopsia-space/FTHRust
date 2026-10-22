//! The custom widget set. Every widget draws itself from token values
//! following the QSS recipes in `ui/style.py` and registers a fidelity
//! marker under the Qt object name its caller gives.

pub mod accordion;
pub mod buttons;
pub mod check;
pub mod combo;
pub mod dialog;
pub mod input;
pub mod popup;
pub mod scroll;
pub mod section;
pub mod slider;
pub mod toggle;

use std::sync::Arc;

use egui::{Align, Color32, Galley, Pos2, Rect, Ui, Vec2};

use crate::fonts::{self, Weight};

/// Lays out `text` without wrapping.
pub fn galley(ui: &Ui, text: &str, px: f32, weight: Weight, color: Color32) -> Arc<Galley> {
    ui.painter()
        .layout_no_wrap(text.to_owned(), fonts::id(px, weight), color)
}

/// Linear colour blend in sRGB (Qt's pixmap crossfade).
pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        ch(a.r(), b.r()),
        ch(a.g(), b.g()),
        ch(a.b(), b.b()),
        ch(a.a(), b.a()),
    )
}

/// Paints `galley` vertically centred in `rect`, aligned horizontally per `align`
/// with `pad_x` inset, and returns the text rect.
pub fn paint_text(
    ui: &Ui,
    galley: Arc<Galley>,
    rect: Rect,
    align: Align,
    pad_x: f32,
    color: Color32,
) -> Rect {
    let size = galley.size();
    let x = match align {
        Align::Min => rect.min.x + pad_x,
        Align::Center => rect.center().x - size.x / 2.0,
        Align::Max => rect.max.x - pad_x - size.x,
    };
    let y = rect.center().y - size.y / 2.0;
    let pos = Pos2::new(x.round(), y.round());
    ui.painter().galley(pos, galley, color);
    Rect::from_min_size(pos, size)
}

/// Qt's `QFontMetrics::height()` for a role: the row height egui lays out.
pub fn text_height(ui: &Ui, px: f32, weight: Weight) -> f32 {
    galley(ui, "Hg", px, weight, Color32::WHITE).size().y
}

/// Qt's `QLabel` height for word-wrapped text laid out at `width`
/// (`QLabelPrivate::sizeForWidth`): the size hint wraps at 80 average
/// characters, re-wraps at half and a quarter of that when the text is
/// short, and the layout keeps that hint as the minimum; a wrapped label
/// is `rows × (line height + 1)` tall. Returns `(rows, height)`.
pub fn qt_wrapped_label_height(
    ui: &Ui,
    text: &str,
    px: f32,
    weight: Weight,
    width: f32,
) -> (usize, f32) {
    let line = text_height(ui, px, weight).round();
    let rows_at = |w: f32| -> (usize, f32) {
        let g = ui.painter().layout(
            text.to_owned(),
            fonts::id(px, weight),
            Color32::WHITE,
            w.max(1.0),
        );
        (g.rows.len().max(1), g.size().x)
    };
    let trial = fonts::avg_char_width(px, weight) * 80.0;
    let (mut rows, mut w_used) = rows_at(trial);
    if (rows as f32) * line < 4.0 * line && w_used > trial / 2.0 {
        (rows, w_used) = rows_at(trial / 2.0);
    }
    if (rows as f32) * line < 2.0 * line && w_used > trial / 4.0 {
        (rows, _) = rows_at(trial / 4.0);
    }
    let (actual, _) = rows_at(width);
    let rows = rows.max(actual);
    (rows, rows as f32 * (line + 1.0))
}

/// A crisp 1 px horizontal rule on pixel row `y` from `x0` to `x1`
/// (egui's `hline` is centred on the coordinate and lands between rows).
pub fn rule_h(ui: &Ui, x0: f32, x1: f32, y: f32, color: Color32) {
    ui.painter().rect_filled(
        Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + 1.0)),
        0.0,
        color,
    );
}

/// A crisp 1 px vertical rule on pixel column `x` from `y0` to `y1`.
pub fn rule_v(ui: &Ui, x: f32, y0: f32, y1: f32, color: Color32) {
    ui.painter().rect_filled(
        Rect::from_min_max(Pos2::new(x, y0), Pos2::new(x + 1.0, y1)),
        0.0,
        color,
    );
}

/// Allocates `size` at the cursor with click sensing.
pub fn allocate(ui: &mut Ui, size: Vec2, sense: egui::Sense) -> (Rect, egui::Response) {
    ui.allocate_exact_size(size, sense)
}
