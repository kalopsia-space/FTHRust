//! `ShellPageStack`: Clips ↔ Settings with a 220 ms InOutSine vertical
//! slide of page snapshots (the live incoming page is drawn offset instead
//! of snapshotted; the result is the same motion).

use egui::{Pos2, Rect, Ui, Vec2};

use crate::motion::{Ease, Transition};
use crate::state::{AppState, Page};
use crate::tokens::{Color, motion};

/// A running page slide.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageSlide {
    from: Option<Page>,
    to: Option<Page>,
    transition: Option<Transition>,
    /// +1 when moving to a higher page index (Clips → Settings), −1 back.
    direction: f32,
}

impl PageSlide {
    /// Starts a slide at `now`.
    pub fn begin(&mut self, from: Page, to: Page, now: f64, reduce_motion: bool) {
        self.from = Some(from);
        self.to = Some(to);
        self.direction = if (to as usize) > (from as usize) {
            1.0
        } else {
            -1.0
        };
        self.transition =
            (!reduce_motion).then(|| Transition::new(now, motion::SLIDE_MS, Ease::InOutSine));
    }

    /// Whether the slide still runs.
    pub fn running(&self, now: f64, reduce_motion: bool) -> bool {
        self.transition
            .is_some_and(|t| t.running(now, reduce_motion))
    }

    /// `(outgoing_offset, incoming_offset)` in pixels for a stack `height`.
    pub fn offsets(&self, now: f64, height: f32, reduce_motion: bool) -> (f32, f32) {
        let p = self
            .transition
            .map(|t| t.progress(now, reduce_motion))
            .unwrap_or(1.0);
        let d = self.direction;
        (-d * height * p, d * height * (1.0 - p))
    }

    /// The page being left, while running.
    pub fn outgoing(&self) -> Option<Page> {
        self.from
    }
}

/// Draws the page stack over `rect` (1 px SHELL_DIVIDER top border), with
/// `draw_page` rendering a page into a ui.
pub fn ui(
    ui: &mut Ui,
    state: &mut AppState,
    rect: Rect,
    mut draw_page: impl FnMut(&mut Ui, &mut AppState, Page),
) {
    let t = state.tokens.clone();
    ui.painter().rect_filled(rect, 0.0, t.color(Color::Bg));
    crate::widgets::rule_h(
        ui,
        rect.min.x,
        rect.max.x,
        rect.min.y,
        t.color(Color::ShellDivider),
    );
    crate::fidelity::mark(ui, "mainStack", rect)
        .top(Color::ShellDivider)
        .done();
    let content = Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y + 1.0), rect.max);
    let now = ui.ctx().input(|i| i.time);
    let reduce = t.reduce_motion;
    if state.page_slide.running(now, reduce) {
        let (out_off, in_off) = state.page_slide.offsets(now, content.height(), reduce);
        let outgoing = state.page_slide.outgoing().unwrap_or(state.page);
        let current = state.page;
        for (page, off) in [(outgoing, out_off), (current, in_off)] {
            let r = content.translate(Vec2::new(0.0, off));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
            child.set_clip_rect(content);
            draw_page(&mut child, state, page);
        }
        ui.ctx().request_repaint();
    } else {
        let page = state.page;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content));
        child.set_clip_rect(content);
        draw_page(&mut child, state, page);
    }
}
