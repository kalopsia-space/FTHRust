//! The settings page (`_SettingsPage`): a 90 px tab bar of seven
//! text-under-icon buttons, a 1 px divider, and the page content with
//! 24 px insets sliding horizontally between categories (220 ms
//! InOutSine); Shift+Left/Right cycles the tabs.

pub mod audio;
pub mod clip;
pub mod general;
pub mod visuals;

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Ui, Vec2};

use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::Icon;
use crate::motion::{self, Ease, Transition};
use crate::state::{AppState, Tab};
use crate::tokens::{Color, geom, motion as timings, size};
use crate::widgets::{galley, lerp, paint_text, scroll};

/// Tab bar height.
pub const TAB_BAR_H: f32 = 90.0;
/// Tab button width (`min-width: 72px` + 20 px padding each side + 3).
pub const TAB_W: f32 = 115.0;
/// `Sizes.PAGE_INSET`.
pub const CONTENT_INSET: f32 = geom::PAGE_INSET;
/// Page layout margins `(0, 0, SPACE_4, SPACE_8)`.
pub const PAGE_MARGIN_RIGHT: f32 = 12.0;
/// Page bottom margin.
pub const PAGE_MARGIN_BOTTOM: f32 = 32.0;

/// A running category slide.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabSlide {
    from: Option<Tab>,
    transition: Option<Transition>,
    direction: f32,
}

impl TabSlide {
    /// Starts a slide at `now`.
    pub fn begin(&mut self, from: Tab, to: Tab, now: f64, reduce_motion: bool) {
        self.from = Some(from);
        self.direction = if to.index() > from.index() { 1.0 } else { -1.0 };
        self.transition =
            (!reduce_motion).then(|| Transition::new(now, timings::SLIDE_MS, Ease::InOutSine));
    }

    /// Still running.
    pub fn running(&self, now: f64, reduce_motion: bool) -> bool {
        self.transition
            .is_some_and(|t| t.running(now, reduce_motion))
    }

    /// `(outgoing_offset, incoming_offset)` in pixels for a stack `width`.
    pub fn offsets(&self, now: f64, width: f32, reduce_motion: bool) -> (f32, f32) {
        let p = self
            .transition
            .map(|t| t.progress(now, reduce_motion))
            .unwrap_or(1.0);
        (
            -self.direction * width * p,
            self.direction * width * (1.0 - p),
        )
    }
}

fn tab_icon(tab: Tab) -> Icon {
    match tab {
        Tab::General => Icon::General,
        Tab::Clip => Icon::Clip,
        Tab::Audio => Icon::Sound,
        Tab::Visuals => Icon::Visuals,
        Tab::Customize => Icon::Personalize,
        Tab::Performance => Icon::Performance,
        Tab::Shortcuts => Icon::Shortcuts,
    }
}

/// Switches to `tab` with the slide.
pub fn select_tab(ui: &Ui, state: &mut AppState, tab: Tab) {
    if tab == state.settings_tab {
        return;
    }
    let now = ui.ctx().input(|i| i.time);
    let reduce = state.tokens.reduce_motion;
    state.tab_slide.begin(state.settings_tab, tab, now, reduce);
    state.settings_tab = tab;
}

/// Draws the settings page into `ui` (the page-stack content rect).
pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, t.color(Color::Bg));
    // Keyboard: Shift+Right / Shift+Left cycle categories.
    let (right, left) = ui.ctx().input(|i| {
        (
            i.key_pressed(egui::Key::ArrowRight) && i.modifiers.shift,
            i.key_pressed(egui::Key::ArrowLeft) && i.modifiers.shift,
        )
    });
    if right {
        let next = state.settings_tab.cycle(1);
        select_tab(ui, state, next);
    } else if left {
        let next = state.settings_tab.cycle(-1);
        select_tab(ui, state, next);
    }
    // Tab bar.
    let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), TAB_BAR_H));
    ui.painter().rect_filled(bar, 0.0, t.color(Color::ShellBg2));
    crate::widgets::rule_h(
        ui,
        bar.min.x,
        bar.max.x,
        bar.max.y - 1.0,
        t.color(Color::ShellDivider),
    );
    fidelity::mark(ui, "settingsTabBar", bar)
        .sample(
            "fill",
            Pos2::new(bar.min.x + 100.0, bar.min.y + 10.0),
            Color::ShellBg2,
        )
        .done();
    let total = TAB_W * Tab::ALL.len() as f32;
    let mut x = bar.min.x + ((bar.width() - total) / 2.0).floor();
    let mut clicked: Option<Tab> = None;
    for tab in Tab::ALL {
        let r = Rect::from_min_size(Pos2::new(x, bar.min.y), Vec2::new(TAB_W, TAB_BAR_H));
        let name = format!("settingsTabBtn[{}]", tab.label());
        let resp = ui.interact(r, ui.scope_id().with(&name), egui::Sense::click());
        let checked = tab == state.settings_tab;
        let blend = motion::hover(
            ui.ctx(),
            resp.id.with("hover"),
            resp.hovered() && !checked,
            t.reduce_motion,
        );
        if !checked && blend > 0.0 {
            ui.painter().rect_filled(
                r,
                CornerRadius::ZERO,
                lerp(Color32::TRANSPARENT, t.color(Color::Surface3), blend),
            );
        }
        let (fg, weight) = if checked {
            (t.color(Color::Text), Weight::W600)
        } else {
            (
                lerp(t.color(Color::TextDim), t.color(Color::Text), blend),
                Weight::W400,
            )
        };
        let tex = state.icons.texture(ui.ctx(), tab_icon(tab), 36.0, &t);
        let irect = Rect::from_min_size(
            Pos2::new((r.center().x - 18.0).round(), r.min.y + 14.0),
            Vec2::splat(36.0),
        );
        ui.painter().image(
            tex.id,
            irect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        let g = galley(ui, tab.label(), size::LABEL, weight, fg);
        let th = crate::fonts::qt_height(size::LABEL, weight);
        paint_text(
            ui,
            g,
            Rect::from_min_size(
                Pos2::new(r.min.x, r.max.y - 10.0 - 2.0 - th),
                Vec2::new(r.width(), th),
            ),
            Align::Center,
            0.0,
            fg,
        );
        let mut marker = fidelity::mark(ui, name, r)
            .text(tab.label(), size::LABEL, weight)
            .icon(tab_icon(tab).asset(), 36.0);
        if checked {
            ui.painter().rect_filled(
                Rect::from_min_size(Pos2::new(r.min.x, r.max.y - 2.0), Vec2::new(r.width(), 2.0)),
                CornerRadius::ZERO,
                t.color(Color::Accent),
            );
            marker = marker.sample(
                "accent",
                Pos2::new(r.center().x.floor(), r.max.y - 2.0),
                Color::Accent,
            );
        }
        marker.done();
        if resp.clicked() {
            clicked = Some(tab);
        }
        x += TAB_W;
    }
    if let Some(tab) = clicked {
        select_tab(ui, state, tab);
    }
    // Divider and content.
    let divider = Rect::from_min_size(
        Pos2::new(rect.min.x, bar.max.y),
        Vec2::new(rect.width(), 1.0),
    );
    ui.painter()
        .rect_filled(divider, 0.0, t.color(Color::ShellDivider));
    fidelity::mark(ui, "settingsDivider", divider)
        .sample(
            "fill",
            Pos2::new(divider.center().x, divider.min.y),
            Color::ShellDivider,
        )
        .done();
    let content = Rect::from_min_max(Pos2::new(rect.min.x, divider.max.y), rect.max);
    fidelity::mark(ui, "settingsContent", content)
        .sample(
            "fill",
            Pos2::new(content.min.x + 4.0, content.max.y - 4.0),
            Color::Bg,
        )
        .done();
    let inner = Rect::from_min_max(
        content.min + Vec2::new(CONTENT_INSET, CONTENT_INSET),
        Pos2::new(content.max.x - CONTENT_INSET, content.max.y),
    );
    let now = ui.ctx().input(|i| i.time);
    let reduce = t.reduce_motion;
    if state.tab_slide.running(now, reduce) {
        let (out_off, in_off) = state.tab_slide.offsets(now, inner.width(), reduce);
        let outgoing = state.tab_slide.from.unwrap_or(state.settings_tab);
        let current = state.settings_tab;
        for (tab, off) in [(outgoing, out_off), (current, in_off)] {
            let r = inner.translate(Vec2::new(off, 0.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
            child.set_clip_rect(inner);
            page(&mut child, state, tab);
        }
        ui.ctx().request_repaint();
    } else {
        let tab = state.settings_tab;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        child.set_clip_rect(inner);
        page(&mut child, state, tab);
    }
    ui.allocate_rect(rect, egui::Sense::hover());
}

/// One category page inside its scroll area: a viewport 8 px narrower
/// than the inset rect (the vertical scrollbar) and page margins
/// `(0, 0, 12, 32)`.
fn page(ui: &mut Ui, state: &mut AppState, tab: Tab) {
    let t = state.tokens.clone();
    let rect = ui.max_rect();
    let name = format!("settingsPage[{}]", tab.label());
    // Qt shows the 8 px scrollbar only when the page is taller than the
    // viewport; a page that fits spans the full width.
    let height_id = egui::Id::unique(("settings-page-height", tab.label()));
    let last_h: f32 = ui
        .ctx()
        .data_mut(|d| d.get_temp(height_id))
        .unwrap_or(f32::INFINITY);
    let bar = if last_h > rect.height() { 8.0 } else { 0.0 };
    let viewport = Rect::from_min_max(rect.min, Pos2::new(rect.max.x - bar, rect.max.y));
    let mut area = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    scroll::vertical(&area, &t, &name)
        .max_height(rect.height())
        .show(&mut area, |ui| {
            ui.set_width(rect.width());
            let right_margin = if tab == Tab::Audio {
                0.0
            } else {
                PAGE_MARGIN_RIGHT
            };
            let body = Rect::from_min_size(
                Pos2::new(viewport.min.x, ui.cursor().min.y),
                Vec2::new(viewport.width() - right_margin, f32::INFINITY),
            );
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
            child.spacing_mut().item_spacing = Vec2::ZERO;
            match tab {
                Tab::General => general::ui(&mut child, state),
                Tab::Clip => clip::ui(&mut child, state),
                Tab::Audio => audio::ui(&mut child, state),
                Tab::Visuals => visuals::ui(&mut child, state),
                _ => placeholder(&mut child, state, tab),
            }
            let bottom = child.cursor().min.y
                + if tab == Tab::Audio {
                    0.0
                } else {
                    PAGE_MARGIN_BOTTOM
                };
            let content_h = bottom - body.min.y;
            if (content_h - last_h).abs() > 0.5 {
                ui.ctx().data_mut(|d| d.insert_temp(height_id, content_h));
                ui.ctx().request_repaint();
            }
            ui.allocate_rect(
                Rect::from_min_max(body.min, Pos2::new(viewport.max.x, bottom)),
                egui::Sense::hover(),
            );
        });
}

fn placeholder(ui: &mut Ui, state: &mut AppState, tab: Tab) {
    let t = state.tokens.clone();
    crate::widgets::section::flat_header(ui, &t, tab.label(), tab.label());
}

/// Vertical spacing (`layout.addSpacing`).
pub fn space(ui: &mut Ui, h: f32) {
    ui.allocate_exact_size(Vec2::new(0.0, h), egui::Sense::hover());
}
