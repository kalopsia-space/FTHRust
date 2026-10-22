//! `_ToggleSwitch` (`main.py:1008-1051`).

use egui::{Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::fidelity;
use crate::tokens::{Color, Tokens};

/// 38×20 switch. Track inset (1, 2, w−2, h−4) r8: ACCENT_SOFT on / SURFACE_1
/// off; border ACCENT when on or focused else BORDER_HI; 12 px knob at x=5
/// (off) or w−17 (on), ACCENT / TEXT_DIM. Disabled: SURFACE_1, BORDER,
/// TEXT_MUTED.
pub fn switch(ui: &mut Ui, t: &Tokens, on: &mut bool, name: &str) -> Response {
    switch_with(ui, t, on, true, name)
}

/// [`switch`] with an enabled flag.
pub fn switch_with(ui: &mut Ui, t: &Tokens, on: &mut bool, enabled: bool, name: &str) -> Response {
    let size = Vec2::new(38.0, 20.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut resp) = ui.allocate_exact_size(size, sense);
    if enabled && resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let focused = resp.has_focus();
    let (track, border, knob) = if enabled {
        (
            if *on {
                Color::AccentSoft
            } else {
                Color::Surface1
            },
            if *on || focused {
                Color::Accent
            } else {
                Color::BorderHi
            },
            if *on { Color::Accent } else { Color::TextDim },
        )
    } else {
        (Color::Surface1, Color::Border, Color::TextMuted)
    };
    let track_rect = Rect::from_min_size(
        rect.min + Vec2::new(1.0, 2.0),
        Vec2::new(size.x - 2.0, size.y - 4.0),
    );
    ui.painter()
        .rect_filled(track_rect, CornerRadius::same(8), t.color(track));
    ui.painter().rect_stroke(
        track_rect,
        CornerRadius::same(8),
        Stroke::new(1.0, t.color(border)),
        StrokeKind::Inside,
    );
    let d = 12.0;
    let x = if *on { size.x - d - 5.0 } else { 5.0 };
    let y = ((size.y - d) / 2.0).floor();
    let center = Pos2::new(rect.min.x + x + d / 2.0, rect.min.y + y + d / 2.0);
    ui.painter().circle_filled(center, d / 2.0, t.color(knob));
    let _ = Color32::TRANSPARENT;
    fidelity::mark(ui, name, rect)
        .sample("accent", center, knob)
        .sample(
            "fill",
            Pos2::new(rect.min.x + if *on { 9.0 } else { 28.0 }, rect.center().y),
            track,
        )
        .sample(
            "top",
            Pos2::new(rect.center().x.floor(), rect.min.y + 2.0),
            border,
        )
        .done();
    resp
}
