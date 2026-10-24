//! The 36 px title strip (`topBar`): logo, brand, Ko-fi button and the
//! three window buttons, laid out with 16 px left margin and 12 px spacing.

use egui::{Align, Color32, Pos2, Rect, Sense, Ui, Vec2};

use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::Icon;
use crate::state::AppState;
use crate::tokens::{Color, geom, size};
use crate::widgets::buttons::{self, Kind, Opts, window_button};
use crate::widgets::{galley, paint_text};

/// Button presses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TitleStripResponse {
    /// Close.
    pub close: bool,
    /// Minimise.
    pub minimise: bool,
    /// Maximise toggle.
    pub maximise: bool,
    /// Ko-fi link.
    pub kofi: bool,
    /// The pointer is over one of the strip's buttons.
    pub over_button: bool,
}

/// Ko-fi button text.
pub const KOFI_TEXT: &str = "Help us keep the app free and open for everyone";
/// Brand text (spec 11.2).
pub const BRAND: &str = "FTHRust Clips";

/// Draws the strip across the available width.
pub fn ui(ui: &mut Ui, state: &mut AppState) -> TitleStripResponse {
    let t = state.tokens.clone();
    let w = ui.available_width();
    let origin = ui.cursor().min;
    let strip = Rect::from_min_size(origin, Vec2::new(w, geom::TITLE_BAR_H));
    ui.painter().rect_filled(strip, 0.0, t.bar_bg());
    fidelity::mark(ui, "topBar", strip)
        .sample(
            "fill",
            Pos2::new(strip.min.x + 300.0, strip.center().y),
            Color::ShellBg,
        )
        .done();
    let mut out = TitleStripResponse::default();
    // Left cluster: logo, brand.
    let logo_rect = Rect::from_min_size(
        Pos2::new(strip.min.x + 16.0, strip.min.y + 4.0),
        Vec2::splat(28.0),
    );
    let tex = state.icons.texture(ui.ctx(), Icon::Logo, 28.0, &t);
    ui.painter().image(
        tex.id,
        logo_rect,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
    fidelity::mark(ui, "logoLabel", logo_rect)
        .icon(Icon::Logo.asset(), 28.0)
        .done();
    let g = galley(ui, BRAND, size::BODY_L, Weight::W700, t.bar_fg());
    let brand_rect = Rect::from_min_size(
        Pos2::new(logo_rect.max.x + geom::ROW_GAP, strip.min.y),
        Vec2::new(g.size().x.ceil(), geom::TITLE_BAR_H),
    );
    paint_text(ui, g, brand_rect, Align::Min, 0.0, t.bar_fg());
    fidelity::mark(ui, format!("QLabel[{BRAND}]"), brand_rect)
        .text(BRAND, size::BODY_L, Weight::W700)
        .done();
    // Right cluster, laid out right to left: close, max, min, Ko-fi.
    let mut x = strip.max.x;
    let btn = Vec2::new(46.0, geom::TITLE_BAR_H);
    x -= btn.x;
    let mut child = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(Pos2::new(x, strip.min.y), btn)),
    );
    let r = window_button(
        &mut child,
        &t,
        &mut state.icons,
        Icon::Close,
        Color::Error,
        "closeBtn",
    );
    out.close = r.clicked();
    out.over_button |= r.contains_pointer();
    x -= geom::ROW_GAP + btn.x;
    let mut child = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(Pos2::new(x, strip.min.y), btn)),
    );
    let r = window_button(
        &mut child,
        &t,
        &mut state.icons,
        Icon::Maximize,
        Color::Surface3,
        "winBtn#2",
    );
    out.maximise = r.clicked();
    out.over_button |= r.contains_pointer();
    x -= geom::ROW_GAP + btn.x;
    let mut child = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(Pos2::new(x, strip.min.y), btn)),
    );
    let r = window_button(
        &mut child,
        &t,
        &mut state.icons,
        Icon::Minimize,
        Color::Surface3,
        "winBtn",
    );
    out.minimise = r.clicked();
    out.over_button |= r.contains_pointer();
    if state.flag("show_kofi_widget", true) {
        let opts = Opts {
            fixed_height: Some(geom::TITLE_BAR_H),
            icon: Some((Icon::Kofi, Vec2::new(26.0, 22.0))),
            ..Opts::default()
        };
        // Measure first so the button can be placed from the right.
        let g = galley(ui, KOFI_TEXT, size::LABEL, Weight::W600, Color32::WHITE);
        let kw = (g.size().x + 30.0 + 16.0 * 2.0 + 2.0).ceil();
        x -= geom::ROW_GAP + kw;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(x, strip.min.y),
            Vec2::new(kw, geom::TITLE_BAR_H),
        )));
        let name = format!("QPushButton[{}]", &KOFI_TEXT[..40]);
        let r = buttons::button_with(
            &mut child,
            &t,
            Some(&mut state.icons),
            Kind::Outline,
            KOFI_TEXT,
            &name,
            &opts,
        );
        out.kofi = r.clicked();
        out.over_button |= r.contains_pointer();
    }
    ui.allocate_rect(strip, Sense::hover());
    out
}
