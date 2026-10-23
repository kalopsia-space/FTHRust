//! `ExportJobsDialog` ("Exports", `ui/export_jobs.py`): a non-modal
//! 720×400 panel listing queued exports with a progress bar each, "Cancel
//! selected" and "Open output folder". Closing it keeps exports running.

use egui::{Align, Area, Id, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use fthrust_core::export::quality_label;

use crate::fidelity;
use crate::fonts::Weight;
use crate::services::export::TaskView;
use crate::state::AppState;
use crate::tokens::{Color, Tokens, size};
use crate::widgets::buttons::{self, Kind, Opts};
use crate::widgets::dialog::{HEADER_H, header};
use crate::widgets::{galley, paint_text};

/// `resize(720, 400)`.
pub const SIZE: Vec2 = Vec2::new(720.0, 400.0);

const HINT: &str =
    "Keep browsing while your clips export. Closing this window keeps exports running.";
const EMPTY: &str = "No export jobs yet. Start an export from a clip.";
/// List frame, relative to the window.
const LIST: Rect = Rect {
    min: Pos2::new(16.0, 82.0),
    max: Pos2::new(704.0, 329.0),
};
/// `QSize(0, 68)` rows.
const ROW_H: f32 = 68.0;

fn quality_text(q: fthrust_media::export::Quality) -> &'static str {
    use fthrust_media::export::Quality;
    quality_label(match q {
        Quality::Fast => "fast",
        Quality::Quality => "quality",
        Quality::Lossless => "lossless",
    })
}

/// Shows the window while `state.export_jobs_open`, centred over the
/// window, above the page but below modal dialogs.
pub fn ui(ctx: &egui::Context, state: &mut AppState) {
    if std::mem::take(&mut state.export_jobs_requested) {
        state.export_jobs_open = true;
    }
    if !state.export_jobs_open {
        return;
    }
    let screen = ctx.content_rect();
    let pos = (screen.center() - SIZE / 2.0).max(screen.min).round();
    Area::new(Id::unique("export-jobs"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .constrain(false)
        .interactable(true)
        .show(ctx, |ui| {
            draw(ui, state, Rect::from_min_size(pos, SIZE));
        });
    // The 500 ms refresh timer.
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}

/// Draws the window into `rect` (tests render it standalone).
pub fn draw(ui: &mut Ui, state: &mut AppState, rect: Rect) {
    let t = state.tokens.clone();
    let tasks = state.export_queue.tasks();
    // Selection by task identity, default row 0.
    if !tasks
        .iter()
        .any(|task| Some(task.id) == state.export_jobs_selected)
    {
        state.export_jobs_selected = tasks.first().map(|task| task.id);
    }
    let at = |r: Rect| r.translate(rect.min.to_vec2());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, t.color(Color::Bg));
    p.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, t.color(Color::BorderHi)),
        StrokeKind::Inside,
    );
    ui.allocate_rect(rect, Sense::click());
    let close = header(
        ui,
        &t,
        &mut state.icons,
        Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER_H)),
        "Exports",
    );
    if close.clicked() {
        state.export_jobs_open = false;
    }
    let hint_name = format!("QLabel[{}]", fidelity::marker_label(HINT));
    if tasks.is_empty() {
        // The list is hidden: the hint and the empty label share the space.
        label(
            ui,
            &t,
            at(rect_xywh(16.0, 54.0, 688.0, 133.0)),
            HINT,
            size::MICRO,
            Weight::W400,
            Color::TextDim,
            Align::Min,
            &hint_name,
        );
        label(
            ui,
            &t,
            at(rect_xywh(16.0, 197.0, 688.0, 132.0)),
            EMPTY,
            15.0,
            Weight::W400,
            Color::TextDim,
            Align::Center,
            &format!("QLabel[{}]", fidelity::marker_label(EMPTY)),
        );
    } else {
        label(
            ui,
            &t,
            at(rect_xywh(16.0, 54.0, 688.0, 18.0)),
            HINT,
            size::MICRO,
            Weight::W400,
            Color::TextDim,
            Align::Min,
            &hint_name,
        );
        list(ui, &t, state, &tasks, at(LIST));
    }
    let selected = tasks
        .iter()
        .find(|task| Some(task.id) == state.export_jobs_selected)
        .cloned();
    let cancel = action(
        ui,
        &t,
        at(rect_xywh(16.0, 345.0, 340.0, 41.0)),
        "Cancel selected",
        // Qt only updates the buttons on a row change, so with no rows they
        // keep their enabled look (and do nothing).
        tasks.is_empty() || selected.as_ref().is_some_and(|task| !task.done),
    );
    if cancel && let Some(task) = &selected {
        state.export_queue.cancel(task.id);
    }
    let open = action(
        ui,
        &t,
        at(rect_xywh(364.0, 345.0, 340.0, 41.0)),
        "Open output folder",
        tasks.is_empty() || selected.is_some(),
    );
    if open
        && let Some(task) = &selected
        && let Some(dir) = task.output.parent()
    {
        crate::shell::open_url(&dir.to_string_lossy());
    }
}

fn rect_xywh(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
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
    fidelity::mark(ui, name, r).text(text, px, weight).done();
}

fn action(ui: &mut Ui, t: &Tokens, r: Rect, text: &str, enabled: bool) -> bool {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    let opts = Opts {
        enabled,
        expand: true,
        fixed_height: Some(r.height()),
        ..Opts::default()
    };
    let name = format!("QPushButton[{text}]");
    buttons::button_with(&mut child, t, None, Kind::Primary, text, &name, &opts).clicked()
        && enabled
}

fn list(ui: &mut Ui, t: &Tokens, state: &mut AppState, tasks: &[TaskView], frame: Rect) {
    let p = ui.painter();
    p.rect_filled(frame, 0.0, t.color(Color::Surface1));
    p.rect_stroke(
        frame,
        0.0,
        Stroke::new(1.0, t.color(Color::Border)),
        StrokeKind::Inside,
    );
    let viewport = frame.shrink(1.0);
    fidelity::mark(ui, "qt_scrollarea_viewport", viewport).done();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(viewport));
    child.set_clip_rect(viewport);
    egui::ScrollArea::vertical()
        .id_salt("export-jobs-list")
        .max_height(viewport.height())
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            let top = ui.cursor().min;
            for (i, task) in tasks.iter().enumerate() {
                let row = Rect::from_min_size(
                    Pos2::new(viewport.min.x, top.y + i as f32 * ROW_H),
                    Vec2::new(viewport.width(), ROW_H),
                );
                let resp = ui.allocate_rect(row, Sense::click());
                if resp.clicked() {
                    state.export_jobs_selected = Some(task.id);
                }
                draw_row(
                    ui,
                    t,
                    task,
                    row,
                    state.export_jobs_selected == Some(task.id),
                );
            }
        });
}

fn draw_row(ui: &Ui, t: &Tokens, task: &TaskView, row: Rect, selected: bool) {
    if selected {
        ui.painter().rect_filled(row, 0.0, t.color(Color::Surface3));
    }
    // Margins 12,10,12,10; spacing 16; percent 42 wide; bar 110×8.
    let inner = Rect::from_min_max(
        Pos2::new(row.min.x + 12.0, row.min.y + 10.0),
        Pos2::new(row.max.x - 12.0, row.max.y - 11.0),
    );
    let bar_w = 110.0;
    let pct_w = 42.0;
    let text_w = inner.width() - bar_w - pct_w - 2.0 * 16.0;
    let name = task
        .source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name_r = Rect::from_min_size(inner.min, Vec2::new(text_w, 22.0));
    label(
        ui,
        t,
        name_r,
        &name,
        size::LABEL,
        Weight::W400,
        Color::Text,
        Align::Min,
        &format!("QLabel[{}]", fidelity::marker_label(&name)),
    );
    ui.interact(name_r, ui.scope_id().with(("name", task.id)), Sense::hover())
        .on_hover_text(&name);
    let status = format!("{} · {}", quality_text(task.quality), task.status);
    let status_r = Rect::from_min_size(
        Pos2::new(inner.min.x, inner.min.y + 25.0),
        Vec2::new(text_w, 22.0),
    );
    label(
        ui,
        t,
        status_r,
        &status,
        11.0,
        Weight::W400,
        Color::TextDim,
        Align::Min,
        &format!("QLabel[{}]", fidelity::marker_label(&status)),
    );
    let tip = if task.detail.is_empty() {
        task.output.to_string_lossy().into_owned()
    } else {
        task.detail.clone()
    };
    ui.interact(status_r, ui.scope_id().with(("status", task.id)), Sense::hover())
        .on_hover_text(tip);
    let percent = task.progress.clamp(0.0, 100.0) as i64;
    let pct_text = format!("{percent}%");
    let pct_r = Rect::from_min_size(
        Pos2::new(inner.min.x + text_w + 16.0, inner.min.y),
        Vec2::new(pct_w, inner.height()),
    );
    label(
        ui,
        t,
        pct_r,
        &pct_text,
        13.0,
        Weight::W400,
        Color::Text,
        Align::Max,
        &format!("QLabel[{pct_text}]"),
    );
    let bar = Rect::from_center_size(
        Pos2::new(pct_r.max.x + 16.0 + bar_w / 2.0, pct_r.center().y),
        Vec2::new(bar_w, 8.0),
    );
    ui.painter().rect_filled(bar, 4.0, t.color(Color::Surface3));
    let fill = Rect::from_min_size(bar.min, Vec2::new(bar_w * percent as f32 / 100.0, 8.0));
    if fill.width() > 0.0 {
        ui.painter().rect_filled(fill, 4.0, t.color(Color::Accent));
    }
}
