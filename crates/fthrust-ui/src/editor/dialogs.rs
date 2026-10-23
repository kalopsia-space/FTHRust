//! The editor's modal crop and stretch dialogs (`CropDialog` +
//! `CropOverlay`, `clip_viewer.py:1699` and `:3084`; `StretchDialog` +
//! `StretchOverlay`, `:2771` and `:2905`). Playback pauses while one is
//! open and resumes after it closes if it was playing. Each accept is one
//! undo step.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use egui::{
    Align, Area, Color32, ColorImage, CornerRadius, Id, Order, Pos2, Rect, Sense, Stroke,
    StrokeKind, TextureHandle, Ui, Vec2,
};

use super::{EditorRequest, Lifecycle};
use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::Icon;
use crate::state::AppState;
use crate::tokens::{Color, Tokens, geom};
use crate::widgets::buttons::{IconButtonStyle, icon_button};
use crate::widgets::{galley, paint_text};

/// Crop dialog size.
pub const CROP_SIZE: Vec2 = Vec2::new(900.0, 580.0);
/// Stretch dialog size.
pub const STRETCH_SIZE: Vec2 = Vec2::new(760.0, 540.0);
/// Crop handle square and hit zone.
const HANDLE: f32 = 9.0;
const HANDLE_HIT: f32 = 14.0;
/// Stretch side-handle hit distance and drag scale (px per 1.0 ratio).
const STRETCH_HIT: f32 = 18.0;
const STRETCH_PX: f32 = 180.0;

/// A crop in source pixels.
type Crop = (u32, u32, u32, u32);
/// A dialog's outcome: `Ok(crop)` from the crop dialog, `Err(ratio)` from
/// the stretch dialog; `None` cancels.
type Outcome = Option<Result<Option<Crop>, f64>>;

/// The decoded preview frame.
enum FrameState {
    Loading(Receiver<Option<ColorImage>>),
    Failed,
    Ready(TextureHandle, [usize; 2]),
}

impl FrameState {
    fn start(path: PathBuf, position_ms: u64, crop: Option<(u32, u32, u32, u32)>) -> FrameState {
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("editor-dialog-frame".into())
            .spawn(move || {
                let _ = tx.send(decode(&path, position_ms, crop));
            });
        match spawned {
            Ok(_) => FrameState::Loading(rx),
            Err(_) => FrameState::Failed,
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let FrameState::Loading(rx) = self
            && let Ok(img) = rx.try_recv()
        {
            *self = match img {
                Some(img) => {
                    let size = img.size;
                    let tex =
                        ctx.load_texture("editor-dialog-frame", img, egui::TextureOptions::LINEAR);
                    FrameState::Ready(tex, size)
                }
                None => FrameState::Failed,
            };
        }
    }

    fn loaded(&self) -> bool {
        matches!(self, FrameState::Ready(..))
    }
}

/// One frame at `position_ms` (else the first), cropped.
pub(crate) fn decode(
    path: &Path,
    position_ms: u64,
    crop: Option<(u32, u32, u32, u32)>,
) -> Option<ColorImage> {
    let f = fthrust_media::playback::video::frame_at(path, position_ms)
        .or_else(|_| fthrust_media::playback::video::frame_at(path, 0))
        .ok()?;
    let mut img = image::RgbaImage::from_raw(f.width, f.height, f.rgba)?;
    if let Some((x, y, w, h)) = crop {
        let x = x.min(img.width().saturating_sub(1));
        let y = y.min(img.height().saturating_sub(1));
        let w = w.min(img.width() - x).max(1);
        let h = h.min(img.height() - y).max(1);
        img = image::imageops::crop_imm(&img, x, y, w, h).to_image();
    }
    Some(ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    ))
}

/// What a crop-overlay press grabbed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CropDrag {
    Handle(usize),
    Move,
    New,
}

/// The open crop dialog.
pub struct CropDialog {
    frame: FrameState,
    /// Crop in display pixels (relative to the frame label).
    display: Option<Rect>,
    /// Crop in source pixels.
    pub crop: Option<(u32, u32, u32, u32)>,
    initial: Option<(u32, u32, u32, u32)>,
    drag: Option<(CropDrag, Pos2, Rect)>,
}

/// The open stretch dialog.
pub struct StretchDialog {
    frame: FrameState,
    /// Current ratio.
    pub ratio: f64,
    base: (u32, u32),
    drag: Option<(bool, f32, f64)>,
}

/// An open editor dialog.
pub enum EditorDialog {
    /// Set/Edit crop.
    Crop(CropDialog),
    /// Stretch video.
    Stretch(StretchDialog),
}

/// Opens the dialogs the editor asked for (pausing playback).
pub fn service(state: &mut AppState) {
    let now = state.editor_now();
    let Some(e) = state.editor.as_mut() else {
        state.editor_dialog = None;
        return;
    };
    let mut open = None;
    e.requests.retain(|r| match r {
        EditorRequest::OpenCrop | EditorRequest::OpenStretch => {
            open = Some(r.clone());
            false
        }
        _ => true,
    });
    let Some(kind) = open else {
        return;
    };
    if state.editor_dialog.is_some() {
        return;
    }
    state.editor_resume_after_dialog = e.lifecycle == Lifecycle::Playing;
    if state.editor_resume_after_dialog {
        e.toggle_play(now);
    }
    let path = e.path.clone();
    state.editor_dialog = Some(match kind {
        EditorRequest::OpenCrop => EditorDialog::Crop(CropDialog {
            frame: FrameState::start(path, 0, None),
            display: None,
            crop: e.crop,
            initial: e.crop,
            drag: None,
        }),
        _ => {
            let (sw, sh) = e.source_size();
            let base = e.crop.map_or((sw, sh), |(_, _, w, h)| (w, h));
            EditorDialog::Stretch(StretchDialog {
                frame: FrameState::start(path, e.position_ms(), e.crop),
                ratio: e.stretch,
                base: (base.0.max(1), base.1.max(1)),
                drag: None,
            })
        }
    });
}

fn finish(state: &mut AppState, accepted: Outcome) {
    let now = state.editor_now();
    if let Some(e) = state.editor.as_mut() {
        match accepted {
            Some(Ok(crop)) => e.set_crop(crop, now),
            Some(Err(ratio)) => e.set_stretch(ratio, now),
            None => {}
        }
        if std::mem::take(&mut state.editor_resume_after_dialog) {
            e.toggle_play(now);
        }
    }
    state.editor_dialog = None;
}

/// Draws the open dialog (modal, centred) and applies its outcome.
pub fn ui(ctx: &egui::Context, state: &mut AppState) {
    let Some(dialog) = state.editor_dialog.as_mut() else {
        return;
    };
    let screen = ctx.content_rect();
    let t = state.tokens.clone();
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    // Block the editor underneath.
    Area::new(Id::unique("editor-dialog-blocker"))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .constrain(false)
        .interactable(true)
        .show(ctx, |ui| {
            ui.allocate_rect(screen, Sense::click_and_drag());
            ui.painter()
                .rect_filled(screen, 0.0, Color32::from_black_alpha(120));
        });
    let size = match dialog {
        EditorDialog::Crop(_) => CROP_SIZE,
        EditorDialog::Stretch(_) => STRETCH_SIZE,
    };
    let pos = (screen.center() - size / 2.0).max(screen.min).round();
    let mut outcome: Option<Outcome> = None;
    Area::new(Id::unique("editor-dialog"))
        .order(Order::Tooltip)
        .fixed_pos(pos)
        .constrain(false)
        .interactable(true)
        .show(ctx, |ui| {
            let rect = Rect::from_min_size(pos, size);
            outcome = match dialog {
                EditorDialog::Crop(d) => crop_ui(ui, &t, &mut state.icons, rect, d),
                EditorDialog::Stretch(d) => stretch_ui(ui, &t, &mut state.icons, rect, d),
            };
        });
    if escape && outcome.is_none() {
        outcome = Some(None);
    }
    if let Some(result) = outcome {
        finish(state, result);
    }
}

/// Draws a crop dialog into `rect` (tests render it standalone).
pub fn draw_crop(ui: &mut Ui, state: &mut AppState, rect: Rect) {
    let t = state.tokens.clone();
    if let Some(EditorDialog::Crop(d)) = state.editor_dialog.as_mut() {
        crop_ui(ui, &t, &mut state.icons, rect, d);
    }
}

/// Draws a stretch dialog into `rect` (tests render it standalone).
pub fn draw_stretch(ui: &mut Ui, state: &mut AppState, rect: Rect) {
    let t = state.tokens.clone();
    if let Some(EditorDialog::Stretch(d)) = state.editor_dialog.as_mut() {
        stretch_ui(ui, &t, &mut state.icons, rect, d);
    }
}

/// The open dialog's frame has decoded.
pub fn frame_loaded(state: &AppState) -> bool {
    match state.editor_dialog.as_ref() {
        Some(EditorDialog::Crop(d)) => d.frame.loaded(),
        Some(EditorDialog::Stretch(d)) => d.frame.loaded(),
        None => false,
    }
}

/// The stretch preview's frame rect for a window `screen` (tests).
pub fn stretch_frame_rect(state: &AppState, screen: Rect) -> Option<Rect> {
    let Some(EditorDialog::Stretch(d)) = state.editor_dialog.as_ref() else {
        return None;
    };
    let pos = (screen.center() - STRETCH_SIZE / 2.0)
        .max(screen.min)
        .round();
    let inner = Rect::from_min_size(pos, STRETCH_SIZE).shrink2(Vec2::new(20.0, 18.0));
    let top = inner.min.y + crate::widgets::dialog::HEADER_H + 12.0 + 17.0 + 12.0;
    let area = Rect::from_min_max(
        Pos2::new(inner.min.x, top),
        Pos2::new(inner.max.x, inner.max.y - 36.0 - 12.0),
    );
    Some(stretch_rect(area, d.base, d.ratio))
}

/// Blocks until the dialog's frame decode finished (tests).
pub fn wait_frame(ctx: &egui::Context, state: &mut AppState) {
    for _ in 0..500 {
        let loading = match state.editor_dialog.as_mut() {
            Some(EditorDialog::Crop(d)) => {
                d.frame.poll(ctx);
                matches!(d.frame, FrameState::Loading(_))
            }
            Some(EditorDialog::Stretch(d)) => {
                d.frame.poll(ctx);
                matches!(d.frame, FrameState::Loading(_))
            }
            None => false,
        };
        if !loading {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[allow(clippy::too_many_arguments)]
fn flat_button(
    ui: &mut Ui,
    t: &Tokens,
    r: Rect,
    text: &str,
    name: &str,
    px: f32,
    primary: bool,
    border: Color,
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
    let (bg, edge, fg) = match (primary, hovered) {
        (true, false) => (Some(Color::Accent), Some(Color::Accent), Color::Bg),
        (true, true) => (Some(Color::Text), Some(Color::Text), Color::Bg),
        (false, false) => (None, Some(border), Color::Text),
        (false, true) => (None, Some(Color::Accent), Color::Accent),
    };
    let fg = if enabled { fg } else { Color::TextMuted };
    let radius = CornerRadius::same(if px >= 10.0 { geom::RADIUS as u8 } else { 0 });
    if let Some(bg) = bg {
        ui.painter().rect_filled(r, radius, t.color(bg));
    }
    if let Some(edge) = edge {
        ui.painter().rect_stroke(
            r,
            radius,
            Stroke::new(1.0, t.color(edge)),
            StrokeKind::Inside,
        );
    }
    let g = galley(ui, text, px, Weight::W600, t.color(fg));
    paint_text(ui, g, r, Align::Center, 0.0, t.color(fg));
    let mut m = fidelity::mark(ui, name, r).text(text, px, Weight::W600);
    if let Some(edge) = edge {
        m = m.sample("top", Pos2::new(r.center().x, r.min.y), edge);
    }
    m.done();
    resp
}

#[allow(clippy::too_many_arguments)]
fn label(
    ui: &Ui,
    t: &Tokens,
    r: Rect,
    text: &str,
    px: f32,
    color: Color,
    align: Align,
    name: &str,
) {
    let g = galley(ui, text, px, Weight::W400, t.color(color));
    paint_text(ui, g, r, align, 0.0, t.color(color));
    if !name.is_empty() {
        fidelity::mark(ui, name, r)
            .text(text, px, Weight::W400)
            .done();
    }
}

/// `None`: still open; `Some(None)`: cancelled; `Some(Some(Ok(crop)))`.
fn crop_ui(
    ui: &mut Ui,
    t: &Tokens,
    icons: &mut crate::icons::IconCache,
    rect: Rect,
    d: &mut CropDialog,
) -> Option<Outcome> {
    d.frame.poll(ui.ctx());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, t.color(Color::Bg));
    // Header: 40 px SHELL_BG, 1 px TEXT bottom rule.
    let hdr = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 40.0));
    p.rect_filled(hdr, 0.0, t.color(Color::ShellBg));
    p.hline(
        hdr.x_range(),
        hdr.max.y - 0.5,
        Stroke::new(1.0, t.color(Color::Text)),
    );
    let g = galley(ui, "Crop", 9.0, Weight::W600, t.color(Color::Text));
    let tr = Rect::from_min_size(
        Pos2::new(hdr.min.x + 20.0, hdr.min.y),
        Vec2::new(g.size().x.ceil(), 40.0),
    );
    paint_text(ui, g, tr, Align::Min, 0.0, t.color(Color::Text));
    fidelity::mark(ui, "QLabel[Crop]", tr)
        .text("Crop", 9.0, Weight::W600)
        .done();
    let close_r = Rect::from_min_size(
        Pos2::new(hdr.max.x - 12.0 - 40.0, hdr.min.y),
        Vec2::splat(40.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(close_r));
    let close = icon_button(
        &mut child,
        t,
        icons,
        Icon::Close,
        14.0,
        IconButtonStyle {
            size: Vec2::splat(40.0),
            bg: None,
            hover_bg: None,
            pressed_bg: None,
            border: None,
            hover_border: None,
            radius: 0.0,
            hover_tint: Some(Color::Error),
            enabled: true,
        },
        "cropDialogClose",
    )
    .clicked();
    // Instruction line.
    let instr = Rect::from_min_size(
        Pos2::new(rect.min.x, hdr.max.y),
        Vec2::new(rect.width(), 28.0),
    );
    let text = "Drag handles to resize  ·  drag inside to move  ·  16:9 / 9:16 for reference frame";
    let g = galley(ui, text, 8.0, Weight::W400, t.color(Color::Accent));
    paint_text(ui, g, instr, Align::Center, 0.0, t.color(Color::Accent));
    fidelity::mark(ui, format!("QLabel[{}]", &text[..40]), instr)
        .text(text, 8.0, Weight::W400)
        .done();
    // Frame area.
    let bottom_h = 48.0;
    let area = Rect::from_min_max(
        Pos2::new(rect.min.x, instr.max.y),
        Pos2::new(rect.max.x, rect.max.y - bottom_h),
    );
    let mut bounds = Rect::NOTHING;
    match &d.frame {
        FrameState::Ready(tex, [w, h]) => {
            let (w, h) = (*w as f32, *h as f32);
            let scale = (area.width() / w).min(area.height() / h);
            let size = Vec2::new((w * scale).floor(), (h * scale).floor());
            bounds = Rect::from_min_size(
                Pos2::new(
                    area.min.x + ((area.width() - size.x) / 2.0).floor(),
                    area.min.y + ((area.height() - size.y) / 2.0).floor(),
                ),
                size,
            );
            ui.painter().image(
                tex.id(),
                bounds,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            if d.display.is_none()
                && let Some((x, y, cw, ch)) = d.initial.take()
            {
                let sc = bounds.width() / w;
                d.display = Some(Rect::from_min_size(
                    Pos2::new(bounds.min.x + x as f32 * sc, bounds.min.y + y as f32 * sc),
                    Vec2::new(cw as f32 * sc, ch as f32 * sc),
                ));
            }
        }
        FrameState::Loading(_) => {
            // Unstyled in the reference (host default font): box only.
            label(
                ui,
                t,
                area,
                "Loading preview…",
                15.0,
                Color::Text,
                Align::Center,
                "",
            );
            fidelity::mark(ui, "QLabel[Loading preview…]", area).done();
        }
        FrameState::Failed => {
            label(
                ui,
                t,
                area,
                "Could not load frame",
                15.0,
                Color::Text,
                Align::Center,
                "",
            );
            fidelity::mark(ui, "QLabel[Could not load frame]", area).done();
        }
    }
    if let FrameState::Ready(_, [sw, sh]) = &d.frame {
        crop_overlay(ui, t, area, bounds, d, (*sw as u32, *sh as u32));
    }
    // Bottom bar.
    let bot = Rect::from_min_max(Pos2::new(rect.min.x, area.max.y), rect.max);
    ui.painter().rect_filled(bot, 0.0, t.color(Color::ShellBg));
    ui.painter().hline(
        bot.x_range(),
        bot.min.y + 0.5,
        Stroke::new(1.0, t.color(Color::Text)),
    );
    let info = d
        .crop
        .map(|(x, y, w, h)| format!("{w} × {h}  at  ({x}, {y})"))
        .unwrap_or_default();
    let ir = Rect::from_min_size(
        Pos2::new(bot.min.x + 16.0, bot.min.y),
        Vec2::new(300.0, bottom_h),
    );
    label(ui, t, ir, &info, 9.0, Color::Accent, Align::Min, "");
    let enabled = d.frame.loaded();
    let by = bot.min.y + 9.0;
    let mut x = bot.max.x - 16.0;
    let mut place = |w: f32| {
        x -= w;
        let r = Rect::from_min_size(Pos2::new(x, by), Vec2::new(w, 30.0));
        x -= 10.0;
        r
    };
    let apply_r = place(110.0);
    let clear_r = place(80.0);
    let r916 = place(60.0);
    let r169 = place(60.0);
    let apply = flat_button(
        ui,
        t,
        apply_r,
        "Apply crop",
        "QPushButton[Apply crop]",
        9.0,
        true,
        Color::Text,
        enabled,
    );
    let clear = flat_button(
        ui,
        t,
        clear_r,
        "Clear",
        "QPushButton[Clear]",
        9.0,
        false,
        Color::Text,
        enabled,
    );
    let b916 = flat_button(
        ui,
        t,
        r916,
        "9:16",
        "QPushButton[9:16]",
        9.0,
        false,
        Color::Text,
        enabled,
    )
    .on_hover_text("Set crop to a centered 9:16 reference frame");
    let b169 = flat_button(
        ui,
        t,
        r169,
        "16:9",
        "QPushButton[16:9]",
        9.0,
        false,
        Color::Text,
        enabled,
    )
    .on_hover_text("Set crop to a centered 16:9 reference frame");
    if let FrameState::Ready(_, [sw, sh]) = &d.frame {
        let src = (*sw as u32, *sh as u32);
        for (clicked, aw, ah) in [(b169.clicked(), 16.0, 9.0), (b916.clicked(), 9.0, 16.0)] {
            if clicked && bounds.width() > 4.0 && bounds.height() > 4.0 {
                let target = aw / ah;
                let (cw, ch) = if target > bounds.width() / bounds.height() {
                    (bounds.width(), (bounds.width() / target).round().max(4.0))
                } else {
                    ((bounds.height() * target).round().max(4.0), bounds.height())
                };
                let r = Rect::from_min_size(
                    Pos2::new(
                        bounds.min.x + ((bounds.width() - cw) / 2.0).floor(),
                        bounds.min.y + ((bounds.height() - ch) / 2.0).floor(),
                    ),
                    Vec2::new(cw, ch),
                );
                d.display = Some(r);
                d.crop = to_source(r, bounds, src);
            }
        }
    }
    if clear.clicked() {
        d.display = None;
        d.crop = None;
        d.initial = None;
    }
    if close {
        return Some(None);
    }
    if apply.clicked() && enabled {
        return Some(Some(Ok(d.crop)));
    }
    None
}

/// Display crop → source pixels (`_on_overlay_changed`); `None` unless
/// both sides exceed 4 px.
fn to_source(r: Rect, bounds: Rect, (sw, sh): (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let bw = bounds.width().max(1.0);
    let bh = bounds.height().max(1.0);
    let x1 = ((r.min.x - bounds.min.x) * sw as f32 / bw).max(0.0) as i64;
    let y1 = ((r.min.y - bounds.min.y) * sh as f32 / bh).max(0.0) as i64;
    let x2 = (((r.max.x - bounds.min.x) * sw as f32 / bw) as i64).min(i64::from(sw));
    let y2 = (((r.max.y - bounds.min.y) * sh as f32 / bh) as i64).min(i64::from(sh));
    let (w, h) = (x2 - x1, y2 - y1);
    (w > 4 && h > 4).then_some((x1 as u32, y1 as u32, w as u32, h as u32))
}

fn handle_points(r: Rect) -> [Pos2; 8] {
    let c = r.center();
    [
        r.left_top(),
        Pos2::new(c.x, r.min.y),
        r.right_top(),
        Pos2::new(r.max.x, c.y),
        r.right_bottom(),
        Pos2::new(c.x, r.max.y),
        r.left_bottom(),
        Pos2::new(r.min.x, c.y),
    ]
}

fn crop_overlay(
    ui: &mut Ui,
    t: &Tokens,
    area: Rect,
    bounds: Rect,
    d: &mut CropDialog,
    src: (u32, u32),
) {
    let resp = ui.interact(
        area,
        ui.scope_id().with("cropOverlay"),
        Sense::click_and_drag(),
    );
    let (pressed, down, pos) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.pointer.interact_pos(),
        )
    });
    let clamp = |r: Rect| r.intersect(bounds);
    if pressed
        && resp.hovered()
        && let Some(p) = pos
    {
        let has = d
            .display
            .is_some_and(|r| r.width() > 4.0 && r.height() > 4.0);
        let hit = if has {
            let r = d.display.unwrap_or(Rect::NOTHING);
            handle_points(r)
                .iter()
                .position(|h| (p.x - h.x).abs() <= HANDLE_HIT && (p.y - h.y).abs() <= HANDLE_HIT)
                .map(CropDrag::Handle)
                .or_else(|| r.contains(p).then_some(CropDrag::Move))
        } else {
            None
        };
        if let Some(h) = hit {
            d.drag = Some((h, p, d.display.unwrap_or(Rect::NOTHING)));
        } else if bounds.contains(p) {
            d.drag = Some((CropDrag::New, p, Rect::from_min_size(p, Vec2::ZERO)));
            d.display = Some(Rect::from_min_size(p, Vec2::ZERO));
        }
    } else if down && let (Some((mode, origin, start)), Some(p)) = (d.drag, pos) {
        let delta = p - origin;
        match mode {
            CropDrag::New => {
                d.display = Some(clamp(Rect::from_two_pos(origin, p)));
            }
            CropDrag::Move => {
                let mut m = start.translate(delta);
                let dx = (bounds.min.x - m.min.x).max(0.0) + (bounds.max.x - m.max.x).min(0.0);
                let dy = (bounds.min.y - m.min.y).max(0.0) + (bounds.max.y - m.max.y).min(0.0);
                m = m.translate(Vec2::new(dx, dy));
                d.display = Some(m);
            }
            CropDrag::Handle(i) => {
                let mut r = start;
                if matches!(i, 0 | 6 | 7) {
                    r.min.x += delta.x;
                }
                if matches!(i, 2..=4) {
                    r.max.x += delta.x;
                }
                if matches!(i, 0..=2) {
                    r.min.y += delta.y;
                }
                if matches!(i, 4..=6) {
                    r.max.y += delta.y;
                }
                let r = Rect::from_two_pos(r.min, r.max);
                if r.width() > 4.0 && r.height() > 4.0 {
                    d.display = Some(clamp(r));
                }
            }
        }
        d.crop = d.display.and_then(|r| to_source(r, bounds, src));
    }
    if !down {
        d.drag = None;
    }
    // Paint: dim outside, outline, thirds, handles.
    let Some(r) = d.display.filter(|r| r.width() > 4.0 && r.height() > 4.0) else {
        return;
    };
    let p = ui.painter_at(area);
    let bg = t.color(Color::Bg);
    let dim = Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 148);
    for band in [
        Rect::from_min_max(area.min, Pos2::new(area.max.x, r.min.y)),
        Rect::from_min_max(Pos2::new(area.min.x, r.max.y), area.max),
        Rect::from_min_max(Pos2::new(area.min.x, r.min.y), Pos2::new(r.min.x, r.max.y)),
        Rect::from_min_max(Pos2::new(r.max.x, r.min.y), Pos2::new(area.max.x, r.max.y)),
    ] {
        p.rect_filled(band, 0.0, dim);
    }
    let text = t.color(Color::Text);
    p.rect_stroke(r, 0.0, Stroke::new(1.0, text), StrokeKind::Inside);
    let faint = Color32::from_rgba_unmultiplied(text.r(), text.g(), text.b(), 42);
    for k in 1..3 {
        let x = r.min.x + (r.width() / 3.0).floor() * k as f32;
        let y = r.min.y + (r.height() / 3.0).floor() * k as f32;
        p.vline(x, r.y_range(), Stroke::new(1.0, faint));
        p.hline(r.x_range(), y, Stroke::new(1.0, faint));
    }
    for h in handle_points(r) {
        let hr = Rect::from_center_size(h, Vec2::splat(HANDLE));
        p.rect_filled(hr, 0.0, text);
        p.rect_stroke(hr, 0.0, Stroke::new(1.0, bg), StrokeKind::Inside);
    }
}

/// `StretchOverlay._rect_for_ratio`.
fn stretch_rect(area: Rect, base: (u32, u32), ratio: f64) -> Rect {
    let available_w = (area.width() - 64.0).max(1.0);
    let available_h = (area.height() - 54.0).max(1.0);
    let aspect = base.0 as f32 / base.1 as f32;
    let base_w = ((available_w * 0.62) as i32).min((available_h * aspect) as i32) as f32;
    let mut base_h = available_h.min((base_w / aspect.max(0.001)).floor());
    let mut target_w = ((base_w as f64 * ratio) as f32).max(24.0).floor();
    if target_w > available_w {
        let scale = available_w / target_w;
        target_w = available_w;
        base_h = (base_h * scale).floor().max(24.0);
    }
    Rect::from_min_size(
        Pos2::new(
            area.min.x + ((area.width() - target_w) / 2.0).floor(),
            area.min.y + ((area.height() - base_h) / 2.0).floor(),
        ),
        Vec2::new(target_w, base_h),
    )
}

fn stretch_ui(
    ui: &mut Ui,
    t: &Tokens,
    icons: &mut crate::icons::IconCache,
    rect: Rect,
    d: &mut StretchDialog,
) -> Option<Outcome> {
    d.frame.poll(ui.ctx());
    ui.painter().rect_filled(rect, 0.0, t.color(Color::ShellBg));
    let inner = rect.shrink2(Vec2::new(20.0, 18.0));
    let hdr = Rect::from_min_size(
        inner.min,
        Vec2::new(inner.width(), crate::widgets::dialog::HEADER_H),
    );
    let close = crate::widgets::dialog::header(ui, t, icons, hdr, "Stretch video").clicked();
    let target_w = ((d.base.0 as f64 * d.ratio) as u32).max(2) & !1;
    let target_h = d.base.1.max(2) & !1;
    let dims = format!("{:.2}×  ·  {target_w} × {target_h}", d.ratio);
    let g = galley(ui, &dims, 11.0, Weight::W400, t.color(Color::Accent));
    let dw = g.size().x.ceil();
    let dr = Rect::from_min_size(
        Pos2::new(inner.max.x - dw, hdr.max.y + 12.0),
        Vec2::new(dw, 17.0),
    );
    paint_text(ui, g, dr, Align::Min, 0.0, t.color(Color::Accent));
    fidelity::mark(ui, "stretchDialogDimensions", dr)
        .text(dims.clone(), 11.0, Weight::W400)
        .done();
    let btn_y = inner.max.y - 36.0;
    let area = Rect::from_min_max(
        Pos2::new(inner.min.x, dr.max.y + 12.0),
        Pos2::new(inner.max.x, btn_y - 12.0),
    );
    // Preview.
    let p = ui.painter_at(area);
    p.rect_filled(area, 0.0, t.color(Color::Surface1));
    let r = stretch_rect(area, d.base, d.ratio);
    if let FrameState::Ready(tex, _) = &d.frame {
        p.image(
            tex.id(),
            r,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    let bg = t.color(Color::Bg);
    let dim = Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 74);
    for band in [
        Rect::from_min_max(area.min, Pos2::new(area.max.x, r.min.y)),
        Rect::from_min_max(Pos2::new(area.min.x, r.max.y), area.max),
        Rect::from_min_max(Pos2::new(area.min.x, r.min.y), Pos2::new(r.min.x, r.max.y)),
        Rect::from_min_max(Pos2::new(r.max.x, r.min.y), Pos2::new(area.max.x, r.max.y)),
    ] {
        p.rect_filled(band, 0.0, dim);
    }
    p.rect_stroke(
        r,
        0.0,
        Stroke::new(2.0, t.color(Color::Accent)),
        StrokeKind::Middle,
    );
    for x in [r.min.x, r.max.x] {
        p.rect_filled(
            Rect::from_min_size(
                Pos2::new(x - 5.0, r.center().y - 24.0),
                Vec2::new(10.0, 48.0),
            ),
            0.0,
            t.color(Color::Accent),
        );
        p.rect_filled(
            Rect::from_min_size(
                Pos2::new(x - 1.0, r.center().y - 10.0),
                Vec2::new(2.0, 20.0),
            ),
            0.0,
            t.color(Color::Surface2),
        );
    }
    let text = format!("{:.2}×   {target_w} × {target_h}", d.ratio);
    let g = galley(ui, &text, 12.0, Weight::W600, t.color(Color::Text));
    let lw = g.size().x.ceil() + 20.0;
    let lr = Rect::from_min_size(
        Pos2::new(area.center().x - (lw / 2.0).floor(), r.max.y + 10.0),
        Vec2::new(lw, 24.0),
    );
    let s2 = t.color(Color::Surface2);
    p.rect_filled(
        lr,
        0.0,
        Color32::from_rgba_unmultiplied(s2.r(), s2.g(), s2.b(), 226),
    );
    p.rect_stroke(
        lr,
        0.0,
        Stroke::new(1.0, t.color(Color::Accent)),
        StrokeKind::Inside,
    );
    paint_text(ui, g, lr, Align::Center, 0.0, t.color(Color::Text));
    // Side-handle drag.
    let resp = ui.interact(
        area,
        ui.scope_id().with("stretchOverlay"),
        Sense::click_and_drag(),
    );
    let (pressed, down, pos) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.pointer.interact_pos(),
        )
    });
    if pressed
        && resp.hovered()
        && let Some(p) = pos
    {
        if (p.x - r.min.x).abs() <= STRETCH_HIT {
            d.drag = Some((true, p.x, d.ratio));
        } else if (p.x - r.max.x).abs() <= STRETCH_HIT {
            d.drag = Some((false, p.x, d.ratio));
        }
    } else if down && let (Some((left, origin, initial)), Some(p)) = (d.drag, pos) {
        let dir = if left { -1.0 } else { 1.0 };
        d.ratio = (initial + f64::from((p.x - origin) * dir / STRETCH_PX)).clamp(0.25, 4.0);
    }
    if !down {
        d.drag = None;
    }
    // Buttons.
    let mut x = inner.max.x;
    let mut right = |ui: &mut Ui, text: &str, name: &str, primary: bool| {
        let w = galley(ui, text, 10.0, Weight::W600, Color32::WHITE)
            .size()
            .x
            .ceil()
            + 34.0;
        x -= w;
        let r = Rect::from_min_size(Pos2::new(x, btn_y), Vec2::new(w, 36.0));
        x -= 8.0;
        flat_button(ui, t, r, text, name, 10.0, primary, Color::BorderHi, true)
    };
    let apply = right(ui, "Apply stretch", "stretchDialogApply", true);
    let cancel = right(ui, "Cancel", "stretchDialogSecondary[Cancel]", false);
    let rw = galley(ui, "Reset to source", 10.0, Weight::W600, Color32::WHITE)
        .size()
        .x
        .ceil()
        + 34.0;
    let reset = flat_button(
        ui,
        t,
        Rect::from_min_size(Pos2::new(inner.min.x, btn_y), Vec2::new(rw, 36.0)),
        "Reset to source",
        "stretchDialogSecondary[Reset to source]",
        10.0,
        false,
        Color::BorderHi,
        true,
    );
    if reset.clicked() {
        d.ratio = 1.0;
    }
    if close || cancel.clicked() {
        return Some(None);
    }
    if apply.clicked() {
        return Some(Some(Err(d.ratio)));
    }
    None
}
