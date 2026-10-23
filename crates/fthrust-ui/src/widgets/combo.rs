//! `combo_qss` + `WheelSafeComboBox`: SURFACE_3, 1 px BORDER, r6, padding
//! 6×12, 15 px text, 38 px tall, 14 px dropdown arrow 8 px from the right;
//! the list drops below with 26 px items (padding-left 10), selected item
//! SURFACE_3 + ACCENT text. The wheel never changes a closed combo.

use egui::{
    Align, Area, Color32, CornerRadius, Id, Order, Pos2, Rect, Response, Sense, Stroke, StrokeKind,
    Ui, Vec2,
};

use super::buttons::dropdown_arrow;
use super::{galley, lerp, paint_text};
use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::IconCache;
use crate::motion;
use crate::tokens::{Color, Tokens, geom, size};

/// Qt combo height.
pub const HEIGHT: f32 = 38.0;
/// List item height.
pub const ITEM_H: f32 = 26.0;

/// Where the currently open combo list is painted (read by popup panels).
fn open_list_id() -> egui::Id {
    egui::Id::unique("fthrust-open-combo-list")
}

/// Returns the rect of the open combo list, if any (`None` once closed).
pub fn open_list_rect(ctx: &egui::Context) -> Option<Rect> {
    ctx.data_mut(|d| d.get_temp::<Option<Rect>>(open_list_id()))
        .flatten()
}

/// A combo's items and selection.
#[derive(Clone, Debug, PartialEq)]
pub struct Combo {
    /// Display labels.
    pub items: Vec<String>,
    /// Selected index (`None` for an empty combo).
    pub selected: usize,
    /// Enabled.
    pub enabled: bool,
    /// Fixed width; `None` fills the available width.
    pub width: Option<f32>,
    /// Background token (`combo_qss(background)`).
    pub background: Color,
    /// Height (Qt gives 36 to combos sharing a row with a 26 px icon button).
    pub height: f32,
    /// Text weight of the current item.
    pub weight: Weight,
}

impl Combo {
    /// A combo over `items` with `selected` chosen.
    pub fn new(items: Vec<String>, selected: usize) -> Combo {
        Combo {
            items,
            selected,
            enabled: true,
            width: None,
            background: Color::Surface3,
            height: HEIGHT,
            weight: Weight::W400,
        }
    }

    /// Fixed width.
    pub fn with_width(mut self, w: f32) -> Combo {
        self.width = Some(w);
        self
    }

    /// Fixed height.
    pub fn with_height(mut self, h: f32) -> Combo {
        self.height = h;
        self
    }

    /// Text weight (`speedCombo` is 600).
    pub fn with_weight(mut self, weight: Weight) -> Combo {
        self.weight = weight;
        self
    }

    /// Current label.
    pub fn current(&self) -> &str {
        self.items
            .get(self.selected)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Selects the item whose label equals `label`, if any.
    pub fn select_label(&mut self, label: &str) -> bool {
        if let Some(i) = self.items.iter().position(|s| s == label) {
            self.selected = i;
            true
        } else {
            false
        }
    }

    /// Draws the combo; `changed()` on the response when the selection changed.
    pub fn ui(&mut self, ui: &mut Ui, t: &Tokens, icons: &mut IconCache, name: &str) -> Response {
        let w = self.width.unwrap_or(ui.available_width()).max(90.0);
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, self.height), sense);
        let popup_id = Id::unique(("fthrust-combo", name, resp.id));
        let mut open: bool = ui.ctx().data_mut(|d| d.get_temp(popup_id)).unwrap_or(false);
        if self.enabled && resp.clicked() {
            open = !open;
        }
        let hovered = self.enabled && resp.hovered();
        let blend = motion::hover(
            ui.ctx(),
            resp.id.with("hover"),
            hovered || open,
            t.reduce_motion,
        );
        let bg = lerp(t.color(self.background), t.color(Color::Surface3), blend);
        let radius = CornerRadius::same(geom::RADIUS as u8);
        ui.painter().rect_filled(rect, radius, bg);
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, t.color(Color::Border)),
            StrokeKind::Inside,
        );
        let text_color = if self.enabled {
            t.color(Color::Text)
        } else {
            t.color(Color::TextMuted)
        };
        let label = self.current().to_owned();
        let g = galley(ui, &label, size::BODY, self.weight, text_color);
        let text_rect = Rect::from_min_max(
            rect.min + Vec2::new(1.0, 0.0),
            Pos2::new(rect.max.x - 22.0 - 8.0, rect.max.y),
        );
        let clip = ui.clip_rect();
        ui.set_clip_rect(text_rect.intersect(clip));
        paint_text(ui, g, text_rect, Align::Min, 12.0, text_color);
        ui.set_clip_rect(clip);
        dropdown_arrow(ui, t, icons, rect, open, 14.0, 8.0);
        fidelity::mark(ui, name, rect)
            .sample(
                "fill",
                Pos2::new(rect.min.x + 5.0, rect.center().y),
                self.background,
            )
            .edge(Color::Border)
            .text(&label, size::BODY, self.weight)
            .icon(crate::icons::Icon::Dropdown.asset(), 14.0)
            .done();
        if open {
            let list_h = ITEM_H * self.items.len() as f32;
            let list_rect = Rect::from_min_size(
                Pos2::new(rect.min.x, rect.max.y),
                Vec2::new(rect.width(), list_h),
            );
            let screen = ui.ctx().content_rect();
            let list_rect = if list_rect.max.y > screen.max.y && rect.min.y - list_h >= screen.min.y
            {
                Rect::from_min_size(Pos2::new(rect.min.x, rect.min.y - list_h), list_rect.size())
            } else {
                list_rect
            };
            // Popup panels must not close on a press inside this list.
            ui.ctx()
                .data_mut(|d| d.insert_temp(open_list_id(), Some(list_rect)));
            let bg_token = self.background;
            let items = self.items.clone();
            let selected = self.selected;
            let mut picked: Option<usize> = None;
            let mut clicked_inside = false;
            Area::new(popup_id.with("area"))
                .order(Order::Foreground)
                .fixed_pos(list_rect.min)
                .constrain(false)
                .interactable(true)
                .show(ui.ctx(), |ui| {
                    ui.set_min_size(list_rect.size());
                    let painter = ui.painter();
                    painter.rect_filled(list_rect, radius, t.color(bg_token));
                    fidelity::mark(ui, format!("{name}/list"), list_rect)
                        .fill(bg_token)
                        .done();
                    for (i, item) in items.iter().enumerate() {
                        let irect = Rect::from_min_size(
                            Pos2::new(list_rect.min.x, list_rect.min.y + i as f32 * ITEM_H),
                            Vec2::new(list_rect.width(), ITEM_H),
                        );
                        let r = ui.interact(irect, popup_id.with(i), Sense::click());
                        let hovered = r.hovered();
                        if hovered || i == selected {
                            ui.painter().rect_filled(
                                irect,
                                CornerRadius::ZERO,
                                t.color(Color::Surface3),
                            );
                        }
                        let color = if i == selected {
                            t.color(Color::Accent)
                        } else {
                            t.color(Color::Text)
                        };
                        let g = galley(ui, item, size::BODY, Weight::W400, color);
                        paint_text(ui, g, irect, Align::Min, 10.0, color);
                        fidelity::mark(ui, format!("{name}/item[{item}]"), irect)
                            .text(item, size::BODY, Weight::W400)
                            .done();
                        if r.clicked() {
                            picked = Some(i);
                            clicked_inside = true;
                        }
                    }
                });
            if let Some(i) = picked {
                if i != self.selected {
                    self.selected = i;
                    resp.mark_changed();
                }
                open = false;
            }
            let pressed_outside = ui.ctx().input(|inp| {
                inp.pointer.any_pressed()
                    && inp
                        .pointer
                        .interact_pos()
                        .is_some_and(|p| !list_rect.contains(p) && !rect.contains(p))
            });
            let escape = ui.ctx().input(|inp| inp.key_pressed(egui::Key::Escape));
            if (pressed_outside && !clicked_inside) || escape {
                open = false;
            }
        }
        if !open {
            let mine = open_list_rect(ui.ctx());
            if mine.is_some_and(|r| r.min.x == rect.min.x && r.width() == rect.width()) {
                ui.ctx()
                    .data_mut(|d| d.insert_temp::<Option<Rect>>(open_list_id(), None));
            }
        }
        ui.ctx().data_mut(|d| d.insert_temp(popup_id, open));
        let _ = Color32::TRANSPARENT;
        resp
    }
}
