//! The trim bar: header row, tool row and the 116 px timeline widget
//! (`TrimSlider` paint and mouse handling, `clip_viewer.py:1045-1700`,
//! header construction `:4984`).

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use fthrust_core::editor::format;

use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::Icon;
use crate::state::AppState;
use crate::tokens::{Color, Tokens, geom, size};
use crate::widgets::buttons::{IconButtonStyle, icon_button};
use crate::widgets::{galley, paint_text};

/// Timeline widget height.
pub const WIDGET_H: f32 = 116.0;
/// Horizontal padding inside the widget.
const PAD: f32 = 14.0;
/// Track top and height inside the widget.
const TY: f32 = 22.0;
const TH: f32 = 70.0;
/// Minimap.
const OVERVIEW_Y: f32 = 103.0;
const OVERVIEW_H: f32 = 7.0;
/// Presses at or below this y (widget-local) hit the minimap.
const MINIMAP_PRESS_Y: f32 = 98.0;
/// Trim-handle grab radius.
const HANDLE_GRAB_PX: f32 = 9.0;
/// Tool row.
const ROW_Y: f32 = 38.0;
const ROW_H: f32 = 26.0;

/// What a primary press on the widget started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    Start,
    End,
    Playhead,
}

/// Draws the trim bar contents into `rect` (the 198 px bar).
pub fn ui(ui: &mut Ui, rect: Rect, state: &mut AppState) {
    if state.editor.is_none() {
        return;
    }
    let now = state.editor_now();
    let t = state.tokens.clone();
    header_row(ui, &t, rect, state);
    tool_row(ui, &t, rect, state, now);
    let widget = Rect::from_min_size(
        Pos2::new(rect.min.x + 16.0, rect.min.y + 70.0),
        Vec2::new(rect.width() - 32.0, WIDGET_H),
    );
    upload_filmstrip(ui, state);
    timeline(ui, &t, widget, state, now);
}

fn header_row(ui: &mut Ui, t: &Tokens, rect: Rect, state: &AppState) {
    let Some(e) = state.editor.as_ref() else {
        return;
    };
    let g = galley(
        ui,
        "Timeline",
        size::MICRO,
        Weight::W600,
        t.color(Color::TextDim),
    );
    let r = Rect::from_min_size(
        Pos2::new(rect.min.x + 16.0, rect.min.y + 11.0),
        Vec2::new(g.size().x.ceil(), 21.0),
    );
    paint_text(ui, g, r, Align::Min, 0.0, t.color(Color::TextDim));
    fidelity::mark(ui, "trimLabel", r)
        .text("Timeline", size::MICRO, Weight::W600)
        .done();
    let label = e.timeline_label();
    let g = galley(
        ui,
        &label,
        size::LABEL,
        Weight::W400,
        t.color(Color::Accent),
    );
    let w = g.size().x.ceil();
    let r = Rect::from_min_size(
        Pos2::new(rect.max.x - 16.0 - w, rect.min.y + 11.0),
        Vec2::new(w, 21.0),
    );
    paint_text(ui, g, r, Align::Min, 0.0, t.color(Color::Accent));
    fidelity::mark(ui, "trimRangeLbl", r)
        .text(label, size::LABEL, Weight::W400)
        .done();
}

/// The bordered 26 px text buttons of the tool row.
#[derive(Clone, Copy)]
enum Style {
    History,
    Action,
    Danger { restore: bool },
}

fn text_button(
    ui: &mut Ui,
    t: &Tokens,
    r: Rect,
    text: &str,
    name: &str,
    style: Style,
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
    let (bg, mut border, mut fg) = match style {
        Style::History => (None, Color::BorderHi, Color::Text),
        Style::Action => (Some(Color::Surface2), Color::BorderHi, Color::Text),
        Style::Danger { restore: true } => (Some(Color::Surface2), Color::Accent, Color::Accent),
        Style::Danger { restore: false } => (Some(Color::Surface2), Color::BorderHi, Color::Text),
    };
    if hovered {
        let c = if matches!(style, Style::Danger { restore: false }) {
            Color::Delete
        } else {
            Color::Accent
        };
        border = c;
        fg = c;
    }
    if !enabled {
        border = Color::Border;
        fg = Color::TextMuted;
    }
    let radius = CornerRadius::same(geom::RADIUS as u8);
    if let Some(bg) = bg {
        ui.painter().rect_filled(r, radius, t.color(bg));
    }
    ui.painter().rect_stroke(
        r,
        radius,
        Stroke::new(1.0, t.color(border)),
        StrokeKind::Inside,
    );
    let g = galley(ui, text, size::MICRO, Weight::W600, t.color(fg));
    paint_text(ui, g, r, Align::Center, 0.0, t.color(fg));
    let mut m = fidelity::mark(ui, name, r)
        .text(text, size::MICRO, Weight::W600)
        .sample("top", Pos2::new(r.center().x, r.min.y), border);
    if let Some(bg) = bg {
        m = m.sample("fill", Pos2::new(r.min.x + 3.0, r.center().y), bg);
    }
    m.done();
    resp
}

fn action_width(ui: &Ui, text: &str) -> f32 {
    // padding 9 + border 1 on each side.
    galley(ui, text, size::MICRO, Weight::W600, Color32::WHITE)
        .size()
        .x
        .ceil()
        + 20.0
}

fn tool_row(ui: &mut Ui, t: &Tokens, rect: Rect, state: &mut AppState, now: f64) {
    let y = rect.min.y + ROW_Y;
    let Some(e) = state.editor.as_ref() else {
        return;
    };
    let (can_undo, can_redo) = (e.undo.can_undo(), e.undo.can_redo());
    let (seg_text, seg_enabled, restore) = e.segment_button();
    let split_text = e.split_label();
    let zoom_text = format!("{}%", (e.timeline.zoom * 100.0).round() as i64);
    let editable = e.duration_ms() > 1;
    let mut x = rect.min.x + 16.0;
    let undo = text_button(
        ui,
        t,
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(66.0, ROW_H)),
        "Undo",
        "timelineHistory[Undo]",
        Style::History,
        can_undo,
    )
    .on_hover_text("Undo last edit  ·  Ctrl+Z");
    x += 66.0 + 6.0;
    let redo = text_button(
        ui,
        t,
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(66.0, ROW_H)),
        "Redo",
        "timelineHistory[Redo]",
        Style::History,
        can_redo,
    )
    .on_hover_text("Redo edit  ·  Ctrl+Y or Ctrl+Shift+Z");
    x += 66.0 + 6.0 + 8.0;
    let tool_style = IconButtonStyle {
        size: Vec2::splat(ROW_H),
        bg: Some(Color::Surface2),
        hover_bg: None,
        pressed_bg: None,
        border: Some(Color::Border),
        hover_border: Some(Color::Accent),
        radius: geom::RADIUS,
        hover_tint: Some(Color::Accent),
        enabled: true,
    };
    let mut tool = |ui: &mut Ui, x: f32, icon: Icon, name: &str, tip: &str| {
        let r = Rect::from_min_size(Pos2::new(x, y), Vec2::splat(ROW_H));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        icon_button(
            &mut child,
            t,
            &mut state.icons,
            icon,
            14.0,
            tool_style,
            name,
        )
        .on_hover_text(tip)
        .clicked()
    };
    let pan_left = tool(ui, x, Icon::StepBack, "timelineTool", "Pan timeline left");
    x += ROW_H + 6.0;
    let zoom_out = tool(ui, x, Icon::Decrease, "timelineTool#2", "Zoom out");
    x += ROW_H + 6.0;
    let zr = Rect::from_min_size(Pos2::new(x, y), Vec2::new(44.0, ROW_H));
    let g = galley(
        ui,
        &zoom_text,
        size::MICRO,
        Weight::W400,
        t.color(Color::TextDim),
    );
    paint_text(ui, g, zr, Align::Center, 0.0, t.color(Color::TextDim));
    fidelity::mark(ui, "timelineZoomLabel", zr)
        .text(zoom_text.clone(), size::MICRO, Weight::W400)
        .done();
    x += 44.0 + 6.0;
    let zoom_in = tool(ui, x, Icon::Increase, "timelineTool#3", "Zoom in");
    x += ROW_H + 6.0;
    let pan_right = tool(
        ui,
        x,
        Icon::StepForward,
        "timelineTool#4",
        "Pan timeline right",
    );
    // Right side: Split, then the segment button flush right.
    let seg_w = action_width(ui, seg_text);
    let seg_r = Rect::from_min_size(
        Pos2::new(rect.max.x - 16.0 - seg_w, y),
        Vec2::new(seg_w, ROW_H),
    );
    let split_w = action_width(ui, split_text);
    let split_r = Rect::from_min_size(
        Pos2::new(seg_r.min.x - 6.0 - split_w, y),
        Vec2::new(split_w, ROW_H),
    );
    let split = text_button(
        ui,
        t,
        split_r,
        split_text,
        "timelineAction",
        Style::Action,
        editable,
    )
    .on_hover_text("Split the selected segment at the playhead");
    let toggle = text_button(
        ui,
        t,
        seg_r,
        seg_text,
        "timelineActionDanger",
        Style::Danger { restore },
        seg_enabled && editable,
    )
    .on_hover_text("Select a split segment, then remove it from the edit");
    let Some(e) = state.editor.as_mut() else {
        return;
    };
    if undo.clicked() {
        e.undo(now);
    }
    if redo.clicked() {
        e.redo(now);
    }
    if pan_left {
        e.timeline.pan(-1.0);
    }
    if pan_right {
        e.timeline.pan(1.0);
    }
    if zoom_out {
        let z = e.timeline.zoom * 0.8;
        e.timeline.set_zoom(z, None);
    }
    if zoom_in {
        let z = e.timeline.zoom * 1.25;
        e.timeline.set_zoom(z, None);
    }
    if split.clicked() {
        e.split(now);
    }
    if toggle.clicked() {
        e.toggle_selected(now);
    }
}

fn upload_filmstrip(ui: &Ui, state: &mut AppState) {
    let Some(frames) = state.editor.as_mut().and_then(|e| e.filmstrip.take_ready()) else {
        return;
    };
    state.editor_filmstrip = frames
        .into_iter()
        .enumerate()
        .map(|(i, img)| {
            ui.ctx()
                .load_texture(format!("filmstrip-{i}"), img, egui::TextureOptions::LINEAR)
        })
        .collect();
}

/// Display fraction → x in the widget.
fn x_for_display(w: Rect, view_start: f64, span: f64, d: f64) -> f32 {
    let tw = w.width() - 2.0 * PAD;
    w.min.x + PAD + ((d - view_start) / span * f64::from(tw)) as i32 as f32
}

fn timeline(ui: &mut Ui, t: &Tokens, w: Rect, state: &mut AppState, now: f64) {
    let id = ui.scope_id().with("trimSlider");
    let resp = ui.interact(w, id, Sense::click_and_drag());
    input(ui, &resp, w, state, now);
    paint(ui, t, w, state);
}

fn input(ui: &mut Ui, resp: &egui::Response, w: Rect, state: &mut AppState, now: f64) {
    let drag_id = resp.id.with("drag");
    let tw = f64::from(w.width() - 2.0 * PAD);
    let Some(e) = state.editor.as_mut() else {
        return;
    };
    if e.duration_ms() <= 1 {
        return;
    }
    let (pressed, released, down, pos, wheel) = ui.input(|i| {
        let mut wheel: Option<(f32, bool)> = None;
        for ev in &i.events {
            if let egui::Event::MouseWheel {
                delta, modifiers, ..
            } = ev
            {
                let d = if delta.y != 0.0 { delta.y } else { delta.x };
                wheel = Some((d, modifiers.shift));
            }
        }
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_released(),
            i.pointer.primary_down(),
            i.pointer.interact_pos(),
            wheel,
        )
    });
    let view_pct = |x: f32, tl: &fthrust_core::editor::timeline::Timeline| -> f64 {
        let local = (f64::from(x - w.min.x - PAD) / tw.max(1.0)).clamp(0.0, 1.0);
        tl.display_to_source(tl.view_start + local * tl.span())
    };
    let overview_pct = |x: f32, tl: &fthrust_core::editor::timeline::Timeline| -> f64 {
        tl.display_to_source((f64::from(x - w.min.x - PAD) / tw.max(1.0)).clamp(0.0, 1.0))
    };
    // Wheel: zoom at the cursor; Shift pans.
    if let (Some((d, shift)), true) = (wheel, resp.hovered())
        && d != 0.0
    {
        if shift {
            e.timeline.pan(if d > 0.0 { -1.0 } else { 1.0 });
        } else if let Some(p) = pos {
            let anchor = view_pct(p.x, &e.timeline);
            let z = e.timeline.zoom * if d > 0.0 { 1.25 } else { 0.8 };
            e.timeline.set_zoom(z, Some(anchor));
        }
    }
    // The drag and the pointer x it last acted on: handles and the playhead
    // move only when the pointer does (`mouseMoveEvent`), never on a
    // press-and-release in place.
    let mut drag: Option<(Drag, f32)> = ui.data(|d| d.get_temp(drag_id));
    if pressed
        && resp.hovered()
        && let Some(p) = pos
    {
        if p.y - w.min.y >= MINIMAP_PRESS_Y {
            let pct = overview_pct(p.x, &e.timeline);
            let span = e.timeline.span();
            let d = e.timeline.source_to_display(pct);
            e.timeline.view_start = (d - span / 2.0).min(1.0 - span).max(0.0);
            let index = e.timeline.segment_index_at(pct).unwrap_or(0);
            e.select(index);
            e.scrub_to(pct, now);
            drag = None;
        } else {
            let pct = view_pct(p.x, &e.timeline);
            let tl = &e.timeline;
            let px_per = (tw / tl.span()).max(1.0);
            let dp = tl.source_to_display(pct);
            let ds = (dp - tl.source_to_display(tl.trim_start)).abs() * px_per;
            let de = (dp - tl.source_to_display(tl.trim_end)).abs() * px_per;
            if ds <= f64::from(HANDLE_GRAB_PX) {
                drag = Some((Drag::Start, p.x));
                e.begin_gesture();
            } else if de <= f64::from(HANDLE_GRAB_PX) {
                drag = Some((Drag::End, p.x));
                e.begin_gesture();
            } else {
                drag = Some((Drag::Playhead, p.x));
                e.select_at(pct);
                e.scrub_to(pct, now);
            }
        }
    } else if let (Some((mode, last_x)), Some(p)) = (drag, pos)
        && (down || released)
        && p.x != last_x
    {
        let pct = view_pct(p.x, &e.timeline);
        match mode {
            Drag::Playhead => e.scrub_to(pct, now),
            Drag::Start => {
                e.timeline.set_trim_start(pct);
                e.on_trim_changed(now);
            }
            Drag::End => {
                e.timeline.set_trim_end(pct);
                e.on_trim_changed(now);
            }
        }
        drag = Some((mode, p.x));
    }
    if released || !down {
        if matches!(drag, Some((Drag::Start | Drag::End, _))) {
            e.end_gesture(now);
        }
        drag = None;
    }
    ui.data_mut(|d| match drag {
        Some(m) => {
            d.insert_temp(drag_id, m);
        }
        None => d.remove::<(Drag, f32)>(drag_id),
    });
    // Context menu: targets the clicked position, not the stale selection.
    if resp.secondary_clicked()
        && let Some(p) = pos
    {
        let pct = if p.y - w.min.y >= MINIMAP_PRESS_Y {
            overview_pct(p.x, &e.timeline)
        } else {
            view_pct(p.x, &e.timeline)
        };
        let (index, pct) = match e.timeline.segment_index_at(pct) {
            Some(i) => (i, pct),
            None => {
                let i = e.timeline.selected.min(e.timeline.segments.len() - 1);
                (i, e.timeline.segments[i].start)
            }
        };
        e.select(index);
        e.scrub_to(pct, now);
        e.context_pct = Some(pct);
    }
    resp.context_menu(|ui| context_menu(ui, state, now));
}

fn context_menu(ui: &mut Ui, state: &mut AppState, now: f64) {
    let (Some(e), settings) = (state.editor.as_mut(), &mut state.settings) else {
        return;
    };
    let Some(pct) = e.context_pct else {
        ui.close();
        return;
    };
    let index = e.timeline.selected;
    let deleted = e.timeline.segments.get(index).is_some_and(|s| s.deleted);
    let can_toggle = e.timeline.can_toggle_selected();
    if ui
        .add_enabled(e.timeline.can_split(pct), egui::Button::new("Split here"))
        .clicked()
    {
        e.split_at(pct, now);
        ui.close();
    }
    let toggle_text = if deleted {
        "Restore segment"
    } else {
        "Delete segment"
    };
    if ui
        .add_enabled(can_toggle, egui::Button::new(toggle_text))
        .clicked()
    {
        e.toggle_segment(index, now);
        ui.close();
    }
    ui.separator();
    let mut hide = e.timeline.hide_deleted;
    if ui.checkbox(&mut hide, "Hide deleted segments").changed() {
        e.set_hide_deleted(hide, settings);
        ui.close();
    }
}

fn paint(ui: &mut Ui, t: &Tokens, w: Rect, state: &AppState) {
    let Some(e) = state.editor.as_ref() else {
        return;
    };
    let tl = &e.timeline;
    let p = ui.painter_at(w);
    let tw = w.width() - 2.0 * PAD;
    let x0 = w.min.x + PAD;
    let ty = w.min.y + TY;
    let span = tl.span();
    let view_end = tl.view_start + span;
    let x_for = |pct: f64| x_for_display(w, tl.view_start, span, tl.source_to_display(pct));
    // Ruler: 5 ticks, retained time when deleted spans are hidden.
    let retained = if tl.hide_deleted {
        tl.segments
            .iter()
            .filter(|s| !s.deleted)
            .map(|s| s.end - s.start)
            .sum::<f64>()
    } else {
        1.0
    };
    for tick in 0..5 {
        let pct = tl.view_start + span * f64::from(tick) / 4.0;
        let x = x0 + (tw * tick as f32 / 4.0).trunc();
        let text = format::clock((pct * e.duration_ms() as f64 * retained) as u64);
        let g = galley(ui, &text, 9.0, Weight::W400, t.color(Color::TextDim));
        let gw = g.size().x;
        let lx = match tick {
            0 => x - 34.0,
            4 => x + 34.0 - gw,
            _ => x - gw / 2.0,
        };
        p.galley(
            Pos2::new(lx, w.min.y + 2.0 + (16.0 - g.size().y) / 2.0),
            g,
            t.color(Color::TextDim),
        );
    }
    // Track.
    let track = Rect::from_min_size(Pos2::new(x0, ty), Vec2::new(tw, TH));
    fidelity::mark(ui, "timelineTrack", track).done();
    p.rect_filled(track, 0.0, t.color(Color::Surface2));
    p.rect_stroke(
        track,
        0.0,
        Stroke::new(1.0, t.color(Color::BorderHi)),
        StrokeKind::Inside,
    );
    // Filmstrip slots (frames sampled in source time).
    if e.duration_ms() > 1 {
        let slots = ((tw as i32) / 92).clamp(4, 14) as usize;
        let frames = &state.editor_filmstrip;
        for slot in 0..slots {
            let left = x0 + (slot as f32 * tw / slots as f32).trunc();
            let right = x0 + ((slot + 1) as f32 * tw / slots as f32).trunc();
            if !frames.is_empty() {
                let d = tl.view_start + span * (slot as f64 + 0.5) / slots as f64;
                let pct = tl.display_to_source(d);
                let i = ((pct * frames.len() as f64) as usize).min(frames.len() - 1);
                let target = Rect::from_min_max(
                    Pos2::new(left, ty),
                    Pos2::new(right.max(left + 1.0), ty + TH),
                );
                // Expanding fit: crop the 180×100 frame to the slot's aspect.
                let fa = 180.0 / 100.0;
                let sa = target.width() / target.height();
                let uv = if sa < fa {
                    let u = sa / fa;
                    Rect::from_min_max(Pos2::new(0.5 - u / 2.0, 0.0), Pos2::new(0.5 + u / 2.0, 1.0))
                } else {
                    let v = fa / sa;
                    Rect::from_min_max(Pos2::new(0.0, 0.5 - v / 2.0), Pos2::new(1.0, 0.5 + v / 2.0))
                };
                p.image(frames[i].id(), target, uv, Color32::WHITE);
            }
            let line = t.color(Color::Text);
            p.vline(
                right + 0.5,
                ty..=ty + TH,
                Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(line.r(), line.g(), line.b(), 24),
                ),
            );
        }
    }
    // Outside the trim.
    let bg = t.color(Color::Bg);
    let dim = Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 176);
    if tl.source_to_display(tl.trim_start) > tl.view_start {
        let r = (x_for(tl.trim_start) - x0).clamp(0.0, tw);
        p.rect_filled(
            Rect::from_min_size(Pos2::new(x0, ty), Vec2::new(r, TH)),
            0.0,
            dim,
        );
    }
    if tl.source_to_display(tl.trim_end) < view_end {
        let ex = x_for(tl.trim_end).clamp(x0, x0 + tw);
        p.rect_filled(
            Rect::from_min_max(Pos2::new(ex, ty), Pos2::new(x0 + tw, ty + TH)),
            0.0,
            dim,
        );
    }
    // Segments: deleted shading and hatch, selection outline, cut lines.
    let s2 = t.color(Color::Surface2);
    let deleted_fill = Color32::from_rgba_unmultiplied(s2.r(), s2.g(), s2.b(), 218);
    for (index, seg) in tl.segments.iter().enumerate() {
        if tl.hide_deleted && seg.deleted {
            continue;
        }
        let lp = tl.source_to_display(seg.start).max(tl.view_start);
        let rp = tl.source_to_display(seg.end).min(view_end);
        if rp <= lp {
            continue;
        }
        let left = x_for_display(w, tl.view_start, span, lp).max(x0);
        let right = x_for_display(w, tl.view_start, span, rp).min(x0 + tw);
        let r = Rect::from_min_max(
            Pos2::new(left, ty),
            Pos2::new(right.max(left + 1.0), ty + TH),
        );
        if seg.deleted {
            p.rect_filled(r, 0.0, deleted_fill);
            let clip = ui.painter_at(r.intersect(w));
            let mut hx = left - TH;
            while hx < right + TH {
                clip.line_segment(
                    [Pos2::new(hx, ty + TH), Pos2::new(hx + TH, ty)],
                    Stroke::new(1.0, t.color(Color::Delete)),
                );
                hx += 10.0;
            }
        }
        if index == tl.selected {
            let c = if seg.deleted {
                Color::Delete
            } else {
                Color::Accent
            };
            let outline = Rect::from_min_size(
                Pos2::new(left + 1.0, ty + 1.0),
                Vec2::new((right - left - 2.0).max(1.0), TH - 2.0),
            );
            p.rect_stroke(
                outline,
                0.0,
                Stroke::new(2.0, t.color(c)),
                StrokeKind::Middle,
            );
        }
        if tl.source_to_display(seg.start) > tl.view_start {
            let cx = x_for(seg.start) + 0.5;
            p.vline(
                cx,
                ty + 8.0..=ty + TH - 8.0,
                Stroke::new(1.0, t.color(Color::Text)),
            );
        }
    }
    // Trim handles.
    for pct in [tl.trim_start, tl.trim_end] {
        let d = tl.source_to_display(pct);
        if tl.view_start <= d && d <= view_end {
            let x = x_for(pct);
            p.rect_filled(
                Rect::from_min_size(Pos2::new(x - 4.0, ty - 3.0), Vec2::new(8.0, TH + 6.0)),
                0.0,
                t.color(Color::Accent),
            );
        }
    }
    // Minimap: whole clip, deleted ranges, viewport box.
    let oy = w.min.y + OVERVIEW_Y;
    p.rect_filled(
        Rect::from_min_size(Pos2::new(x0, oy), Vec2::new(tw, OVERVIEW_H)),
        0.0,
        t.color(Color::BorderHi),
    );
    if !tl.hide_deleted {
        for seg in tl.segments.iter().filter(|s| s.deleted) {
            p.rect_filled(
                Rect::from_min_size(
                    Pos2::new(x0 + (seg.start as f32 * tw).trunc(), oy),
                    Vec2::new(
                        ((seg.end - seg.start) as f32 * tw).trunc().max(1.0),
                        OVERVIEW_H,
                    ),
                ),
                0.0,
                t.color(Color::Delete),
            );
        }
    }
    p.rect_stroke(
        Rect::from_min_size(
            Pos2::new(x0 + (tl.view_start as f32 * tw).trunc(), oy),
            Vec2::new((span as f32 * tw).trunc().max(3.0), OVERVIEW_H),
        ),
        0.0,
        Stroke::new(1.0, t.color(Color::Accent)),
        StrokeKind::Inside,
    );
    // Playhead.
    let d = tl.source_to_display(tl.playhead);
    if tl.view_start <= d && d <= view_end {
        let x = x_for(tl.playhead);
        p.line_segment(
            [Pos2::new(x, ty - 7.0), Pos2::new(x, ty + TH + 5.0)],
            Stroke::new(2.0, t.color(Color::Text)),
        );
        p.add(Shape::convex_polygon(
            vec![
                Pos2::new(x - 5.0, ty - 7.0),
                Pos2::new(x + 5.0, ty - 7.0),
                Pos2::new(x, ty - 1.0),
            ],
            t.color(Color::Text),
            Stroke::NONE,
        ));
    }
}
