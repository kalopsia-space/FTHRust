//! Button recipes (`ui/style.py` `button_*_qss`, `TopBarButton`, window
//! buttons, `IconButton`).

use egui::{Align, Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::{galley, lerp, paint_text};
use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::{Icon, IconCache};
use crate::motion;
use crate::tokens::{Color, Tokens, geom, size};

/// Button kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Kind {
    Primary,
    Outline,
    Secondary,
    Pill,
    PillGhost,
    Ghost,
}

struct Recipe {
    bg: Option<Color>,
    text: Color,
    border: Option<Color>,
    hover_bg: Option<Color>,
    hover_text: Color,
    hover_border: Option<Color>,
    pressed_bg: Option<Color>,
    pressed_text: Color,
    pressed_border: Option<Color>,
    pad: Vec2,
}

fn recipe(kind: Kind) -> Recipe {
    use Color::*;
    match kind {
        Kind::Primary => Recipe {
            bg: Some(Accent),
            text: Bg,
            border: None,
            hover_bg: Some(Text),
            hover_text: Bg,
            hover_border: None,
            pressed_bg: Some(AccentDim),
            pressed_text: Text,
            pressed_border: None,
            pad: Vec2::new(18.0, 10.0),
        },
        Kind::Outline => Recipe {
            bg: None,
            text: Text,
            border: Some(BorderHi),
            hover_bg: None,
            hover_text: Accent,
            hover_border: Some(Accent),
            pressed_bg: Some(Accent),
            pressed_text: Bg,
            pressed_border: Some(Accent),
            pad: Vec2::new(16.0, 9.0),
        },
        Kind::Secondary => Recipe {
            bg: Some(Surface3),
            text: Text,
            border: Some(Border),
            hover_bg: Some(Border),
            hover_text: Text,
            hover_border: Some(BorderHi),
            pressed_bg: Some(Surface2),
            pressed_text: Text,
            pressed_border: Some(Border),
            pad: Vec2::new(18.0, 10.0),
        },
        Kind::Pill => Recipe {
            bg: Some(AccentSoft),
            text: Accent,
            border: None,
            hover_bg: Some(Accent),
            hover_text: Bg,
            hover_border: None,
            pressed_bg: Some(Accent),
            pressed_text: Bg,
            pressed_border: None,
            pad: Vec2::new(16.0, 7.0),
        },
        Kind::PillGhost => Recipe {
            bg: None,
            text: TextDim,
            border: Some(BorderHi),
            hover_bg: None,
            hover_text: Accent,
            hover_border: Some(Accent),
            pressed_bg: None,
            pressed_text: Accent,
            pressed_border: Some(Accent),
            pad: Vec2::new(14.0, 7.0),
        },
        Kind::Ghost => Recipe {
            bg: None,
            text: Text,
            border: None,
            hover_bg: None,
            hover_text: Accent,
            hover_border: None,
            pressed_bg: None,
            pressed_text: Accent,
            pressed_border: None,
            pad: Vec2::new(10.0, 6.0),
        },
    }
}

/// Options for [`button_with`].
#[derive(Clone, Debug)]
pub struct Opts {
    /// Disabled buttons use BORDER / TEXT_MUTED (primary) or dim text.
    pub enabled: bool,
    /// Minimum width (Qt `setMinimumWidth`).
    pub min_width: f32,
    /// A fixed height instead of text + padding.
    pub fixed_height: Option<f32>,
    /// Fill the available width (Qt expanding policy).
    pub expand: bool,
    /// Icon drawn left of the text at this logical size.
    pub icon: Option<(Icon, Vec2)>,
    /// Text colour override (error-bar actions use the level colour).
    pub text_color: Option<Color>,
    /// Border colour override.
    pub border_color: Option<Color>,
    /// Pixel size override (default LABEL 14).
    pub px: Option<f32>,
    /// Weight override (default 600).
    pub weight: Option<Weight>,
    /// Padding override.
    pub pad: Option<Vec2>,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            enabled: true,
            min_width: 0.0,
            fixed_height: None,
            expand: false,
            icon: None,
            text_color: None,
            border_color: None,
            px: None,
            weight: None,
            pad: None,
        }
    }
}

/// Draws a text button of `kind`.
pub fn button(ui: &mut Ui, t: &Tokens, kind: Kind, text: &str, name: &str) -> Response {
    button_with(ui, t, None, kind, text, name, &Opts::default())
}

/// Draws a text button with options (and an optional icon).
pub fn button_with(
    ui: &mut Ui,
    t: &Tokens,
    icons: Option<&mut IconCache>,
    kind: Kind,
    text: &str,
    name: &str,
    opts: &Opts,
) -> Response {
    let r = recipe(kind);
    let px = opts.px.unwrap_or(size::LABEL);
    let weight = opts.weight.unwrap_or(Weight::W600);
    let pad = opts.pad.unwrap_or(r.pad);
    let border_w = if r.border.is_some() || opts.border_color.is_some() {
        1.0
    } else {
        0.0
    };
    let galley = galley(ui, text, px, weight, Color32::WHITE);
    let text_size = galley.size();
    let icon_w = opts.icon.map(|(_, s)| s.x + 4.0).unwrap_or(0.0);
    let mut w = (text_size.x + icon_w + pad.x * 2.0 + border_w * 2.0)
        .ceil()
        .max(opts.min_width);
    if opts.expand {
        w = w.max(ui.available_width());
    }
    let h = opts
        .fixed_height
        .unwrap_or((text_size.y + pad.y * 2.0 + border_w * 2.0).round());
    let sense = if opts.enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), sense);
    let hovered = opts.enabled && resp.hovered();
    let pressed = opts.enabled && resp.is_pointer_button_down_on();
    let blend = motion::hover(ui.ctx(), resp.id.with("hover"), hovered, t.reduce_motion);
    let (bg, text_c, border) = if !opts.enabled {
        match kind {
            Kind::Primary => (
                Some(t.color(Color::Border)),
                t.color(Color::TextMuted),
                None,
            ),
            _ => (
                r.bg.map(|c| t.color(c)),
                t.color(Color::TextMuted),
                opts.border_color.or(r.border).map(|c| t.color(c)),
            ),
        }
    } else if pressed {
        (
            r.pressed_bg.map(|c| t.color(c)),
            t.color(
                opts.text_color
                    .map(|_| r.pressed_text)
                    .unwrap_or(r.pressed_text),
            ),
            r.pressed_border.map(|c| t.color(c)),
        )
    } else {
        let base_text = opts.text_color.unwrap_or(r.text);
        let base_border = opts.border_color.or(r.border);
        let hover_border = opts.border_color.map(|_| Color::Accent).or(r.hover_border);
        let bg = match (r.bg, r.hover_bg) {
            (Some(a), Some(b)) => Some(lerp(t.color(a), t.color(b), blend)),
            (Some(a), None) => Some(t.color(a)),
            (None, Some(b)) => Some(lerp(Color32::TRANSPARENT, t.color(b), blend)),
            (None, None) => None,
        };
        let text_c = lerp(t.color(base_text), t.color(r.hover_text), blend);
        let border = match (base_border, hover_border) {
            (Some(a), Some(b)) => Some(lerp(t.color(a), t.color(b), blend)),
            (Some(a), None) => Some(t.color(a)),
            (None, _) => None,
        };
        (bg, text_c, border)
    };
    let radius = CornerRadius::same(geom::RADIUS as u8);
    if let Some(bg) = bg {
        ui.painter().rect_filled(rect, radius, bg);
    }
    if let Some(border) = border {
        ui.painter()
            .rect_stroke(rect, radius, Stroke::new(1.0, border), StrokeKind::Inside);
    }
    let content_w = text_size.x + icon_w;
    let mut x = rect.center().x - content_w / 2.0;
    if let (Some((icon, isize)), Some(icons)) = (opts.icon, icons) {
        let tex = icons.texture(ui.ctx(), icon, isize.x.max(isize.y), t);
        let irect = Rect::from_min_size(
            egui::pos2(x.round(), (rect.center().y - isize.y / 2.0).round()),
            isize,
        );
        ui.painter().image(
            tex.id,
            irect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        x += isize.x + 4.0;
    }
    let text_rect = Rect::from_min_size(
        egui::pos2(x, rect.min.y),
        Vec2::new(text_size.x, rect.height()),
    );
    paint_text(ui, galley, text_rect, Align::Min, 0.0, text_c);
    let mut marker = fidelity::mark(ui, name, rect).text(text, px, weight);
    if let Some(bg) = r.bg {
        marker = marker.sample("fill", egui::pos2(rect.min.x + 8.0, rect.center().y), bg);
    }
    if let Some(b) = opts.border_color.or(r.border) {
        marker = marker.edge(b);
    }
    if let Some((icon, isize)) = opts.icon {
        marker = marker.icon(icon.asset(), isize.x);
    }
    marker.done();
    resp
}

/// ACCENT fill, BG text (`button_primary_qss`).
pub fn primary(ui: &mut Ui, t: &Tokens, text: &str, name: &str) -> Response {
    button(ui, t, Kind::Primary, text, name)
}

/// Transparent, 1 px BORDER_HI (`button_outline_qss`).
pub fn outline(ui: &mut Ui, t: &Tokens, text: &str, name: &str) -> Response {
    button(ui, t, Kind::Outline, text, name)
}

/// SURFACE_3, 1 px BORDER (`button_secondary_qss`).
pub fn secondary(ui: &mut Ui, t: &Tokens, text: &str, name: &str) -> Response {
    button(ui, t, Kind::Secondary, text, name)
}

/// ACCENT_SOFT fill, ACCENT text (`button_pill_qss`).
pub fn pill(ui: &mut Ui, t: &Tokens, text: &str, name: &str) -> Response {
    button(ui, t, Kind::Pill, text, name)
}

/// 1 px BORDER_HI, TEXT_DIM (`button_pill_ghost_qss`).
pub fn pill_ghost(ui: &mut Ui, t: &Tokens, text: &str, name: &str) -> Response {
    button(ui, t, Kind::PillGhost, text, name)
}

/// No background (`button_ghost_qss`).
pub fn ghost(ui: &mut Ui, t: &Tokens, text: &str, name: &str) -> Response {
    button(ui, t, Kind::Ghost, text, name)
}

/// Style for [`icon_button`].
#[derive(Clone, Copy, Debug)]
pub struct IconButtonStyle {
    /// Button size.
    pub size: Vec2,
    /// Resting fill.
    pub bg: Option<Color>,
    /// Hover fill.
    pub hover_bg: Option<Color>,
    /// Pressed fill.
    pub pressed_bg: Option<Color>,
    /// Resting border.
    pub border: Option<Color>,
    /// Hover border.
    pub hover_border: Option<Color>,
    /// Corner radius.
    pub radius: f32,
    /// Icon tint on hover (SourceIn), else the theme tint.
    pub hover_tint: Option<Color>,
    /// Enabled.
    pub enabled: bool,
}

impl IconButtonStyle {
    /// A plain square button with no background.
    pub fn plain(size: f32) -> IconButtonStyle {
        IconButtonStyle {
            size: Vec2::splat(size),
            bg: None,
            hover_bg: None,
            pressed_bg: None,
            border: None,
            hover_border: None,
            radius: 0.0,
            hover_tint: None,
            enabled: true,
        }
    }
}

/// An icon-only button.
pub fn icon_button(
    ui: &mut Ui,
    t: &Tokens,
    icons: &mut IconCache,
    icon: Icon,
    icon_px: f32,
    style: IconButtonStyle,
    name: &str,
) -> Response {
    let sense = if style.enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(style.size, sense);
    let hovered = style.enabled && resp.hovered();
    let pressed = style.enabled && resp.is_pointer_button_down_on();
    let blend = motion::hover(ui.ctx(), resp.id.with("hover"), hovered, t.reduce_motion);
    let radius = CornerRadius::same(style.radius as u8);
    let bg = if pressed {
        style.pressed_bg.or(style.hover_bg).map(|c| t.color(c))
    } else {
        match (style.bg, style.hover_bg) {
            (Some(a), Some(b)) => Some(lerp(t.color(a), t.color(b), blend)),
            (Some(a), None) => Some(t.color(a)),
            (None, Some(b)) => Some(lerp(Color32::TRANSPARENT, t.color(b), blend)),
            (None, None) => None,
        }
    };
    if let Some(bg) = bg {
        ui.painter().rect_filled(rect, radius, bg);
    }
    let border = match (style.border, style.hover_border) {
        (Some(a), Some(b)) => Some(lerp(t.color(a), t.color(b), blend)),
        (Some(a), None) => Some(t.color(a)),
        (None, Some(b)) if hovered => Some(t.color(b)),
        _ => None,
    };
    if let Some(border) = border {
        ui.painter()
            .rect_stroke(rect, radius, Stroke::new(1.0, border), StrokeKind::Inside);
    }
    let tex = if let (Some(ht), true) = (style.hover_tint, hovered || pressed) {
        let mut tinted = t.clone();
        tinted
            .icon_tints
            .insert(icon.logical_name().to_owned(), t.color(ht));
        icons.texture(ui.ctx(), icon, icon_px, &tinted)
    } else {
        icons.texture(ui.ctx(), icon, icon_px, t)
    };
    let irect = Rect::from_center_size(rect.center().round(), Vec2::splat(icon_px));
    let tint = if style.enabled {
        Color32::WHITE
    } else {
        Color32::from_white_alpha(110)
    };
    ui.painter().image(
        tex.id,
        irect,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        tint,
    );
    let mut marker = fidelity::mark(ui, name, rect).icon(icon.asset(), icon_px);
    if let Some(bg) = style.bg {
        marker = marker.sample("fill", egui::pos2(rect.min.x + 3.0, rect.min.y + 3.0), bg);
    }
    marker.done();
    resp
}

/// Title-strip window button: 46×36, 14 px icon, transparent, hover
/// `hover_bg` (SURFACE_3 for min/max, ERROR for close).
pub fn window_button(
    ui: &mut Ui,
    t: &Tokens,
    icons: &mut IconCache,
    icon: Icon,
    hover_bg: Color,
    name: &str,
) -> Response {
    icon_button(
        ui,
        t,
        icons,
        icon,
        14.0,
        IconButtonStyle {
            size: Vec2::new(46.0, geom::TITLE_BAR_H),
            bg: None,
            hover_bg: Some(hover_bg),
            pressed_bg: Some(hover_bg),
            border: None,
            hover_border: None,
            radius: 0.0,
            hover_tint: Some(Color::Text),
            enabled: true,
        },
        name,
    )
}

/// Capture toolbar control (`TopBarButton`): 44 px tall, SURFACE_3, r6,
/// 15 px 400 text left-aligned with padding 14 / 30, 14 px dropdown arrow
/// 8 px from the right (rotated when `open`); hover ACCENT text on
/// SURFACE_2; pressed BG text on ACCENT.
pub fn top_bar_button(
    ui: &mut Ui,
    t: &Tokens,
    icons: &mut IconCache,
    text: &str,
    open: bool,
    width: f32,
    name: &str,
) -> Response {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(width, geom::CAPTURE_CONTROL_H), Sense::click());
    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();
    let blend = motion::hover(ui.ctx(), resp.id.with("hover"), hovered, t.reduce_motion);
    let (bg, fg) = if pressed {
        (t.color(Color::Accent), t.color(Color::Bg))
    } else {
        (
            lerp(t.color(Color::Surface3), t.color(Color::Surface2), blend),
            lerp(t.color(Color::Text), t.color(Color::Accent), blend),
        )
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(geom::RADIUS as u8), bg);
    let galley = galley(ui, text, size::TOOLBAR, Weight::W400, fg);
    let avail = rect.width() - 14.0 - 30.0;
    let shown = if galley.size().x > avail {
        elide(ui, text, avail)
    } else {
        text.to_owned()
    };
    let galley = if shown == text {
        galley
    } else {
        super::galley(ui, &shown, size::TOOLBAR, Weight::W400, fg)
    };
    paint_text(ui, galley, rect, Align::Min, 14.0, fg);
    dropdown_arrow(ui, t, icons, rect, open, 14.0, 8.0);
    fidelity::mark(ui, name, rect)
        .sample(
            "fill",
            egui::pos2(rect.min.x + 4.0, rect.center().y),
            Color::Surface3,
        )
        .text(text, size::TOOLBAR, Weight::W400)
        .icon(Icon::Dropdown.asset(), 14.0)
        .done();
    resp
}

/// Paints the shared dropdown arrow (`paint_dropdown_arrow`): `size` px,
/// `right_padding` from the right edge, rotated 180° when `expanded`.
pub fn dropdown_arrow(
    ui: &Ui,
    t: &Tokens,
    icons: &mut IconCache,
    rect: Rect,
    expanded: bool,
    size: f32,
    right_padding: f32,
) {
    let tex = icons.texture(ui.ctx(), Icon::Dropdown, size, t);
    let x = rect.max.x - size - right_padding;
    let y = rect.min.y + ((rect.height() - size) / 2.0).floor();
    let irect = Rect::from_min_size(egui::pos2(x, y), Vec2::splat(size));
    let uv = if expanded {
        Rect::from_min_max(egui::pos2(1.0, 1.0), egui::pos2(0.0, 0.0))
    } else {
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
    };
    ui.painter().image(tex.id, irect, uv, Color32::WHITE);
}

/// Qt `ElideRight` for a width.
pub fn elide(ui: &Ui, text: &str, max_w: f32) -> String {
    let mut s: String = text.to_owned();
    while !s.is_empty() {
        let candidate = format!("{s}…");
        if super::galley(ui, &candidate, size::TOOLBAR, Weight::W400, Color32::WHITE)
            .size()
            .x
            <= max_w
        {
            return candidate;
        }
        s.pop();
    }
    "…".to_owned()
}
