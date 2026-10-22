//! `ErrorBar` (`ui/error_bar.py`): a 36 px SURFACE_1 bar with a 1 px top
//! border in the level colour; "Notice", bold title, "—", detail, stretch,
//! "+N MORE", up to two action buttons and a 28 px dismiss button; margins
//! (20, 0, 20, 0), spacing 10. Qt's `QFrame { border-top }` rule also
//! applies to every QLabel inside, so each label is drawn with its own
//! 1 px top line and 3 px of horizontal frame padding, exactly as Qt does.

use std::collections::VecDeque;

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use fthrust_core::error_codes::format_error_title;

use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::Icon;
use crate::motion;
use crate::state::AppState;
use crate::tokens::{Color, Tokens, geom, size};
use crate::widgets::buttons::{IconButtonStyle, icon_button};
use crate::widgets::{galley, lerp, paint_text};

/// Severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// ERROR colour.
    Error,
    /// WARNING colour.
    Warning,
}

impl Level {
    /// Token.
    pub fn color(self) -> Color {
        match self {
            Level::Error => Color::Error,
            Level::Warning => Color::Warning,
        }
    }
}

/// An action button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    /// Button text.
    pub label: String,
    /// Returned when clicked.
    pub id: String,
}

/// One queued message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Title as displayed.
    pub title: String,
    /// Detail text.
    pub detail: String,
    /// Severity.
    pub level: Level,
    /// Actions (first two shown).
    pub actions: Vec<Action>,
    /// Incident key.
    pub key: Option<String>,
    /// Dismiss after an action fires.
    pub dismiss_on_action: bool,
}

/// Error bar state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorBarState {
    current: Option<Entry>,
    queue: VecDeque<Entry>,
    /// `error_notifications_enabled`.
    pub enabled: bool,
}

impl Default for ErrorBarState {
    fn default() -> Self {
        ErrorBarState {
            current: None,
            queue: VecDeque::new(),
            enabled: true,
        }
    }
}

impl ErrorBarState {
    /// An empty bar with `error_notifications_enabled` = `enabled`.
    pub fn new(enabled: bool) -> ErrorBarState {
        ErrorBarState {
            enabled,
            ..Default::default()
        }
    }

    /// Shown.
    pub fn visible(&self) -> bool {
        self.current.is_some()
    }

    /// The displayed entry.
    pub fn current(&self) -> Option<&Entry> {
        self.current.as_ref()
    }

    /// Queued entries behind the current one.
    pub fn queued_count(&self) -> usize {
        self.queue.len()
    }

    /// `+N MORE` when queued.
    pub fn more_label(&self) -> Option<String> {
        (!self.queue.is_empty()).then(|| format!("+{} MORE", self.queue.len()))
    }

    /// `ErrorBar.push`: shows or queues; a `key` replaces the entry with the
    /// same key wherever it is.
    pub fn push(
        &mut self,
        title: &str,
        detail: &str,
        level: Level,
        actions: Vec<Action>,
        key: Option<String>,
        dismiss_on_action: bool,
    ) {
        let entry = Entry {
            title: title.to_owned(),
            detail: detail.to_owned(),
            level,
            actions,
            key: key.clone(),
            dismiss_on_action,
        };
        if let Some(k) = &key {
            if self
                .current
                .as_ref()
                .is_some_and(|c| c.key.as_deref() == Some(k))
            {
                self.current = Some(entry);
                return;
            }
            if let Some(slot) = self.queue.iter_mut().find(|q| q.key.as_deref() == Some(k)) {
                *slot = entry;
                return;
            }
        }
        if self.current.is_none() {
            self.current = Some(entry);
        } else {
            self.queue.push_back(entry);
        }
    }

    /// `MainWindow.push_error`: formats the title (`[Error NNN] TITLE` when
    /// documented, else upper-cased), bounds the detail to 240 characters,
    /// and respects `error_notifications_enabled`. Returns whether shown.
    pub fn push_error(
        &mut self,
        title: &str,
        detail: &str,
        level: Level,
        actions: Vec<Action>,
        key: Option<String>,
        dismiss_on_action: bool,
    ) -> bool {
        if !self.enabled {
            return false;
        }
        let display_title = format_error_title(title);
        let detail: String = detail.trim().chars().take(240).collect();
        self.push(
            &display_title,
            &detail,
            level,
            actions,
            key,
            dismiss_on_action,
        );
        true
    }

    /// Dismisses the current entry and shows the next.
    pub fn dismiss_current(&mut self) {
        self.current = self.queue.pop_front();
    }

    /// Clears everything.
    pub fn clear(&mut self) {
        self.current = None;
        self.queue.clear();
    }

    /// Removes the entry with `key`; returns whether one was removed.
    pub fn remove_by_key(&mut self, key: &str) -> bool {
        if self
            .current
            .as_ref()
            .is_some_and(|c| c.key.as_deref() == Some(key))
        {
            self.dismiss_current();
            return true;
        }
        let before = self.queue.len();
        self.queue.retain(|q| q.key.as_deref() != Some(key));
        self.queue.len() != before
    }
}

/// Qt label with the inherited `border-top`: `advance + 6` wide, 1 px line
/// in the level colour at its top, text vertically centred.
#[allow(clippy::too_many_arguments)]
fn bar_label(
    ui: &mut Ui,
    t: &Tokens,
    x: f32,
    y: f32,
    h: f32,
    text: &str,
    px: f32,
    weight: Weight,
    color: Color,
    line: Color,
) -> Rect {
    let g = galley(ui, text, px, weight, t.color(color));
    let rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(g.size().x.round() + 6.0, h));
    crate::widgets::rule_h(ui, rect.min.x, rect.max.x, rect.min.y, t.color(line));
    paint_text(ui, g, rect, Align::Center, 0.0, t.color(color));
    fidelity::mark(ui, format!("QLabel[{}]", &text[..text.len().min(40)]), rect)
        .text(text, px, weight)
        .top(line)
        .done();
    rect
}

/// Draws the bar when visible; returns a fired action id.
pub fn ui(ui: &mut Ui, state: &mut AppState) -> Option<String> {
    let t = state.tokens.clone();
    let entry = state.error_bar.current().cloned()?;
    let level = entry.level.color();
    let origin = ui.cursor().min;
    let bar = Rect::from_min_size(origin, Vec2::new(ui.available_width(), geom::HW_BAR_H));
    ui.painter().rect_filled(bar, 0.0, t.color(Color::Surface1));
    crate::widgets::rule_h(ui, bar.min.x, bar.max.x, bar.min.y, t.color(level));
    fidelity::mark(ui, "errorBar", bar)
        .sample(
            "fill",
            Pos2::new(bar.center().x, bar.center().y),
            Color::Surface1,
        )
        .top(level)
        .done();
    let (y, h) = (bar.min.y + 1.0, bar.height() - 1.0);
    let mut x = bar.min.x + 20.0;
    let r = bar_label(
        ui,
        &t,
        x,
        y,
        h,
        "Notice",
        size::BODY,
        Weight::W400,
        level,
        level,
    );
    x = r.max.x + 10.0;
    let r = bar_label(
        ui,
        &t,
        x,
        y,
        h,
        &entry.title,
        size::BODY,
        Weight::W600,
        level,
        level,
    );
    x = r.max.x + 10.0;
    let r = bar_label(
        ui,
        &t,
        x,
        y,
        h,
        "—",
        size::BODY,
        Weight::W400,
        Color::Text,
        level,
    );
    x = r.max.x + 10.0;
    bar_label(
        ui,
        &t,
        x,
        y,
        h,
        &entry.detail,
        size::BODY,
        Weight::W400,
        Color::Text,
        level,
    );
    // Right side, right to left: dismiss, actions, "+N MORE".
    let mut rx = bar.max.x - 20.0;
    rx -= 28.0;
    let dismiss_rect = Rect::from_min_size(Pos2::new(rx, y), Vec2::new(28.0, h));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(dismiss_rect));
    let mut style = IconButtonStyle::plain(28.0);
    style.size = Vec2::new(28.0, h);
    style.hover_tint = Some(Color::Text);
    let dismiss = icon_button(
        &mut child,
        &t,
        &mut state.icons,
        Icon::Close,
        14.0,
        style,
        "errorBarDismiss",
    );
    let mut fired: Option<String> = None;
    for action in entry.actions.iter().take(2).rev() {
        let g = galley(ui, &action.label, size::LABEL, Weight::W600, Color32::WHITE);
        let w = g.size().x.ceil() + 20.0 + 2.0;
        let bh = g.size().y.round() + 6.0 + 2.0;
        rx -= 10.0 + w;
        let rect = Rect::from_min_size(
            Pos2::new(rx, y + ((h - bh) / 2.0).floor()),
            Vec2::new(w, bh),
        );
        let resp = ui.interact(
            rect,
            ui.scope_id().with(("error-action", &action.id)),
            Sense::click(),
        );
        let blend = motion::hover(
            ui.ctx(),
            resp.id.with("hover"),
            resp.hovered(),
            t.reduce_motion,
        );
        let bg = lerp(Color32::TRANSPARENT, t.color(level), blend);
        let fg = lerp(t.color(level), t.color(Color::Text), blend);
        if bg.a() > 0 {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, bg);
        }
        ui.painter().rect_stroke(
            rect,
            CornerRadius::ZERO,
            Stroke::new(1.0, t.color(level)),
            StrokeKind::Inside,
        );
        paint_text(ui, g, rect, Align::Center, 0.0, fg);
        fidelity::mark(ui, format!("QPushButton[{}]", action.label), rect)
            .edge(level)
            .text(&action.label, size::LABEL, Weight::W600)
            .done();
        if resp.clicked() {
            fired = Some(action.id.clone());
        }
    }
    if let Some(more) = state.error_bar.more_label() {
        let g = galley(ui, &more, size::LABEL, Weight::W400, Color32::WHITE);
        let w = g.size().x.round() + 6.0;
        rx -= 10.0 + w;
        bar_label(
            ui,
            &t,
            rx,
            y,
            h,
            &more,
            size::LABEL,
            Weight::W400,
            Color::TextDim,
            level,
        );
    }
    ui.allocate_rect(bar, Sense::hover());
    if dismiss.clicked() || (fired.is_some() && entry.dismiss_on_action) {
        state.error_bar.dismiss_current();
    }
    fired
}
