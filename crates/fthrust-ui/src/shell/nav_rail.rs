//! The 94 px navigation rail (`navigationPanel`): Clips and Settings
//! (78×78 text-under-icon buttons, margins (8, 16, 8, 12), spacing 12) and
//! one button per open clip editor.

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Sense, StrokeKind, Ui, Vec2};

use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::Icon;
use crate::motion;
use crate::state::{AppState, Page};
use crate::tokens::{Color, Tokens, geom, size};
use crate::widgets::{galley, lerp, paint_text};

/// Rail width.
pub const WIDTH: f32 = geom::NAV_W;

/// What the rail asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RailResponse {
    /// Page selected.
    pub page: Option<Page>,
    /// Editor session (1-based) selected.
    pub editor: Option<usize>,
    /// Editor session to close (right-click → "Close editor").
    pub close_editor: Option<usize>,
}

/// Draws the rail over `rect`.
pub fn ui(ui: &mut Ui, state: &mut AppState, rect: Rect) -> RailResponse {
    let t = state.tokens.clone();
    ui.painter()
        .rect_filled(rect, 0.0, t.color(Color::ShellBg2));
    crate::widgets::rule_v(
        ui,
        rect.max.x - 1.0,
        rect.min.y,
        rect.max.y,
        t.color(Color::ShellDivider),
    );
    fidelity::mark(ui, "navigationPanel", rect)
        .sample(
            "fill",
            Pos2::new(rect.min.x + 4.0, rect.center().y),
            Color::ShellBg2,
        )
        .done();
    let mut out = RailResponse::default();
    let mut y = rect.min.y + 16.0;
    let btn = Vec2::splat(geom::NAV_BTN);
    let home = tool_button(
        ui,
        &t,
        state,
        Rect::from_min_size(Pos2::new(rect.min.x + 8.0, y), btn),
        Icon::Home,
        "Clips",
        state.page == Page::Clips,
        "homeBtn",
    );
    if home {
        out.page = Some(Page::Clips);
    }
    y += btn.y + 12.0;
    let gear = tool_button(
        ui,
        &t,
        state,
        Rect::from_min_size(Pos2::new(rect.min.x + 8.0, y), btn),
        Icon::General,
        "Settings",
        state.page == Page::Settings,
        "settingsGearBtn",
    );
    if gear {
        out.page = Some(Page::Settings);
    }
    y += btn.y + 12.0;
    let sessions = state.editor_sessions.clone();
    for s in sessions.iter().take(3) {
        let r = Rect::from_min_size(Pos2::new(rect.min.x + 8.0, y), btn);
        let label = format!(
            "Editor {}\n{}",
            s.number,
            elide_stem(ui, &s.stem, btn.x - 16.0)
        );
        let name = format!("editorSidebarButton[{}]", label.replace('\n', " "));
        let resp = ui
            .interact(
                r,
                ui.scope_id().with(("editorSidebarButton", s.number)),
                Sense::click(),
            )
            .on_hover_text(format!(
                "{}\nPaused — click to restore\nRight-click to close",
                s.file_name
            ));
        draw_tool_button(
            ui,
            &t,
            r,
            None,
            &label,
            false,
            resp.hovered(),
            &name,
            &mut state.icons,
        );
        if resp.clicked() {
            out.editor = Some(s.number);
        }
        resp.context_menu(|ui| {
            if ui.button("Close editor").clicked() {
                out.close_editor = Some(s.number);
                ui.close();
            }
        });
        y += btn.y + 12.0;
    }
    ui.allocate_rect(rect, Sense::hover());
    out
}

fn elide_stem(ui: &Ui, stem: &str, max_w: f32) -> String {
    let fits = |s: &str| {
        galley(ui, s, size::LABEL, Weight::W400, Color32::WHITE)
            .size()
            .x
            <= max_w
    };
    if fits(stem) {
        return stem.to_owned();
    }
    let mut s = stem.to_owned();
    while !s.is_empty() {
        s.pop();
        let candidate = format!("{s}…");
        if fits(&candidate) {
            return candidate;
        }
    }
    "…".into()
}

#[allow(clippy::too_many_arguments)]
fn tool_button(
    ui: &mut Ui,
    t: &Tokens,
    state: &mut AppState,
    rect: Rect,
    icon: Icon,
    text: &str,
    checked: bool,
    name: &str,
) -> bool {
    let resp = ui.interact(rect, ui.scope_id().with(name), Sense::click());
    draw_tool_button(
        ui,
        t,
        rect,
        Some(icon),
        text,
        checked,
        resp.hovered(),
        name,
        &mut state.icons,
    );
    resp.clicked()
}

/// `QFrame#navigationPanel QToolButton` recipe: transparent, TEXT_DIM 14 px
/// 400, 1 px transparent border r6, padding 8; hover SURFACE_2 + TEXT;
/// checked ACCENT_SOFT + TEXT 600 + 3 px ACCENT left border.
#[allow(clippy::too_many_arguments)]
fn draw_tool_button(
    ui: &mut Ui,
    t: &Tokens,
    rect: Rect,
    icon: Option<Icon>,
    text: &str,
    checked: bool,
    hovered: bool,
    name: &str,
    icons: &mut crate::icons::IconCache,
) {
    let blend = motion::hover(
        ui.ctx(),
        ui.scope_id().with((name, "hover")),
        hovered && !checked,
        t.reduce_motion,
    );
    let radius = CornerRadius::same(geom::RADIUS as u8);
    let (bg, fg, weight) = if checked {
        (
            t.color(Color::AccentSoft),
            t.color(Color::Text),
            Weight::W600,
        )
    } else {
        (
            lerp(Color32::TRANSPARENT, t.color(Color::Surface2), blend),
            lerp(t.color(Color::TextDim), t.color(Color::Text), blend),
            Weight::W400,
        )
    };
    if bg.a() > 0 {
        ui.painter().rect_filled(rect, radius, bg);
    }
    if checked {
        // border-left: 3px solid ACCENT inside the rounded box.
        let left = Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height()));
        ui.painter().rect_filled(
            left,
            CornerRadius {
                nw: geom::RADIUS as u8,
                sw: geom::RADIUS as u8,
                ne: 0,
                se: 0,
            },
            t.color(Color::Accent),
        );
    }
    let icon_h = if icon.is_some() { 18.0 } else { 0.0 };
    let gap = if icon.is_some() { 4.0 } else { 0.0 };
    let lines: Vec<&str> = text.split('\n').collect();
    let galleys: Vec<_> = lines
        .iter()
        .map(|l| galley(ui, l, size::LABEL, weight, fg))
        .collect();
    let text_h: f32 = galleys.iter().map(|g| g.size().y.round()).sum();
    let block_h = icon_h + gap + text_h;
    let mut y = rect.min.y + ((rect.height() - block_h) / 2.0).floor();
    if let Some(icon) = icon {
        let tex = icons.texture(ui.ctx(), icon, 18.0, t);
        let irect = Rect::from_min_size(
            Pos2::new((rect.center().x - 9.0).round(), y),
            Vec2::splat(18.0),
        );
        ui.painter().image(
            tex.id,
            irect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        y += icon_h + gap;
    }
    for g in galleys {
        let h = g.size().y.round();
        paint_text(
            ui,
            g,
            Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), h)),
            Align::Center,
            0.0,
            fg,
        );
        y += h;
    }
    let _ = StrokeKind::Inside;
    let mut marker = fidelity::mark(ui, name, rect).text(text, size::LABEL, weight);
    if let Some(icon) = icon {
        marker = marker.icon(icon.asset(), 18.0);
    }
    if checked {
        marker = marker
            .sample(
                "fill",
                Pos2::new(rect.min.x + 12.0, rect.min.y + 8.0),
                Color::AccentSoft,
            )
            .sample(
                "accent",
                Pos2::new(rect.min.x + 1.0, rect.center().y),
                Color::Accent,
            );
    }
    marker.done();
}
