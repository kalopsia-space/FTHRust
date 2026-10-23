//! The editor sidebar (`clip_viewer.py:5085-5380`): share/quality/export,
//! then six collapsible panels (CLIP SPEED, EDIT TOOLS, AUDIO MIX, COLOR &
//! EFFECTS, CLIP DETAILS, FILE DETAILS) and the Delete button, in a
//! scrolling column. Geometry follows the Qt dumps: content margins 14/16,
//! 8 px spacing, 41 px panel headers, panel bodies with 14/4/14/14 margins.

use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Align, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use fthrust_core::editor::format;
use fthrust_core::editor::snapshot::EFFECT_LIMITS;
use fthrust_core::editor::tools::{QuickCrop, SPEED_PRESETS};
use fthrust_core::library_ownership::{Ownership, classify_media_path};
use serde_json::json;

use super::{EditorRequest, MetaState};
use crate::fidelity;
use crate::fonts::Weight;
use crate::state::AppState;
use crate::tokens::{Color, Tokens, geom, size};
use crate::widgets::buttons::dropdown_arrow;
use crate::widgets::combo::Combo;
use crate::widgets::slider::editor_slider;
use crate::widgets::{galley, paint_text};

const MARGIN_X: f32 = 14.0;
const MARGIN_Y: f32 = 16.0;
const SPACING: f32 = 8.0;
const HEADER_H: f32 = 41.0;
const SCROLLBAR_W: f32 = 8.0;
const DELETE_H: f32 = 36.0;
/// Panel titles, in order.
pub const PANELS: [&str; 6] = [
    "CLIP SPEED",
    "EDIT TOOLS",
    "AUDIO MIX",
    "COLOR & EFFECTS",
    "CLIP DETAILS",
    "FILE DETAILS",
];
/// `QUALITY_OPTIONS`: label and `export_quality` value.
pub use fthrust_core::export::QUALITY_OPTIONS;
const EFFECT_TITLES: [&str; 6] = [
    "Exposure",
    "Contrast",
    "Saturation",
    "Temperature",
    "Sharpness",
    "Vignette",
];
const ROW_H: f32 = 67.0;
const EFFECT_H: f32 = 54.0;

/// Body height of panel `i` when open.
fn body_h(i: usize, sources: usize) -> f32 {
    match i {
        0 => 4.0 + 21.0 + 8.0 + 38.0 + 8.0 + 25.0 + 14.0,
        1 => 4.0 + 32.0 + 8.0 + 26.0 + 8.0 + 32.0 + 8.0 + 17.0 + 14.0,
        2 => {
            let rows = (1 + sources) as f32;
            4.0 + rows * ROW_H + (rows - 1.0) * SPACING + 14.0
        }
        3 => 4.0 + 6.0 * EFFECT_H + 6.0 * SPACING + 28.0 + 14.0,
        4 => {
            4.0 + 21.0 + 10.0 + 26.0 + 10.0 + 21.0 + 10.0 + 24.0 + 10.0 + 21.0 + 10.0 + 24.0 + 14.0
        }
        _ => 4.0 + 7.0 * (21.0 + 10.0 + 23.0) + 6.0 * 10.0 + 14.0,
    }
}

/// Repeated Qt object names get `#2`, `#3`… in tree order.
#[derive(Default)]
struct Names(HashMap<String, usize>);

impl Names {
    fn uniq(&mut self, name: impl Into<String>) -> String {
        let name = name.into();
        let n = self.0.entry(name.clone()).or_insert(0);
        *n += 1;
        if *n == 1 { name } else { format!("{name}#{n}") }
    }
}

/// Paints into the scrolled column.
struct Col<'a> {
    t: &'a Tokens,
    quality_h: f32,
    speed_h: f32,
    x: f32,
    w: f32,
    top: f32,
    names: Names,
}

impl Col<'_> {
    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(self.x + x, self.top + y), Vec2::new(w, h))
    }
}

fn is_linked_import(state: &AppState, path: &std::path::Path) -> bool {
    let imports: Vec<PathBuf> = state
        .settings
        .get("imported_clip_folders")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    classify_media_path(path, &state.library.clips_root, &imports) == Ownership::LinkedImported
}

/// Draws the sidebar into `rect`.
pub fn ui(ui: &mut Ui, rect: Rect, state: &mut AppState) {
    if state.editor.is_none() {
        return;
    }
    let now = state.editor_now();
    let t = state.tokens.clone();
    ui.painter().rect_filled(rect, 0.0, t.color(Color::ShellBg));
    let sources = state.editor.as_ref().map_or(0, |e| e.source_rows().len());
    // Content height. When it does not fit, Qt first squeezes the combos
    // from their 38 px hint to their 36 px minimum, then scrolls.
    let panels = state.editor_panels;
    let natural = |quality: f32, speed: f32| -> (f32, [f32; 6]) {
        let mut y =
            MARGIN_Y + 42.0 + SPACING + quality + SPACING + 38.0 + SPACING + 19.0 + SPACING + 6.0;
        let mut panel_y = [0.0f32; 6];
        for (i, py) in panel_y.iter_mut().enumerate() {
            *py = y;
            let body = if panels[i] {
                body_h(i, sources) - if i == 0 { 38.0 - speed } else { 0.0 }
            } else {
                0.0
            };
            y += 2.0 + HEADER_H + body + SPACING;
        }
        (y, panel_y)
    };
    let (full, _) = natural(38.0, 38.0);
    let excess = (full + DELETE_H + MARGIN_Y - rect.height()).max(0.0);
    let speed_shrinkable = if panels[0] { 2.0 } else { 0.0 };
    let shrink = excess.min(2.0 + speed_shrinkable).floor();
    let quality_shrink = if speed_shrinkable > 0.0 {
        (shrink / 2.0).ceil()
    } else {
        shrink
    };
    let speed_shrink = shrink - quality_shrink;
    let quality_h = 38.0 - quality_shrink;
    let speed_h = 38.0 - speed_shrink;
    let (natural_delete, panel_y) = natural(quality_h, speed_h);
    let delete_y = natural_delete.max(rect.height() - MARGIN_Y - DELETE_H);
    let total = delete_y + DELETE_H + MARGIN_Y;
    let overflow = total > rect.height() + 0.5;
    let content_w = if overflow {
        rect.width() - SCROLLBAR_W
    } else {
        rect.width()
    };
    // Scrolling.
    let max_scroll = (total - rect.height()).max(0.0);
    let hovered = ui.rect_contains_pointer(rect);
    if hovered {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        state.editor_sidebar_scroll -= dy;
    }
    state.editor_sidebar_scroll = state.editor_sidebar_scroll.clamp(0.0, max_scroll);
    let scroll = state.editor_sidebar_scroll;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    let ui = &mut child;
    let mut c = Col {
        t: &t,
        quality_h,
        speed_h,
        x: rect.min.x + MARGIN_X,
        w: content_w - 2.0 * MARGIN_X,
        top: rect.min.y - scroll,
        names: Names::default(),
    };
    fidelity::mark(
        ui,
        "editorSidebarContent",
        Rect::from_min_size(rect.min, Vec2::new(content_w, rect.height())),
    )
    .done();
    primary_actions(ui, &mut c, state);
    for (i, y) in panel_y.iter().enumerate() {
        panel(ui, &mut c, state, i, *y, sources, now);
    }
    delete_button(ui, &mut c, state, delete_y);
    if overflow {
        let track = Rect::from_min_max(Pos2::new(rect.max.x - SCROLLBAR_W, rect.min.y), rect.max);
        ui.painter()
            .rect_filled(track, 0.0, t.color(Color::ShellBg));
        let inner = track.shrink(2.0);
        let h = (inner.height() * rect.height() / total).max(28.0);
        let y0 = inner.min.y
            + (inner.height() - h)
                * if max_scroll > 0.0 {
                    scroll / max_scroll
                } else {
                    0.0
                };
        ui.painter().rect_filled(
            Rect::from_min_size(Pos2::new(inner.min.x, y0), Vec2::new(inner.width(), h)),
            CornerRadius::same(geom::RADIUS as u8),
            t.color(Color::BorderHi),
        );
    }
}

/// A filled or outlined text button.
#[allow(clippy::too_many_arguments)]
fn button(
    ui: &mut Ui,
    t: &Tokens,
    r: Rect,
    text: &str,
    name: &str,
    px: f32,
    (bg, border, fg): (Option<Color>, Option<Color>, Color),
    (hover_bg, hover_border, hover_fg): (Option<Color>, Option<Color>, Option<Color>),
    enabled: bool,
) -> egui::Response {
    let resp = ui.interact(
        r,
        ui.scope_id().with(name),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = enabled && resp.hovered();
    let bg = if hovered { hover_bg.or(bg) } else { bg };
    let border = if hovered {
        hover_border.or(border)
    } else {
        border
    };
    let fg = if hovered { hover_fg.unwrap_or(fg) } else { fg };
    let radius = CornerRadius::same(geom::RADIUS as u8);
    if let Some(bg) = bg {
        ui.painter().rect_filled(r, radius, t.color(bg));
    }
    if let Some(b) = border {
        ui.painter()
            .rect_stroke(r, radius, Stroke::new(1.0, t.color(b)), StrokeKind::Inside);
    }
    let g = galley(ui, text, px, Weight::W600, t.color(fg));
    paint_text(ui, g, r, Align::Center, 0.0, t.color(fg));
    let mut m = fidelity::mark(ui, name, r).text(text, px, Weight::W600);
    if let Some(b) = border {
        m = m.sample("top", Pos2::new(r.center().x, r.min.y), b);
    }
    m.done();
    resp
}

/// The outlined tool buttons (`cropBtn`, `quickCropBtn`, … `resetLookBtn`).
fn outline(
    ui: &mut Ui,
    t: &Tokens,
    r: Rect,
    text: &str,
    name: &str,
    enabled: bool,
    active: bool,
) -> egui::Response {
    let (border, fg) = match (enabled, active) {
        (false, _) => (Color::Border, Color::TextMuted),
        (true, true) => (Color::Accent, Color::Accent),
        (true, false) => (Color::BorderHi, Color::Text),
    };
    button(
        ui,
        t,
        r,
        text,
        name,
        size::LABEL,
        (None, Some(border), fg),
        (None, Some(Color::Accent), Some(Color::Accent)),
        enabled,
    )
}

#[allow(clippy::too_many_arguments)]
fn label(
    ui: &Ui,
    t: &Tokens,
    r: Rect,
    text: &str,
    px: f32,
    weight: Weight,
    color: Color,
    align: Align,
    name: &str,
) {
    let g = galley(ui, text, px, weight, t.color(color));
    paint_text(ui, g, r, align, 0.0, t.color(color));
    if !name.is_empty() {
        fidelity::mark(ui, name, r).text(text, px, weight).done();
    }
}

fn primary_actions(ui: &mut Ui, c: &mut Col<'_>, state: &mut AppState) {
    let t = c.t;
    let r = c.rect(0.0, MARGIN_Y, c.w, 42.0);
    let share = button(
        ui,
        t,
        r,
        "Share clip",
        "shareBtn",
        size::BODY_L,
        (Some(Color::Accent), None, Color::Bg),
        (Some(Color::Text), None, None),
        true,
    );
    // Quality combo (`export_quality`).
    let current = state
        .settings
        .get_str("export_quality")
        .and_then(|v| QUALITY_OPTIONS.iter().position(|(_, q)| *q == v))
        .unwrap_or(1);
    let mut combo = Combo::new(
        QUALITY_OPTIONS
            .iter()
            .map(|(l, _)| (*l).to_owned())
            .collect(),
        current,
    )
    .with_height(c.quality_h);
    let r = c.rect(0.0, MARGIN_Y + 42.0 + SPACING, c.w, c.quality_h);
    let name = format!("WheelSafeComboBox[{}]", QUALITY_OPTIONS[current].0);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    let changed = combo
        .ui(&mut child, t, &mut state.icons, &name)
        .on_hover_text("Used for Export and Share. Lossless saves filtered frames in MKV; files can be very large.")
        .changed();
    if changed {
        state
            .settings
            .set("export_quality", json!(QUALITY_OPTIONS[combo.selected].1));
    }
    let r = c.rect(
        0.0,
        MARGIN_Y + 42.0 + c.quality_h + 2.0 * SPACING,
        c.w,
        38.0,
    );
    let export = button(
        ui,
        t,
        r,
        "Export",
        "exportBtn",
        size::BODY_L,
        (Some(Color::Surface3), Some(Color::Border), Color::Text),
        (Some(Color::Border), Some(Color::BorderHi), None),
        true,
    );
    let r = c.rect(
        0.0,
        MARGIN_Y + 42.0 + c.quality_h + 38.0 + 3.0 * SPACING,
        c.w,
        19.0,
    );
    let jobs = ui.interact(r, ui.scope_id().with("exportJobs"), Sense::click());
    let now = state.editor_now();
    let jobs_text = super::export_ui::jobs_label_cached(state, now);
    let fg = if jobs.hovered() {
        Color::Accent
    } else {
        Color::Text
    };
    // The reference leaves this button unstyled, so its font is the host's
    // default (Google Sans Flex on the render machine): only the box is held.
    label(
        ui,
        t,
        r,
        &jobs_text,
        size::BODY,
        Weight::W400,
        fg,
        Align::Center,
        "",
    );
    fidelity::mark(ui, "QPushButton[Export jobs]", r).done();
    if jobs.clicked() {
        state.export_jobs_open = true;
    }
    if let Some(e) = state.editor.as_mut() {
        if share.clicked() {
            e.requests.push(EditorRequest::Share);
        }
        if export.clicked() {
            e.requests.push(EditorRequest::Export);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn panel(
    ui: &mut Ui,
    c: &mut Col<'_>,
    state: &mut AppState,
    i: usize,
    y: f32,
    sources: usize,
    now: f64,
) {
    let t = c.t;
    let open = state.editor_panels[i];
    let squeeze = if i == 0 { 38.0 - c.speed_h } else { 0.0 };
    let h = 2.0
        + HEADER_H
        + if open {
            body_h(i, sources) - squeeze
        } else {
            0.0
        };
    let frame = c.rect(0.0, y, c.w, h);
    let radius = CornerRadius::same(geom::RADIUS as u8);
    ui.painter()
        .rect_filled(frame, radius, t.color(Color::Surface1));
    ui.painter().rect_stroke(
        frame,
        radius,
        Stroke::new(1.0, t.color(Color::Border)),
        StrokeKind::Inside,
    );
    let name = c.names.uniq("detailsPanel");
    fidelity::mark(ui, name, frame)
        .sample(
            "top",
            Pos2::new(frame.center().x, frame.min.y),
            Color::Border,
        )
        .done();
    let header = c.rect(1.0, y + 1.0, c.w - 2.0, HEADER_H);
    let resp = ui.interact(
        header,
        ui.scope_id().with(("panelHeader", i)),
        Sense::click(),
    );
    let fg = if resp.hovered() {
        Color::Accent
    } else {
        Color::Text
    };
    let g = galley(ui, PANELS[i], size::LABEL, Weight::W600, t.color(fg));
    paint_text(ui, g, header, Align::Min, 12.0, t.color(fg));
    dropdown_arrow(ui, t, &mut state.icons, header, open, 14.0, 8.0);
    let name = c.names.uniq("panelHeader");
    fidelity::mark(ui, name, header)
        .text(PANELS[i], size::LABEL, Weight::W600)
        .done();
    if resp.clicked() {
        state.editor_panels[i] = !open;
    }
    if !open {
        return;
    }
    let body = c.rect(
        1.0,
        y + 1.0 + HEADER_H,
        c.w - 2.0,
        body_h(i, sources) - squeeze,
    );
    let name = c.names.uniq("panelBody");
    fidelity::mark(ui, name, body).done();
    // Body content: 14 px side margins, 4 px top.
    let bx = 1.0 + 14.0;
    let bw = c.w - 2.0 - 28.0;
    let by = y + 1.0 + HEADER_H + 4.0;
    match i {
        0 => speed_body(ui, c, state, bx, by, bw, now),
        1 => edit_body(ui, c, state, bx, by, bw, now),
        2 => {
            fidelity::mark(ui, "audioMixPanel", body).done();
            audio_body(ui, c, state, bx, by, bw)
        }
        3 => look_body(ui, c, state, bx, by, bw, now),
        4 => details_body(ui, c, state, bx, by, bw),
        _ => file_body(ui, c, state, bx, by, bw),
    }
}

fn detail_key(ui: &Ui, c: &mut Col<'_>, x: f32, y: f32, w: f32, text: &str, shared: bool) {
    let name = if shared {
        c.names.uniq(format!("detailKey[{text}]"))
    } else {
        c.names.uniq("detailKey")
    };
    let r = c.rect(x, y, w, 21.0);
    label(
        ui,
        c.t,
        r,
        text,
        size::LABEL,
        Weight::W600,
        Color::TextDim,
        Align::Min,
        &name,
    );
}

#[allow(clippy::too_many_arguments)]
fn speed_body(
    ui: &mut Ui,
    c: &mut Col<'_>,
    state: &mut AppState,
    x: f32,
    y: f32,
    w: f32,
    now: f64,
) {
    detail_key(ui, c, x, y, w, "Playback speed", false);
    let Some(e) = state.editor.as_mut() else {
        return;
    };
    let current = SPEED_PRESETS
        .iter()
        .position(|(_, r)| (*r - e.speed).abs() < 1e-9)
        .unwrap_or(3);
    let mut combo = Combo::new(
        SPEED_PRESETS.iter().map(|(l, _)| (*l).to_owned()).collect(),
        current,
    )
    .with_height(c.speed_h)
    .with_weight(Weight::W600);
    let r = c.rect(x, y + 21.0 + 8.0, w, c.speed_h);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    let changed = combo
        .ui(&mut child, c.t, &mut state.icons, "speedCombo")
        .on_hover_text("Playback and export speed. Audio follows this rate.")
        .changed();
    let Some(e) = state.editor.as_mut() else {
        return;
    };
    if changed {
        e.set_speed(SPEED_PRESETS[combo.selected].1, now);
    }
    let r = c.rect(x, y + 21.0 + 8.0 + c.speed_h + 8.0, w, 25.0);
    let on = e.preserve_pitch;
    if pitch_toggle(ui, c.t, r, on)
        .on_hover_text("Keep voices and music at their original pitch while changing speed.")
        .clicked()
    {
        e.set_preserve_pitch(!on, now);
    }
}

/// `QCheckBox#pitchToggle`: 14 px r6 indicator (SURFACE_2, TEXT_DIM
/// border; ACCENT when checked), 8 px spacing, 14 px label, full row.
fn pitch_toggle(ui: &mut Ui, t: &Tokens, r: Rect, on: bool) -> egui::Response {
    let resp = ui.interact(r, ui.scope_id().with("pitchToggle"), Sense::click());
    let b = Rect::from_min_size(
        Pos2::new(r.min.x, r.min.y + ((r.height() - 14.0) / 2.0).floor()),
        Vec2::splat(14.0),
    );
    let (fill, border) = if on {
        (Color::Accent, Color::Accent)
    } else if resp.hovered() {
        (Color::Surface2, Color::Accent)
    } else {
        (Color::Surface2, Color::TextDim)
    };
    let cr = CornerRadius::same(geom::RADIUS as u8);
    ui.painter().rect_filled(b, cr, t.color(fill));
    ui.painter()
        .rect_stroke(b, cr, Stroke::new(1.0, t.color(border)), StrokeKind::Inside);
    let tr = Rect::from_min_max(Pos2::new(b.max.x + 8.0, r.min.y), r.max);
    let g = galley(
        ui,
        "Preserve pitch",
        size::LABEL,
        Weight::W400,
        t.color(Color::Text),
    );
    paint_text(ui, g, tr, Align::Min, 0.0, t.color(Color::Text));
    fidelity::mark(ui, "pitchToggle", r)
        .text("Preserve pitch", size::LABEL, Weight::W400)
        .sample("accent", b.center(), fill)
        .done();
    resp
}

#[allow(clippy::too_many_arguments)]
fn edit_body(ui: &mut Ui, c: &mut Col<'_>, state: &mut AppState, x: f32, y: f32, w: f32, now: f64) {
    let t = c.t;
    let quick = state
        .settings
        .get("quick_crop")
        .and_then(QuickCrop::from_setting);
    let Some(e) = state.editor.as_ref() else {
        return;
    };
    let has_crop = e.crop.is_some();
    let saved = e.saved_feedback_until.is_some();
    let crop_text = if has_crop { "Edit crop" } else { "Set crop" };
    let wide = w - 62.0 - 6.0;
    let crop = outline(
        ui,
        t,
        c.rect(x, y, wide, 32.0),
        crop_text,
        "cropBtn",
        true,
        has_crop,
    );
    let quick_tip = match &quick {
        Some(q) => format!("Saved: {}×{} ({}×{} source)", q.w, q.h, q.src_w, q.src_h),
        None => "No quick crop saved yet".to_owned(),
    };
    let quick_btn = outline(
        ui,
        t,
        c.rect(x + wide + 6.0, y, 62.0, 32.0),
        "Quick",
        "quickCropBtn",
        quick.is_some(),
        false,
    )
    .on_hover_text(quick_tip);
    let save_text = if saved { "saved" } else { "Save as quick crop" };
    let save = outline(
        ui,
        t,
        c.rect(x, y + 40.0, w, 26.0),
        save_text,
        "saveQcBtn",
        has_crop,
        false,
    );
    let stretch = outline(
        ui,
        t,
        c.rect(x, y + 74.0, wide, 32.0),
        "Stretch video",
        "stretchBtn",
        true,
        false,
    )
    .on_hover_text("Open a crop-aware preview and drag either side");
    let reset = outline(
        ui,
        t,
        c.rect(x + wide + 6.0, y + 74.0, 62.0, 32.0),
        "Reset",
        "resetStretchBtn",
        true,
        false,
    );
    let info = e
        .crop
        .map(|(_, _, w, h)| format!("{w} × {h}"))
        .unwrap_or_default();
    let r = c.rect(x, y + 114.0, w, 17.0);
    label(
        ui,
        t,
        r,
        &info,
        size::MICRO,
        Weight::W400,
        Color::Accent,
        Align::Center,
        "",
    );
    fidelity::mark(ui, "cropInfoLbl", r).done();
    let editable = e.duration_ms() > 1;
    let Some(e) = state.editor.as_mut() else {
        return;
    };
    if crop.clicked() && editable {
        e.requests.push(EditorRequest::OpenCrop);
    }
    if stretch.clicked() && editable {
        e.requests.push(EditorRequest::OpenStretch);
    }
    if quick_btn.clicked() {
        e.apply_quick_crop(&state.settings, now);
    }
    if save.clicked() {
        e.save_quick_crop(&mut state.settings, now);
    }
    if reset.clicked() {
        e.reset_stretch(now);
    }
}

fn audio_body(ui: &mut Ui, c: &mut Col<'_>, state: &mut AppState, x: f32, y: f32, w: f32) {
    let t = c.t;
    let Some(e) = state.editor.as_ref() else {
        return;
    };
    let rows = e.source_rows();
    let master = e.master_volume;
    let loaded = e.player_loaded() || !rows.is_empty();
    let mut changes: Vec<(Option<String>, Option<u8>, Option<bool>)> = Vec::new();
    let all: Vec<(Option<&super::SourceRow>, String)> =
        std::iter::once((None, "MASTER".to_owned()))
            .chain(rows.iter().map(|r| (Some(r), r.label.clone())))
            .collect();
    for (n, (row, name)) in all.iter().enumerate() {
        let ry = y + n as f32 * (ROW_H + SPACING);
        let frame = c.rect(x, ry, w, ROW_H);
        ui.painter()
            .rect_filled(frame, 0.0, t.color(Color::Surface1));
        let ctl = c.names.uniq("audioMixControl");
        fidelity::mark(ui, ctl, frame).done();
        let value = row.map_or(master, |r| r.gain);
        let available = row.is_none_or(|r| r.available);
        let inner_x = x + 10.0;
        let inner_w = w - 20.0;
        let mute_w = if row.is_some() { 48.0 + 6.0 } else { 0.0 };
        let name_w = inner_w - 34.0 - 6.0 - mute_w;
        let r = c.rect(inner_x, ry + 8.0, name_w, 21.0);
        let nm = c.names.uniq("audioMixName");
        label(
            ui,
            t,
            r,
            name,
            size::LABEL,
            Weight::W400,
            Color::TextDim,
            Align::Min,
            &nm,
        );
        let vtext = if available {
            format!("{value}%")
        } else {
            "—".to_owned()
        };
        let r = c.rect(inner_x + name_w + 6.0, ry + 8.0, 34.0, 21.0);
        let nm = c.names.uniq("audioMixValue");
        label(
            ui,
            t,
            r,
            &vtext,
            size::LABEL,
            Weight::W400,
            Color::Accent,
            Align::Max,
            &nm,
        );
        if let Some(src) = row {
            let r = c.rect(inner_x + inner_w - 48.0, ry + 9.0, 48.0, 18.0);
            let enabled = available && loaded;
            let resp = ui.interact(
                r,
                ui.scope_id().with(("mute", &src.id)),
                if enabled {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            let text = if src.muted { "MUTED" } else { "MUTE" };
            let fg = if src.muted {
                Color::Delete
            } else if enabled && resp.hovered() {
                Color::Accent
            } else {
                Color::TextMuted
            };
            let nm = c.names.uniq("audioMixMute");
            label(
                ui,
                t,
                r,
                text,
                size::MICRO,
                Weight::W600,
                fg,
                Align::Center,
                &nm,
            );
            if resp.clicked() {
                changes.push((Some(src.id.clone()), None, Some(!src.muted)));
            }
        }
        let r = c.rect(inner_x, ry + 8.0 + 21.0 + 4.0, inner_w, 26.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        let mut v = f32::from(value);
        let nm = c.names.uniq("audioMixSlider");
        let tip = format!("{name} gain for this open clip only.");
        let resp = editor_slider(&mut child, t, &mut v, 0.0..=100.0, &nm).on_hover_text(tip);
        if resp.changed() && available {
            changes.push((row.map(|r| r.id.clone()), Some(v as u8), None));
        }
    }
    let Some(e) = state.editor.as_mut() else {
        return;
    };
    for (id, gain, muted) in changes {
        match id {
            None => {
                if let Some(g) = gain {
                    e.set_master(g, &mut state.settings);
                }
            }
            Some(id) => e.set_source(&id, gain, muted),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn look_body(ui: &mut Ui, c: &mut Col<'_>, state: &mut AppState, x: f32, y: f32, w: f32, now: f64) {
    let t = c.t;
    let Some(e) = state.editor.as_mut() else {
        return;
    };
    let mut any_down = false;
    for (n, ((key, lo, hi), title)) in EFFECT_LIMITS.iter().zip(EFFECT_TITLES).enumerate() {
        let ry = y + n as f32 * (EFFECT_H + SPACING);
        let frame = c.rect(x, ry, w, EFFECT_H);
        let nm = c.names.uniq("effectControl");
        fidelity::mark(ui, nm, frame).done();
        let value = e.effects.get(key);
        let g = galley(
            ui,
            title,
            size::LABEL,
            Weight::W400,
            t.color(Color::TextDim),
        );
        let r = c.rect(x, ry + 2.0, g.size().x.ceil(), 21.0);
        let nm = c.names.uniq("effectName");
        label(
            ui,
            t,
            r,
            title,
            size::LABEL,
            Weight::W400,
            Color::TextDim,
            Align::Min,
            &nm,
        );
        let vtext = if value == 0 {
            "0".to_owned()
        } else {
            format!("{value:+}")
        };
        let r = c.rect(x + w - 34.0, ry + 2.0, 34.0, 21.0);
        let nm = c.names.uniq("effectValue");
        label(
            ui,
            t,
            r,
            &vtext,
            size::LABEL,
            Weight::W400,
            Color::Accent,
            Align::Max,
            &nm,
        );
        let r = c.rect(x, ry + 2.0 + 21.0 + 3.0, w, 26.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        let mut v = value as f32;
        let nm = c.names.uniq("effectSlider");
        let resp = editor_slider(&mut child, t, &mut v, (*lo as f32)..=(*hi as f32), &nm)
            .on_hover_text(format!("Adjust {}", title.to_lowercase()));
        if resp.is_pointer_button_down_on() {
            any_down = true;
            e.begin_gesture();
        }
        if resp.changed() {
            e.set_effect(key, v as i32, now);
        }
    }
    if !any_down && e.in_gesture() && !ui.input(|i| i.pointer.primary_down()) {
        e.end_gesture(now);
    }
    let r = c.rect(x, y + 6.0 * (EFFECT_H + SPACING), w, 28.0);
    if outline(ui, t, r, "Reset look", "resetLookBtn", true, false).clicked() {
        e.reset_look(now);
    }
}

fn details_body(ui: &mut Ui, c: &mut Col<'_>, state: &mut AppState, x: f32, y: f32, w: f32) {
    let t = c.t;
    if state.clip_metadata.is_none() {
        state.clip_metadata =
            fthrust_core::clip_metadata::ClipMetadataStore::open(&state.paths).ok();
    }
    let Some(e) = state.editor.as_mut() else {
        return;
    };
    detail_key(ui, c, x, y, w, "Title", true);
    let stem = e
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = format::title(&stem);
    let r = c.rect(x, y + 31.0, w, 26.0);
    label(
        ui,
        t,
        r,
        &title,
        size::BODY_L,
        Weight::W600,
        Color::Text,
        Align::Min,
        "detailValBig",
    );
    let mut flush = false;
    for (n, (key, placeholder)) in [
        ("Tag", "e.g. Gaming, Highlight …"),
        ("Description", "Short clip description …"),
    ]
    .into_iter()
    .enumerate()
    {
        let ky = y + 67.0 + n as f32 * 65.0;
        detail_key(ui, c, x, ky, w, key, true);
        let r = c.rect(x, ky + 31.0, w, 24.0);
        let text = if n == 0 {
            &mut e.tag
        } else {
            &mut e.description
        };
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        let font = egui::FontId::new(
            size::MICRO,
            egui::FontFamily::Name(Weight::W400.family().into()),
        );
        let resp = child.add(
            egui::TextEdit::singleline(text)
                .frame(egui::Frame::NONE)
                .font(font)
                .text_color(t.color(Color::Text))
                .hint_text(egui::RichText::new(placeholder).color(t.color(Color::TextDim)))
                .desired_width(w)
                .margin(egui::Margin::symmetric(0, 2))
                .min_size(Vec2::new(w, 24.0)),
        );
        let line = if resp.has_focus() {
            Color::Accent
        } else {
            Color::TextDim
        };
        ui.painter()
            .hline(r.x_range(), r.max.y - 0.5, Stroke::new(1.0, t.color(line)));
        let nm = c.names.uniq(format!("detailEdit[{placeholder}]"));
        fidelity::mark(ui, nm, r)
            .text(placeholder, size::MICRO, Weight::W400)
            .done();
        if resp.changed() {
            e.details_dirty = true;
        }
        if resp.lost_focus() {
            flush = true;
        }
    }
    if flush && let Some(store) = state.clip_metadata.as_mut() {
        e.flush_details(store);
    }
}

fn file_body(ui: &mut Ui, c: &mut Col<'_>, state: &mut AppState, x: f32, y: f32, w: f32) {
    let t = c.t;
    let Some(e) = state.editor.as_ref() else {
        return;
    };
    if e.file_info.is_none() {
        let md = std::fs::metadata(&e.path).ok();
        let created = md
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(time::OffsetDateTime::from)
            .map(|d| {
                d.to_offset(time::UtcOffset::local_offset_at(d).unwrap_or(time::UtcOffset::UTC))
            })
            .unwrap_or_else(time::OffsetDateTime::now_utc);
        let size_mb = md.map_or(0.0, |m| m.len() as f64 / (1024.0 * 1024.0));
        if let Some(e) = state.editor.as_mut() {
            e.file_info = Some((created, size_mb));
        }
    }
    let Some(e) = state.editor.as_ref() else {
        return;
    };
    let (created, size_mb) = e
        .file_info
        .unwrap_or((time::OffsetDateTime::UNIX_EPOCH, 0.0));
    let (quality, vbr, tbr) = match e.meta() {
        MetaState::Loading => (
            "Loading…".to_owned(),
            "Loading…".to_owned(),
            "Loading…".to_owned(),
        ),
        MetaState::Unavailable => (
            "Unavailable".to_owned(),
            "Unavailable".to_owned(),
            "Unavailable".to_owned(),
        ),
        MetaState::Ready(m) => (
            match (m.width, m.height) {
                (Some(w), Some(h)) => format!(
                    "{w}x{h}, {}",
                    fthrust_media::probe::format_fps(m.average_fps)
                ),
                _ => "Unavailable".to_owned(),
            },
            fthrust_media::probe::format_bitrate(m.video_bitrate_bps),
            fthrust_media::probe::format_bitrate(m.total_bitrate_bps),
        ),
    };
    let location = format::location(
        &e.path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let rows = [
        ("Created", format::created(created)),
        ("Video Quality", quality),
        ("Video Bitrate", vbr),
        ("Total Bitrate", tbr),
        ("Duration", format::clock(e.duration_ms())),
        ("Size", format!("{size_mb:.1} MB")),
        ("Location", location),
    ];
    for (n, (key, value)) in rows.iter().enumerate() {
        let ky = y + n as f32 * 64.0;
        detail_key(ui, c, x, ky, w, key, true);
        let r = c.rect(x, ky + 31.0, w, 23.0);
        let nm = c.names.uniq(format!("detailVal[{value}]"));
        label(
            ui,
            t,
            r,
            value,
            size::BODY,
            Weight::W400,
            Color::Text,
            Align::Min,
            &nm,
        );
    }
}

fn delete_button(ui: &mut Ui, c: &mut Col<'_>, state: &mut AppState, y: f32) {
    let Some(e) = state.editor.as_ref() else {
        return;
    };
    let linked = match e.linked_import {
        Some(v) => v,
        None => {
            let v = is_linked_import(state, &e.path);
            if let Some(e) = state.editor.as_mut() {
                e.linked_import = Some(v);
            }
            v
        }
    };
    let r = c.rect(0.0, y, c.w, DELETE_H);
    let text = if linked {
        "Linked original — protected"
    } else {
        "Delete"
    };
    let (border, fg) = if linked {
        (Color::Border, Color::TextMuted)
    } else {
        (Color::Delete, Color::Delete)
    };
    let mut resp = button(
        ui,
        c.t,
        r,
        text,
        "deleteBtn",
        size::BODY_L,
        (None, Some(border), fg),
        (Some(Color::Delete), None, Some(Color::Text)),
        !linked,
    );
    if linked {
        resp =
            resp.on_hover_text("Remove its import folder from FTHR; the original stays on disk.");
    }
    if resp.clicked()
        && let Some(e) = state.editor.as_mut()
    {
        e.requests.push(EditorRequest::DeleteClip);
    }
}
