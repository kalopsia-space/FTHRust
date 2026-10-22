//! The custom widget set. Every widget draws itself from token values
//! following the QSS recipes in `ui/style.py` and registers a fidelity
//! marker under the Qt object name its caller gives.

pub mod buttons;
pub mod check;
pub mod input;
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

/// Allocates `size` at the cursor with click sensing.
pub fn allocate(ui: &mut Ui, size: Vec2, sense: egui::Sense) -> (Rect, egui::Response) {
    ui.allocate_exact_size(size, sense)
}
