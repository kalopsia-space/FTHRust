//! `FthrDialog` (`ui/dialogs.py`): BG shell with a 1 px BORDER_HI r6 border,
//! a 38 px SURFACE_2 header (1 px BORDER bottom) with the title at x=12
//! (14 px 600) and a 30×30 close button 6 px from the right (hover ERROR),
//! a body with margins (16, 16, 16, 8) and 10 px spacing, and an action
//! row with margins (16, 8, 16, 14), 8 px spacing, right-aligned.

use egui::{Align, Area, CornerRadius, Id, Order, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2};

use super::buttons::{IconButtonStyle, icon_button};
use super::{galley, paint_text};
use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::{Icon, IconCache};
use crate::tokens::{Color, Tokens, geom, size};

/// Header height.
pub const HEADER_H: f32 = 38.0;

/// What the frame reports back.
#[derive(Clone, Copy, Debug)]
pub struct FrameResponse {
    /// The dialog rect.
    pub rect: Rect,
    /// The header close button was clicked.
    pub close_clicked: bool,
}

impl Default for FrameResponse {
    fn default() -> Self {
        FrameResponse {
            rect: Rect::NOTHING,
            close_clicked: false,
        }
    }
}

/// A dialog frame.
pub struct Frame<'a> {
    title: &'a str,
    width: f32,
}

impl<'a> Frame<'a> {
    /// A frame titled `title` with Qt's `width` (minimum width).
    pub fn new(title: &'a str, width: f32) -> Frame<'a> {
        Frame { title, width }
    }

    /// Lays the dialog out at `pos` inside `ui` (the harness renders dialogs
    /// at the origin; the app centres them with [`Frame::show_centered`]).
    pub fn show_at(
        self,
        ui: &mut Ui,
        t: &Tokens,
        icons: &mut IconCache,
        pos: Pos2,
        body: impl FnOnce(&mut Ui),
        actions: impl FnOnce(&mut Ui),
    ) -> FrameResponse {
        let w = self.width;
        let bg_idx = ui.painter().add(egui::Shape::Noop);
        let header_idx = ui.painter().add(egui::Shape::Noop);
        let header_rect = Rect::from_min_size(pos, Vec2::new(w, HEADER_H));
        let radius = CornerRadius::same(geom::RADIUS as u8);
        ui.painter().set(
            header_idx,
            egui::Shape::rect_filled(header_rect, radius, t.color(Color::Surface2)),
        );
        super::rule_h(
            ui,
            header_rect.min.x,
            header_rect.max.x,
            header_rect.max.y - 1.0,
            t.color(Color::Border),
        );
        fidelity::mark(ui, "fthrDialogHeader", header_rect)
            .sample(
                "fill",
                Pos2::new(header_rect.center().x, header_rect.min.y + 10.0),
                Color::Surface2,
            )
            .done();
        let g = galley(
            ui,
            self.title,
            size::LABEL,
            Weight::W600,
            t.color(Color::Text),
        );
        let title_rect = Rect::from_min_size(
            Pos2::new(pos.x + 12.0, pos.y),
            Vec2::new(g.size().x.ceil(), HEADER_H - 1.0),
        );
        paint_text(ui, g, title_rect, Align::Min, 0.0, t.color(Color::Text));
        fidelity::mark(ui, "fthrDialogTitle", title_rect)
            .text(self.title, size::LABEL, Weight::W600)
            .done();
        let close_rect = Rect::from_min_size(
            Pos2::new(pos.x + w - 6.0 - 30.0, pos.y + 3.0),
            Vec2::splat(30.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(close_rect));
        let close = icon_button(
            &mut child,
            t,
            icons,
            Icon::Close,
            14.0,
            IconButtonStyle {
                size: Vec2::splat(30.0),
                bg: None,
                hover_bg: Some(Color::Error),
                pressed_bg: Some(Color::Error),
                border: None,
                hover_border: None,
                radius: geom::RADIUS,
                hover_tint: Some(Color::Text),
                enabled: true,
            },
            "fthrDialogClose",
        );
        // Body.
        let body_top = header_rect.max.y + 16.0;
        let body_inner = ui.scope_builder(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(
                Pos2::new(pos.x + 16.0, body_top),
                Vec2::new(w - 32.0, f32::INFINITY),
            )),
            |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(0.0, 10.0);
                body(ui);
            },
        );
        let body_bottom = body_inner.response.rect.max.y + 8.0;
        // Action row, right-aligned.
        let actions_top = body_bottom + 8.0;
        let act_inner = ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(
                    Pos2::new(pos.x + 16.0, actions_top),
                    Vec2::new(w - 32.0, f32::INFINITY),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Min)),
            |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
                actions(ui);
            },
        );
        let bottom = act_inner.response.rect.max.y + 14.0;
        let rect = Rect::from_min_max(pos, Pos2::new(pos.x + w, bottom));
        ui.painter().set(
            bg_idx,
            egui::Shape::rect_filled(rect, radius, t.color(Color::Bg)),
        );
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, t.color(Color::BorderHi)),
            StrokeKind::Inside,
        );
        ui.allocate_rect(rect, egui::Sense::hover());
        fidelity::mark(ui, "fthrDialog", rect)
            .sample(
                "fill",
                Pos2::new(rect.min.x + 5.0, rect.center().y),
                Color::Bg,
            )
            .edge(Color::BorderHi)
            .done();
        FrameResponse {
            rect,
            close_clicked: close.clicked(),
        }
    }

    /// Shows the dialog centred over the window, above everything, and
    /// swallows pointer input behind it (Qt modal).
    pub fn show_centered(
        self,
        ctx: &egui::Context,
        t: &Tokens,
        icons: &mut IconCache,
        modal: bool,
        body: impl FnOnce(&mut Ui),
        actions: impl FnOnce(&mut Ui),
    ) -> FrameResponse {
        let id = Id::unique(("fthrust-dialog", self.title));
        let screen = ctx.content_rect();
        let last: Option<Vec2> = ctx.data_mut(|d| d.get_temp(id.with("size")));
        let size = last.unwrap_or(Vec2::new(self.width, 160.0));
        let drag: Vec2 = ctx
            .data_mut(|d| d.get_temp(id.with("drag")))
            .unwrap_or(Vec2::ZERO);
        let pos = (screen.center() - size / 2.0 + drag).round();
        if modal {
            Area::new(id.with("blocker"))
                .order(Order::Foreground)
                .fixed_pos(screen.min)
                .constrain(false)
                .interactable(true)
                .show(ctx, |ui| {
                    ui.allocate_rect(screen, egui::Sense::click_and_drag());
                });
        }
        let mut out = FrameResponse::default();
        Area::new(id)
            .order(Order::Tooltip)
            .fixed_pos(pos)
            .constrain(false)
            .interactable(true)
            .show(ctx, |ui| {
                out = self.show_at(ui, t, icons, pos, body, actions);
                let header = Rect::from_min_size(pos, Vec2::new(out.rect.width(), HEADER_H));
                let r = ui.interact(header, id.with("header-drag"), egui::Sense::drag());
                if r.dragged() {
                    ctx.data_mut(|d| d.insert_temp(id.with("drag"), drag + r.drag_delta()));
                }
            });
        if last != Some(out.rect.size()) {
            ctx.data_mut(|d| d.insert_temp(id.with("size"), out.rect.size()));
            ctx.request_repaint();
        }
        out
    }
}

/// Word-wrapped body copy, 17 px 400 (`FthrMessageDialog`).
pub fn message_body(ui: &mut Ui, t: &Tokens, message: &str, width: f32) -> Rect {
    let g = ui.painter().layout(
        message.to_owned(),
        crate::fonts::id(size::BODY_L, Weight::W400),
        t.color(Color::Text),
        width,
    );
    let (_, h) = super::qt_wrapped_label_height(ui, message, size::BODY_L, Weight::W400, width);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, h), egui::Sense::hover());
    ui.painter().galley(
        Pos2::new(rect.min.x, rect.min.y + ((h - g.size().y) / 2.0).floor()),
        g,
        t.color(Color::Text),
    );
    fidelity::mark(
        ui,
        format!("QLabel[{}]", &message[..message.len().min(40)]),
        rect,
    )
    .text(message, size::BODY_L, Weight::W400)
    .done();
    rect
}

/// Dialog input field (`fthrDialogInput`): SURFACE_1, 1 px BORDER_HI (ACCENT
/// focused), r6, padding 7×9, 15 px; 40 px tall.
pub fn input_field(
    ui: &mut Ui,
    t: &Tokens,
    text: &mut String,
    width: f32,
    name: &str,
) -> egui::Response {
    let h = 40.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, h), egui::Sense::hover());
    let radius = CornerRadius::same(geom::RADIUS as u8);
    ui.painter()
        .rect_filled(rect, radius, t.color(Color::Surface1));
    let inner = Rect::from_min_max(
        rect.min + Vec2::new(10.0, 1.0),
        rect.max - Vec2::new(10.0, 1.0),
    );
    let edit = egui::TextEdit::singleline(text)
        .font(crate::fonts::id(size::BODY, Weight::W400))
        .text_color(t.color(Color::Text))
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .align(egui::Align2::LEFT_CENTER)
        .desired_width(inner.width());
    let resp = ui.put(inner, edit).accessible_name(name);
    let border = if resp.has_focus() {
        Color::Accent
    } else {
        Color::BorderHi
    };
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, t.color(border)),
        StrokeKind::Inside,
    );
    fidelity::mark(ui, name, rect)
        .sample(
            "fill",
            Pos2::new(rect.min.x + 4.0, rect.center().y),
            Color::Surface1,
        )
        .text(text.as_str(), size::BODY, Weight::W400)
        .done();
    resp
}
