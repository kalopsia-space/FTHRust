//! `_PopupPanel`: SURFACE_2, 1 px BORDER_HI, r6, min width 300, placed
//! under its anchor by `fthrust_core::window_geometry::popup_anchor` and
//! kept 8 px inside the screen; 180 ms OutCubic fade except on Wayland;
//! an outside press or Escape closes it.

use egui::{Area, CornerRadius, Id, Order, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2};

use crate::fidelity;
use crate::motion::{Ease, Transition};
use crate::tokens::{Color, Tokens, geom, motion};

/// Open state plus the fade transition.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelState {
    /// Shown.
    pub open: bool,
    /// Set for one frame when the panel closed (outside click, Escape or
    /// a caller's `close()`), so the trigger button can drop its arrow.
    pub just_closed: bool,
    fade: Option<Transition>,
    was_open: bool,
    last_size: Option<Vec2>,
}

impl PanelState {
    /// Closes the panel.
    pub fn close(&mut self) {
        if self.open {
            self.open = false;
            self.just_closed = true;
        }
    }

    /// Opens the panel.
    pub fn show(&mut self) {
        self.open = true;
    }

    /// Toggles.
    pub fn toggle(&mut self) {
        if self.open { self.close() } else { self.show() }
    }
}

/// The result of showing a panel.
pub struct PanelResponse {
    /// The panel rect (empty when closed).
    pub rect: Rect,
}

/// Draws the popup shell (SURFACE_2, 1 px BORDER_HI, r6) at `pos` with
/// `width`, laying `content` out inside the border, and returns the rect.
/// Tests render popups through this at the origin.
pub fn draw_frame(
    ui: &mut Ui,
    t: &Tokens,
    name: &str,
    pos: Pos2,
    width: f32,
    content: impl FnOnce(&mut Ui),
) -> Rect {
    draw_frame_with_opacity(ui, t, name, pos, width, 1.0, content)
}

fn draw_frame_with_opacity(
    ui: &mut Ui,
    t: &Tokens,
    name: &str,
    pos: Pos2,
    width: f32,
    opacity: f32,
    content: impl FnOnce(&mut Ui),
) -> Rect {
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let border_idx = ui.painter().add(egui::Shape::Noop);
    let inner = ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(
            pos + Vec2::new(1.0, 1.0),
            Vec2::new(width - 2.0, f32::INFINITY),
        )),
        |ui| {
            ui.set_min_width(width - 2.0);
            content(ui);
        },
    );
    let bottom = inner.response.rect.max.y + 1.0;
    let rect = Rect::from_min_max(pos, Pos2::new(pos.x + width, bottom));
    let radius = CornerRadius::same(geom::RADIUS as u8);
    // `Painter::set` bypasses the ui opacity, so the shell fades by hand.
    ui.painter().set(
        bg_idx,
        egui::Shape::rect_filled(
            rect,
            radius,
            t.color(Color::Surface2).gamma_multiply(opacity),
        ),
    );
    ui.painter().set(
        border_idx,
        egui::Shape::rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, t.color(Color::BorderHi).gamma_multiply(opacity)),
            StrokeKind::Inside,
        ),
    );
    fidelity::mark(ui, name, rect)
        .sample(
            "fill",
            Pos2::new(rect.min.x + 5.0, rect.center().y),
            Color::Surface2,
        )
        .edge(Color::BorderHi)
        .done();
    ui.allocate_rect(rect, egui::Sense::hover());
    rect
}

/// A popup panel anchored to a widget rect.
pub struct Panel<'a> {
    name: &'a str,
    anchor: Rect,
    min_width: f32,
}

impl<'a> Panel<'a> {
    /// A panel named `name` under `anchor` with `min_width`.
    pub fn new(name: &'a str, anchor: Rect, min_width: f32) -> Panel<'a> {
        Panel {
            name,
            anchor,
            min_width: min_width.max(300.0),
        }
    }

    /// Shows the panel when `state.open`; `content` lays out its body.
    pub fn show(
        self,
        ui: &mut Ui,
        t: &Tokens,
        state: &mut PanelState,
        content: impl FnOnce(&mut Ui),
    ) -> PanelResponse {
        state.just_closed = false;
        let ctx = ui.ctx().clone();
        if !state.open {
            state.was_open = false;
            state.fade = None;
            return PanelResponse {
                rect: Rect::NOTHING,
            };
        }
        let now = ctx.input(|i| i.time);
        if !state.was_open {
            state.was_open = true;
            state.fade = (!t.wayland && !t.reduce_motion)
                .then(|| Transition::new(now, motion::POPUP_FADE_MS, Ease::OutCubic));
        }
        let opacity = state
            .fade
            .map(|f| f.progress(now, t.reduce_motion))
            .unwrap_or(1.0);
        if state.fade.is_some_and(|f| f.running(now, t.reduce_motion)) {
            ctx.request_repaint();
        }
        let screen = ctx.content_rect();
        let margin = geom::RESIZE_MARGIN;
        let size_guess = state.last_size.unwrap_or(Vec2::new(self.min_width, 120.0));
        let w = size_guess
            .x
            .max(self.min_width)
            .min((screen.width() - margin * 2.0).max(160.0));
        let h = size_guess
            .y
            .min((screen.height() - margin * 2.0).max(120.0));
        let anchor = fthrust_core::window_geometry::Rect {
            x: self.anchor.min.x as i32,
            y: self.anchor.min.y as i32,
            width: self.anchor.width() as u32,
            height: self.anchor.height() as u32,
        };
        let screen_r = fthrust_core::window_geometry::Rect {
            x: screen.min.x as i32,
            y: screen.min.y as i32,
            width: screen.width() as u32,
            height: screen.height() as u32,
        };
        let (px, py) =
            fthrust_core::window_geometry::popup_anchor(anchor, (w as u32, h as u32), screen_r);
        let mut pos = Pos2::new(px as f32, py as f32);
        // Final clamp with the 8 px margin (show_below's post-show nudge).
        if pos.x < screen.min.x + margin {
            pos.x = screen.min.x + margin;
        } else if pos.x + w > screen.max.x - margin {
            pos.x = screen.max.x - margin - w;
        }
        if pos.y < screen.min.y + margin {
            pos.y = screen.min.y + margin;
        } else if pos.y + h > screen.max.y - margin {
            pos.y = screen.max.y - margin - h;
        }
        let id = Id::unique(("fthrust-popup", self.name));
        let name = self.name.to_owned();
        let tokens = t.clone();
        let inner = Area::new(id)
            .order(Order::Foreground)
            .fixed_pos(pos)
            .constrain(false)
            .interactable(true)
            .show(&ctx, |ui| {
                ui.set_opacity(opacity);
                draw_frame_with_opacity(ui, &tokens, &name, pos, w, opacity, content)
            });
        let rect = inner.inner;
        let size = rect.size();
        if state.last_size != Some(size) {
            state.last_size = Some(size);
            ctx.request_repaint();
        }
        let pressed_outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !rect.contains(p) && !self.anchor.contains(p))
        });
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if pressed_outside || escape {
            state.close();
        }
        PanelResponse { rect }
    }
}
