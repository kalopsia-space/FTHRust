//! `_AccordionSection` (`ui/customize_page.py`): SURFACE_3 card with a 1 px
//! BORDER and r8, a 44 px header (margins 16, spacing 12: ACCENT number,
//! TEXT title, both 17 px 600, 14 px arrow at the right; hover BORDER_HI)
//! and a body with 16 px margins and 8 px spacing.

use egui::{Align, CornerRadius, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::buttons::dropdown_arrow;
use super::{galley, lerp, paint_text};
use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::IconCache;
use crate::motion;
use crate::tokens::{Color, Tokens, geom, size};

/// Header height.
pub const HEADER_H: f32 = geom::CAPTURE_CONTROL_H;
/// Body margin.
pub const BODY_MARGIN: f32 = 16.0;
/// Body item spacing.
pub const BODY_SPACING: f32 = 8.0;

/// Draws a collapsible section; `body` runs only when `open`.
#[allow(clippy::too_many_arguments)]
pub fn section(
    ui: &mut Ui,
    t: &Tokens,
    icons: &mut IconCache,
    number: &str,
    title: &str,
    open: &mut bool,
    name: &str,
    body: impl FnOnce(&mut Ui),
) -> Response {
    let w = ui.available_width();
    let start = ui.cursor().min;
    let header_rect = Rect::from_min_size(start, Vec2::new(w, HEADER_H));
    let radius = CornerRadius::same(geom::RADIUS_CARD as u8);
    // Paint the card background first; its height is known after the body.
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let border_idx = ui.painter().add(egui::Shape::Noop);
    let (hrect, resp) = ui.allocate_exact_size(header_rect.size(), Sense::click());
    if resp.clicked() {
        *open = !*open;
    }
    let blend = motion::hover(
        ui.ctx(),
        resp.id.with("hover"),
        resp.hovered(),
        t.reduce_motion,
    );
    let header_bg = lerp(t.color(Color::Surface3), t.color(Color::BorderHi), blend);
    ui.painter().rect_filled(hrect, radius, header_bg);
    let mut x = hrect.min.x + 16.0;
    if !number.is_empty() {
        let g = galley(
            ui,
            number,
            size::BODY_L,
            Weight::W600,
            t.color(Color::Accent),
        );
        let nrect = Rect::from_min_size(
            Pos2::new(x, hrect.min.y),
            Vec2::new(g.size().x.ceil(), hrect.height()),
        );
        paint_text(ui, g, nrect, Align::Min, 0.0, t.color(Color::Accent));
        fidelity::mark(ui, format!("{name}/number"), nrect)
            .text(number, size::BODY_L, Weight::W600)
            .done();
        x += nrect.width() + 12.0;
    }
    let g = galley(ui, title, size::BODY_L, Weight::W600, t.color(Color::Text));
    let trect = Rect::from_min_size(
        Pos2::new(x, hrect.min.y),
        Vec2::new(g.size().x.ceil(), hrect.height()),
    );
    paint_text(ui, g, trect, Align::Min, 0.0, t.color(Color::Text));
    fidelity::mark(ui, format!("{name}/title"), trect)
        .text(title, size::BODY_L, Weight::W600)
        .done();
    dropdown_arrow(ui, t, icons, hrect, *open, 14.0, 16.0);
    let mut total = Rect::from_min_size(start, Vec2::new(w, HEADER_H));
    if *open {
        let inner = ui.scope_builder(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(
                Pos2::new(start.x + BODY_MARGIN, hrect.max.y + BODY_MARGIN),
                Vec2::new(w - BODY_MARGIN * 2.0, f32::INFINITY),
            )),
            |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(0.0, BODY_SPACING);
                body(ui);
            },
        );
        let body_rect = inner.response.rect;
        let bottom = body_rect.max.y + BODY_MARGIN;
        ui.allocate_rect(
            Rect::from_min_max(hrect.left_bottom(), Pos2::new(hrect.max.x, bottom)),
            Sense::hover(),
        );
        total = Rect::from_min_max(start, Pos2::new(start.x + w, bottom));
    }
    ui.painter().set(
        bg_idx,
        egui::Shape::rect_filled(total, radius, t.color(Color::Surface3)),
    );
    ui.painter().set(
        border_idx,
        egui::Shape::rect_stroke(
            total,
            radius,
            Stroke::new(1.0, t.color(Color::Border)),
            StrokeKind::Inside,
        ),
    );
    // The header is painted above the card shapes; re-add the stroke on top so
    // the 1 px border stays visible over the header fill.
    ui.painter().rect_stroke(
        total,
        radius,
        Stroke::new(1.0, t.color(Color::Border)),
        StrokeKind::Inside,
    );
    fidelity::mark(ui, name, total)
        .sample(
            "fill",
            Pos2::new(total.center().x, hrect.center().y),
            Color::Surface3,
        )
        .edge(Color::Border)
        .done();
    resp
}
