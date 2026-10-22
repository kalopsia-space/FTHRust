//! `_ColorSwatch`: a 96×52 tile (30 px rounded colour with a 1 px BORDER_HI
//! inset by 1, the label in 7 pt TEXT_DIM below) that opens a colour picker.

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::fidelity;
use crate::fonts::Weight;
use crate::tokens::{Color, Tokens, geom, parse_hex, to_hex};

/// Swatch size.
pub const SIZE: Vec2 = Vec2::new(96.0, 52.0);

/// Draws a swatch; returns the new hex when the picker changed it.
pub fn swatch(
    ui: &mut Ui,
    t: &Tokens,
    token: &str,
    label: &str,
    hex: &str,
    name: &str,
) -> Option<String> {
    let (rect, resp) = ui.allocate_exact_size(SIZE, Sense::click());
    let color = parse_hex(hex).unwrap_or(Color32::BLACK);
    let tile = Rect::from_min_size(
        rect.min + Vec2::new(1.0, 1.0),
        Vec2::new(SIZE.x - 2.0, 28.0),
    );
    let radius = CornerRadius::same(geom::RADIUS as u8);
    ui.painter().rect_filled(tile, radius, color);
    ui.painter().rect_stroke(
        tile,
        radius,
        Stroke::new(1.0, t.color(Color::BorderHi)),
        StrokeKind::Middle,
    );
    let g = ui.painter().layout_no_wrap(
        label.to_owned(),
        crate::fonts::id(9.0, Weight::W400),
        t.color(Color::TextDim),
    );
    ui.painter().galley(
        Pos2::new(rect.min.x, rect.min.y + 33.0),
        g,
        t.color(Color::TextDim),
    );
    fidelity::mark(ui, name, rect).done();
    let popup_id = ui.scope_id().with(("swatch-picker", token));
    let mut open: bool = ui.ctx().data_mut(|d| d.get_temp(popup_id)).unwrap_or(false);
    if resp.clicked() {
        open = !open;
    }
    let mut changed = None;
    if open {
        let mut current = color;
        let area = egui::Area::new(popup_id.with("area"))
            .order(egui::Order::Foreground)
            .fixed_pos(Pos2::new(rect.min.x, rect.max.y + 4.0))
            .constrain(true)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(t.color(Color::Surface2))
                    .stroke(Stroke::new(1.0, t.color(Color::BorderHi)))
                    .inner_margin(12.0)
                    .corner_radius(radius)
                    .show(ui, |ui| {
                        ui.set_width(240.0);
                        if egui::color_picker::color_picker_color32(
                            ui,
                            &mut current,
                            egui::color_picker::Alpha::Opaque,
                        ) {
                            changed = Some(to_hex(current));
                        }
                    })
            });
        let prect = area.response.rect;
        let pressed_outside = ui.ctx().input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !prect.contains(p) && !rect.contains(p))
        });
        if pressed_outside || ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(popup_id, open));
    changed
}

/// A small clickable colour rectangle (icon tint swatches): `size`, fill
/// `hex`, 1 px BORDER_HI; returns the new hex when changed.
pub fn mini_swatch(ui: &mut Ui, t: &Tokens, size: Vec2, hex: &str, name: &str) -> Option<String> {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let color = parse_hex(hex).unwrap_or(Color32::BLACK);
    ui.painter().rect_filled(rect, 0.0, color);
    ui.painter().rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, t.color(Color::BorderHi)),
        StrokeKind::Inside,
    );
    fidelity::mark(ui, name, rect).edge(Color::BorderHi).done();
    let popup_id = ui.scope_id().with(("mini-picker", name));
    let mut open: bool = ui.ctx().data_mut(|d| d.get_temp(popup_id)).unwrap_or(false);
    if resp.clicked() {
        open = !open;
    }
    let mut changed = None;
    if open {
        let mut current = color;
        let area = egui::Area::new(popup_id.with("area"))
            .order(egui::Order::Foreground)
            .fixed_pos(Pos2::new(rect.min.x, rect.max.y + 4.0))
            .constrain(true)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(t.color(Color::Surface2))
                    .stroke(Stroke::new(1.0, t.color(Color::BorderHi)))
                    .inner_margin(12.0)
                    .corner_radius(CornerRadius::same(geom::RADIUS as u8))
                    .show(ui, |ui| {
                        ui.set_width(240.0);
                        if egui::color_picker::color_picker_color32(
                            ui,
                            &mut current,
                            egui::color_picker::Alpha::Opaque,
                        ) {
                            changed = Some(to_hex(current));
                        }
                    })
            });
        let prect = area.response.rect;
        let pressed_outside = ui.ctx().input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !prect.contains(p) && !rect.contains(p))
        });
        if pressed_outside || ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(popup_id, open));
    changed
}
