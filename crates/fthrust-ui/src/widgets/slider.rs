//! `slider_qss` with `_SettingsSlider` / `ClickableSlider` behaviour: a
//! 2 px groove (BORDER, ACCENT sub-page), a 12×12 r4 handle (ACCENT; TEXT
//! on hover/press), a 26 px hit area and absolute click-to-set.

use std::ops::RangeInclusive;

use egui::{CornerRadius, Pos2, Rect, Response, Sense, Ui, Vec2};

use crate::fidelity;
use crate::tokens::{Color, Tokens, geom};

/// Settings variant: 12 px r4 handle, 26 px tall.
pub fn settings_slider(
    ui: &mut Ui,
    t: &Tokens,
    value: &mut f32,
    range: RangeInclusive<f32>,
    name: &str,
) -> Response {
    slider(ui, t, value, range, 12.0, geom::RADIUS_SM, 26.0, name)
}

/// Editor variant: 10 px r6 handle.
pub fn editor_slider(
    ui: &mut Ui,
    t: &Tokens,
    value: &mut f32,
    range: RangeInclusive<f32>,
    name: &str,
) -> Response {
    slider(ui, t, value, range, 10.0, geom::RADIUS, 26.0, name)
}

#[allow(clippy::too_many_arguments)]
fn slider(
    ui: &mut Ui,
    t: &Tokens,
    value: &mut f32,
    range: RangeInclusive<f32>,
    handle: f32,
    radius: f32,
    hit_h: f32,
    name: &str,
) -> Response {
    let w = ui.available_width();
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, hit_h), Sense::click_and_drag());
    let (lo, hi) = (*range.start(), *range.end());
    let span = (rect.width() - handle).max(1.0);
    if (resp.is_pointer_button_down_on() || resp.dragged() || resp.clicked())
        && let Some(p) = ui.ctx().pointer_interact_pos()
    {
        let x = (p.x - rect.min.x - handle / 2.0).clamp(0.0, span);
        let v = lo + (x / span) * (hi - lo);
        let v = if (hi - lo) >= 2.0 { v.round() } else { v };
        if v != *value {
            *value = v;
            resp.mark_changed();
        }
    }
    let frac = if hi > lo {
        ((*value - lo) / (hi - lo)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let cy = rect.min.y + (hit_h / 2.0).floor();
    let groove = Rect::from_min_max(
        Pos2::new(rect.min.x, cy - 1.0),
        Pos2::new(rect.max.x, cy + 1.0),
    );
    let hx = rect.min.x + (frac * span).round();
    ui.painter()
        .rect_filled(groove, CornerRadius::ZERO, t.color(Color::Border));
    ui.painter().rect_filled(
        Rect::from_min_max(groove.min, Pos2::new(hx + handle / 2.0, groove.max.y)),
        CornerRadius::ZERO,
        t.color(Color::Accent),
    );
    let active = resp.hovered() || resp.dragged() || resp.is_pointer_button_down_on();
    let handle_color = if active { Color::Text } else { Color::Accent };
    let hrect = Rect::from_min_size(Pos2::new(hx, cy - handle / 2.0), Vec2::splat(handle));
    ui.painter().rect_filled(
        hrect,
        CornerRadius::same(radius as u8),
        t.color(handle_color),
    );
    fidelity::mark(ui, name, rect)
        .sample("accent", hrect.center(), handle_color)
        .sample(
            "fill",
            Pos2::new(rect.min.x + 1.0, cy),
            if frac > 0.0 {
                Color::Accent
            } else {
                Color::Border
            },
        )
        .done();
    resp
}
