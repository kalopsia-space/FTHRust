//! `lineedit_qss` and `_NumberSpinBox`.

use std::ops::RangeInclusive;

use egui::{Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::{galley, lerp, text_height};
use crate::fidelity;
use crate::fonts::{self, Weight};
use crate::icons::{Icon, IconCache};
use crate::motion;
use crate::tokens::{Color, Tokens, geom, size};
use crate::widgets::buttons::{IconButtonStyle, icon_button};

/// Options for [`line_edit_with`].
#[derive(Clone, Debug)]
pub struct LineEditOpts {
    /// Enabled.
    pub enabled: bool,
    /// Width (default: available).
    pub width: Option<f32>,
    /// Placeholder shown when empty.
    pub placeholder: String,
    /// Read-only (BROWSE fields).
    pub read_only: bool,
}

impl Default for LineEditOpts {
    fn default() -> Self {
        LineEditOpts {
            enabled: true,
            width: None,
            placeholder: String::new(),
            read_only: false,
        }
    }
}

/// SURFACE_2, 1 px BORDER, r6, padding 6×10, 15 px; hover BORDER_HI; focus ACCENT.
pub fn line_edit(ui: &mut Ui, t: &Tokens, text: &mut String, name: &str) -> Response {
    line_edit_with(ui, t, text, name, &LineEditOpts::default())
}

/// [`line_edit`] with options.
pub fn line_edit_with(
    ui: &mut Ui,
    t: &Tokens,
    text: &mut String,
    name: &str,
    opts: &LineEditOpts,
) -> Response {
    let h = (text_height(ui, size::BODY, Weight::W400) + 6.0 * 2.0 + 2.0).round();
    let w = opts.width.unwrap_or(ui.available_width());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let inner = Rect::from_min_max(
        rect.min + Vec2::new(11.0, 1.0),
        rect.max - Vec2::new(11.0, 1.0),
    );
    let fill = if opts.enabled {
        Color::Surface2
    } else {
        Color::Surface1
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(geom::RADIUS as u8), t.color(fill));
    let text_color = if opts.enabled {
        t.color(Color::Text)
    } else {
        t.color(Color::TextMuted)
    };
    let edit = egui::TextEdit::singleline(text)
        .font(fonts::id(size::BODY, Weight::W400))
        .text_color(text_color)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .align(egui::Align2::LEFT_CENTER)
        .interactive(opts.enabled && !opts.read_only)
        .hint_text(
            egui::RichText::new(&opts.placeholder)
                .font(fonts::id(size::BODY, Weight::W400))
                .color(t.color(Color::TextMuted)),
        )
        .desired_width(inner.width());
    let resp = ui.put(inner, edit).accessible_name(name);
    let hovered = opts.enabled
        && (resp.hovered()
            || rect.contains(
                ui.ctx()
                    .pointer_hover_pos()
                    .unwrap_or(Pos2::new(-1.0, -1.0)),
            ));
    let blend = motion::hover(ui.ctx(), resp.id.with("hover"), hovered, t.reduce_motion);
    let border = if !opts.enabled {
        t.color(Color::Border)
    } else if resp.has_focus() {
        t.color(Color::Accent)
    } else {
        lerp(t.color(Color::Border), t.color(Color::BorderHi), blend)
    };
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(geom::RADIUS as u8),
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    fidelity::mark(ui, name, rect)
        .sample("fill", Pos2::new(rect.min.x + 5.0, rect.center().y), fill)
        .edge(Color::Border)
        .text(text.as_str(), size::BODY, Weight::W400)
        .done();
    resp
}

/// `_NumberSpinBox`: 34 px tall, SURFACE_2, 1 px BORDER (ACCENT on hover /
/// focus), r6, value right-aligned 15 px 600 with letter spacing 0.8 in a
/// 64 px right padding, two 28×(h−2) stepper buttons (SURFACE_1, left
/// border BORDER, r4, hover ACCENT) at the right; steps clamp to `range`.
#[allow(clippy::too_many_arguments)]
pub fn number_spin(
    ui: &mut Ui,
    t: &Tokens,
    icons: &mut IconCache,
    value: &mut i64,
    range: RangeInclusive<i64>,
    step: i64,
    suffix: &str,
    name: &str,
) -> Response {
    let h = 34.0;
    let w = ui.available_width();
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let radius = CornerRadius::same(geom::RADIUS as u8);
    ui.painter()
        .rect_filled(rect, radius, t.color(Color::Surface2));
    // Value text (editable through a buffer stored per widget).
    let id = resp.id.with("buffer");
    let mut buffer: String = ui
        .ctx()
        .data_mut(|d| d.get_temp(id))
        .unwrap_or_else(|| value.to_string());
    let editing: bool = ui
        .ctx()
        .data_mut(|d| d.get_temp(id.with("editing")))
        .unwrap_or(false);
    if !editing {
        buffer = value.to_string();
    }
    let text_rect = Rect::from_min_max(
        rect.min + Vec2::new(11.0, 1.0),
        Pos2::new(rect.max.x - 64.0, rect.max.y - 1.0),
    );
    let font = fonts::id(size::BODY, Weight::W600);
    let edit = egui::TextEdit::singleline(&mut buffer)
        .font(font.clone())
        .text_color(t.color(Color::Text))
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .align(egui::Align2::RIGHT_CENTER)
        .desired_width(text_rect.width());
    let edit_resp = ui.put(text_rect, edit).accessible_name(name);
    if edit_resp.has_focus() {
        ui.ctx()
            .data_mut(|d| d.insert_temp(id.with("editing"), true));
        ui.ctx().data_mut(|d| d.insert_temp(id, buffer.clone()));
    }
    if edit_resp.lost_focus() {
        ui.ctx()
            .data_mut(|d| d.insert_temp(id.with("editing"), false));
        if let Ok(v) = buffer.trim().parse::<i64>() {
            let v = v.clamp(*range.start(), *range.end());
            if v != *value {
                *value = v;
                resp.mark_changed();
            }
        }
    }
    if !suffix.is_empty() && !edit_resp.has_focus() {
        let g = galley(
            ui,
            suffix,
            size::BODY,
            Weight::W600,
            t.color(Color::TextDim),
        );
        let _ = g;
    }
    // Steppers.
    let bw = 28.0;
    let bh = (h - 2.0).max(0.0);
    let down_rect = Rect::from_min_size(
        Pos2::new(rect.max.x - bw * 2.0 - 1.0, rect.min.y + 1.0),
        Vec2::new(bw, bh),
    );
    let up_rect = Rect::from_min_size(
        Pos2::new(rect.max.x - bw - 1.0, rect.min.y + 1.0),
        Vec2::new(bw, bh),
    );
    let style = |enabled: bool| IconButtonStyle {
        size: Vec2::new(bw, bh),
        bg: Some(Color::Surface1),
        hover_bg: Some(Color::Accent),
        pressed_bg: Some(Color::Text),
        border: None,
        hover_border: None,
        radius: geom::RADIUS_SM,
        hover_tint: Some(Color::Bg),
        enabled,
    };
    let can_down = *value > *range.start();
    let can_up = *value < *range.end();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(down_rect));
    child.set_clip_rect(rect);
    let down = icon_button(
        &mut child,
        t,
        icons,
        Icon::Decrease,
        14.0,
        style(can_down),
        &format!("{name}/fthrStepDown"),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(up_rect));
    let up = icon_button(
        &mut child,
        t,
        icons,
        Icon::Increase,
        14.0,
        style(can_up),
        &format!("{name}/fthrStepUp"),
    );
    for r in [&down_rect, &up_rect] {
        super::rule_v(ui, r.min.x, r.min.y, r.max.y, t.color(Color::Border));
    }
    if down.clicked() && can_down {
        *value = (*value - step).max(*range.start());
        resp.mark_changed();
    }
    if up.clicked() && can_up {
        *value = (*value + step).min(*range.end());
        resp.mark_changed();
    }
    let hovered = rect.contains(
        ui.ctx()
            .pointer_hover_pos()
            .unwrap_or(Pos2::new(-1.0, -1.0)),
    );
    let border = if hovered || edit_resp.has_focus() {
        Color::Accent
    } else {
        Color::Border
    };
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, t.color(border)),
        StrokeKind::Inside,
    );
    let _ = Color32::TRANSPARENT;
    fidelity::mark(ui, name, rect)
        .sample(
            "fill",
            Pos2::new(rect.min.x + 5.0, rect.center().y),
            Color::Surface2,
        )
        .edge(Color::Border)
        .text(&buffer, size::BODY, Weight::W600)
        .done();
    resp
}

/// Spin box style variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpinStyle {
    /// `_NumberSpinBox`: SURFACE_2 field, SURFACE_1 steppers (r4).
    Number,
    /// `presetStepper`: SURFACE_1 field, SURFACE_2 steppers (r6).
    Preset,
}

/// A float spin box with `decimals` (`_PresetDoubleSpinBox` when
/// `decimals > 0`): 34 px tall, value right-aligned 15 px 600, suffix, two
/// 28×32 steppers; steps clamp to `range`. Returns the response of the
/// field (`changed()` when the value changed).
#[allow(clippy::too_many_arguments)]
pub fn spin_box(
    ui: &mut Ui,
    t: &Tokens,
    icons: &mut IconCache,
    value: &mut f64,
    range: RangeInclusive<f64>,
    step: f64,
    decimals: usize,
    suffix: &str,
    style: SpinStyle,
    enabled: bool,
    name: &str,
) -> Response {
    let h = 34.0;
    let w = ui.available_width();
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let radius = CornerRadius::same(geom::RADIUS as u8);
    let (field_bg, stepper_bg, stepper_radius) = match style {
        SpinStyle::Number => (Color::Surface2, Color::Surface1, geom::RADIUS_SM),
        SpinStyle::Preset => (Color::Surface1, Color::Surface2, geom::RADIUS),
    };
    let field_bg = if enabled { field_bg } else { Color::Surface1 };
    ui.painter().rect_filled(rect, radius, t.color(field_bg));
    let fmt = |v: f64| format!("{v:.decimals$}{suffix}");
    let id = resp.id.with("buffer");
    let editing: bool = ui
        .ctx()
        .data_mut(|d| d.get_temp(id.with("editing")))
        .unwrap_or(false);
    let mut buffer: String = if editing {
        ui.ctx()
            .data_mut(|d| d.get_temp(id))
            .unwrap_or_else(|| fmt(*value))
    } else {
        fmt(*value)
    };
    let text_rect = Rect::from_min_max(
        rect.min + Vec2::new(11.0, 1.0),
        Pos2::new(rect.max.x - 64.0, rect.max.y - 1.0),
    );
    let text_color = if enabled {
        t.color(Color::Text)
    } else {
        t.color(Color::TextMuted)
    };
    let edit = egui::TextEdit::singleline(&mut buffer)
        .font(fonts::id(size::BODY, Weight::W600))
        .text_color(text_color)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .align(egui::Align2::RIGHT_CENTER)
        .interactive(enabled)
        .desired_width(text_rect.width());
    let edit_resp = ui.put(text_rect, edit).accessible_name(name);
    if edit_resp.has_focus() {
        ui.ctx()
            .data_mut(|d| d.insert_temp(id.with("editing"), true));
        ui.ctx().data_mut(|d| d.insert_temp(id, buffer.clone()));
    }
    if edit_resp.lost_focus() {
        ui.ctx()
            .data_mut(|d| d.insert_temp(id.with("editing"), false));
        let digits: String = buffer
            .trim()
            .trim_end_matches(suffix.trim())
            .trim()
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect();
        if let Ok(v) = digits.parse::<f64>() {
            let v = v.clamp(*range.start(), *range.end());
            if v != *value {
                *value = v;
                resp.mark_changed();
            }
        }
    }
    let bw = 28.0;
    let bh = h - 2.0;
    let down_rect = Rect::from_min_size(
        Pos2::new(rect.max.x - bw * 2.0 - 1.0, rect.min.y + 1.0),
        Vec2::new(bw, bh),
    );
    let up_rect = Rect::from_min_size(
        Pos2::new(rect.max.x - bw - 1.0, rect.min.y + 1.0),
        Vec2::new(bw, bh),
    );
    let can_down = enabled && *value > *range.start();
    let can_up = enabled && *value < *range.end();
    let bstyle = |on: bool| IconButtonStyle {
        size: Vec2::new(bw, bh),
        bg: Some(stepper_bg),
        hover_bg: Some(Color::Accent),
        pressed_bg: Some(Color::Text),
        border: None,
        hover_border: None,
        radius: stepper_radius,
        hover_tint: Some(Color::Bg),
        enabled: on,
    };
    let (down_name, up_name) = match style {
        SpinStyle::Number => (format!("{name}/fthrStepDown"), format!("{name}/fthrStepUp")),
        SpinStyle::Preset => (
            format!("{name}/presetStepDown"),
            format!("{name}/presetStepUp"),
        ),
    };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(down_rect));
    let down = icon_button(
        &mut child,
        t,
        icons,
        Icon::Decrease,
        14.0,
        bstyle(can_down),
        &down_name,
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(up_rect));
    let up = icon_button(
        &mut child,
        t,
        icons,
        Icon::Increase,
        14.0,
        bstyle(can_up),
        &up_name,
    );
    for r in [&down_rect, &up_rect] {
        super::rule_v(ui, r.min.x, r.min.y, r.max.y, t.color(Color::Border));
    }
    if down.clicked() && can_down {
        *value = (*value - step).max(*range.start());
        resp.mark_changed();
    }
    if up.clicked() && can_up {
        *value = (*value + step).min(*range.end());
        resp.mark_changed();
    }
    let hovered = enabled
        && rect.contains(
            ui.ctx()
                .pointer_hover_pos()
                .unwrap_or(Pos2::new(-1.0, -1.0)),
        );
    let border = if hovered || edit_resp.has_focus() {
        Color::Accent
    } else {
        Color::Border
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
            Pos2::new(rect.min.x + 5.0, rect.center().y),
            field_bg,
        )
        .edge(Color::Border)
        .done();
    resp
}
