//! The editor layer: it covers the whole window while a clip is open
//! (the reference's maximized `ClipEditorWindow`): header, video surface,
//! control bar, the trim bar and the sidebar (`clip_viewer.py:4855`).

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2};

use super::{EditorRequest, Lifecycle};
use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::Icon;
use crate::shell::FrameResponse;
use crate::state::AppState;
use crate::tokens::{Color, geom, size};
use crate::widgets::buttons::{IconButtonStyle, icon_button};
use crate::widgets::{galley, paint_text};

/// Header height.
pub const HEADER_H: f32 = 40.0;
/// Control bar height.
pub const CTRL_H: f32 = 46.0;
/// Trim bar height (the 116 px timeline plus its header rows).
pub const TRIM_H: f32 = 198.0;
/// Play button size.
const PLAY_SIZE: Vec2 = Vec2::new(96.0, 33.0);

/// The rects the layer is split into.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// Header strip.
    pub header: Rect,
    /// Video surface.
    pub video: Rect,
    /// Control bar.
    pub ctrl: Rect,
    /// Trim bar.
    pub trim: Rect,
    /// Sidebar.
    pub sidebar: Rect,
}

/// Splits `screen` like the reference layout.
pub fn layout(screen: Rect) -> Layout {
    let header = Rect::from_min_size(screen.min, Vec2::new(screen.width(), HEADER_H));
    let sidebar = Rect::from_min_max(
        Pos2::new(screen.max.x - geom::EDITOR_SIDEBAR_W, header.max.y),
        screen.max,
    );
    let trim = Rect::from_min_max(
        Pos2::new(screen.min.x, screen.max.y - TRIM_H),
        Pos2::new(sidebar.min.x, screen.max.y),
    );
    let ctrl = Rect::from_min_max(
        Pos2::new(screen.min.x, trim.min.y - CTRL_H),
        Pos2::new(sidebar.min.x, trim.min.y),
    );
    let video = Rect::from_min_max(
        Pos2::new(screen.min.x, header.max.y),
        Pos2::new(sidebar.min.x, ctrl.min.y),
    );
    Layout {
        header,
        video,
        ctrl,
        trim,
        sidebar,
    }
}

/// Draws the editor over the whole window. The response carries the
/// header as the drag strip and the maximize press.
pub fn ui(ui: &mut Ui, state: &mut AppState) -> FrameResponse {
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    let now = state.editor_now();
    let t = state.tokens.clone();
    let l = layout(screen);
    let mut out = FrameResponse {
        strip: l.header,
        ..FrameResponse::default()
    };
    ui.painter().rect_filled(screen, 0.0, t.color(Color::Bg));
    header(ui, state, l.header, &mut out);
    video_surface(ui, state, l.video);
    control_bar(ui, state, l.ctrl, now);
    // Trim bar frame (the timeline fills it).
    ui.painter()
        .rect_filled(l.trim, 0.0, t.color(Color::Surface1));
    ui.painter().hline(
        l.trim.x_range(),
        l.trim.min.y + 0.5,
        Stroke::new(1.0, t.color(Color::ShellDivider)),
    );
    fidelity::mark(ui, "trimBar", l.trim)
        .sample(
            "top",
            Pos2::new(l.trim.center().x, l.trim.min.y),
            Color::ShellDivider,
        )
        .done();
    super::timeline_ui::ui(ui, l.trim, state);
    // Sidebar frame (the panels fill it).
    ui.painter()
        .rect_filled(l.sidebar, 0.0, t.color(Color::ShellBg));
    fidelity::mark(ui, "editorSidebar", l.sidebar)
        .sample("fill", l.sidebar.center(), Color::ShellBg)
        .done();
    if state
        .editor
        .as_ref()
        .is_some_and(|e| e.lifecycle == Lifecycle::Playing)
    {
        ctx.request_repaint();
    }
    out
}

fn header(ui: &mut Ui, state: &mut AppState, rect: Rect, out: &mut FrameResponse) {
    let t = state.tokens.clone();
    ui.painter().rect_filled(rect, 0.0, t.color(Color::ShellBg));
    ui.painter().hline(
        rect.x_range(),
        rect.max.y - 0.5,
        Stroke::new(1.0, t.color(Color::ShellDivider)),
    );
    fidelity::mark(ui, "editorHeader", rect)
        .sample(
            "fill",
            Pos2::new(rect.center().x, rect.min.y + 10.0),
            Color::ShellBg,
        )
        .done();
    let stem: String = state
        .editor
        .as_ref()
        .and_then(|e| e.path.file_stem())
        .map(|s| s.to_string_lossy().chars().take(50).collect())
        .unwrap_or_default();
    let g = galley(ui, &stem, size::LABEL, Weight::W600, t.color(Color::Text));
    let title = Rect::from_min_size(
        Pos2::new(rect.min.x + 20.0, rect.min.y),
        Vec2::new(g.size().x.ceil(), HEADER_H),
    );
    paint_text(ui, g, title, Align::Min, 0.0, t.color(Color::Text));
    fidelity::mark(ui, "editorTitle", title)
        .text(stem.clone(), size::LABEL, Weight::W600)
        .done();
    let buttons = [
        (
            Icon::Minimize,
            Color::Surface3,
            "editorMin",
            "Pause editor in sidebar",
        ),
        (
            Icon::Maximize,
            Color::Surface3,
            "editorMax",
            "Maximize or restore",
        ),
        (Icon::Close, Color::Error, "editorClose", "Close"),
    ];
    let mut x = rect.max.x - 40.0 * buttons.len() as f32;
    for (icon, hover, name, tip) in buttons {
        let r = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::splat(40.0));
        x += 40.0;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        let resp = icon_button(
            &mut child,
            &t,
            &mut state.icons,
            icon,
            14.0,
            IconButtonStyle {
                size: Vec2::splat(40.0),
                bg: None,
                hover_bg: Some(hover),
                pressed_bg: Some(hover),
                border: None,
                hover_border: None,
                radius: 0.0,
                hover_tint: Some(Color::Text),
                enabled: true,
            },
            name,
        )
        .on_hover_text(tip);
        out.over_button |= resp.contains_pointer();
        if resp.clicked() {
            match name {
                "editorMin" => push(state, EditorRequest::Park),
                "editorMax" => out.maximise = true,
                _ => push(state, EditorRequest::Close),
            }
        }
    }
}

fn push(state: &mut AppState, r: EditorRequest) {
    if let Some(e) = state.editor.as_mut() {
        e.requests.push(r);
    }
}

fn video_surface(ui: &mut Ui, state: &mut AppState, rect: Rect) {
    let t = &state.tokens;
    ui.painter().rect_filled(rect, 0.0, t.color(Color::Bg));
    let texture = state
        .editor_video
        .as_ref()
        .and_then(|v| Some((v.id()?, v.size()?)))
        .map(|(id, (w, h))| (id, Vec2::new(w as f32, h as f32)))
        .or_else(|| {
            let path = state.editor.as_ref()?.path.clone();
            state.thumb_textures.get(&path).map(|e| {
                let [w, h] = e.texture.size();
                (e.texture.id(), Vec2::new(w as f32, h as f32))
            })
        });
    if let Some((id, size)) = texture
        && size.x > 0.0
        && size.y > 0.0
    {
        let scale = (rect.width() / size.x).min(rect.height() / size.y);
        let r = Rect::from_center_size(rect.center(), size * scale).round();
        ui.painter().image(
            id,
            r,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

fn control_bar(ui: &mut Ui, state: &mut AppState, rect: Rect, now: f64) {
    let t = state.tokens.clone();
    ui.painter()
        .rect_filled(rect, 0.0, t.color(Color::Surface1));
    ui.painter().hline(
        rect.x_range(),
        rect.min.y + 0.5,
        Stroke::new(1.0, t.color(Color::ShellDivider)),
    );
    fidelity::mark(ui, "ctrlBar", rect)
        .sample(
            "fill",
            Pos2::new(rect.center().x, rect.center().y),
            Color::Surface1,
        )
        .sample(
            "top",
            Pos2::new(rect.center().x, rect.min.y),
            Color::ShellDivider,
        )
        .done();
    let Some(e) = state.editor.as_ref() else {
        return;
    };
    let (label, lifecycle) = (e.play_label(), e.lifecycle);
    let time = format!(
        "{} / {}",
        fthrust_core::editor::format::clock(e.position_ms()),
        fthrust_core::editor::format::clock(e.duration_ms())
    );
    let status = e.status_text().to_owned();
    // Play button: checked (accent) while playing or queued.
    let play = Rect::from_min_size(Pos2::new(rect.min.x + 16.0, rect.min.y + 7.0), PLAY_SIZE);
    let resp = ui.interact(play, ui.scope_id().with("playBtn"), egui::Sense::click());
    let checked = matches!(lifecycle, Lifecycle::Playing | Lifecycle::PlayQueued);
    let enabled = !matches!(lifecycle, Lifecycle::Failed | Lifecycle::Closed);
    let hovered = resp.hovered() && enabled;
    let (bg, border, fg) = if checked {
        (Color::Accent, Color::Accent, Color::Bg)
    } else if hovered {
        (Color::Surface2, Color::Accent, Color::Accent)
    } else {
        (Color::Surface2, Color::Border, Color::Text)
    };
    let radius = CornerRadius::same(geom::RADIUS as u8);
    ui.painter().rect_filled(play, radius, t.color(bg));
    ui.painter().rect_stroke(
        play,
        radius,
        Stroke::new(1.0, t.color(border)),
        StrokeKind::Inside,
    );
    let mut marker = fidelity::mark(ui, "playBtn", play).sample(
        "fill",
        Pos2::new(play.min.x + 4.0, play.center().y),
        bg,
    );
    if lifecycle == Lifecycle::PlayQueued {
        let g = galley(ui, label, size::LABEL, Weight::W600, t.color(fg));
        paint_text(ui, g, play, Align::Center, 0.0, t.color(fg));
        marker = marker.text(label, size::LABEL, Weight::W600);
    } else {
        let icon = if lifecycle == Lifecycle::Playing {
            Icon::Pause
        } else {
            Icon::Play
        };
        let mut tinted = t.clone();
        tinted
            .icon_tints
            .insert(icon.logical_name().to_owned(), t.color(fg));
        let tex = state.icons.texture(ui.ctx(), icon, 16.0, &tinted);
        ui.painter().image(
            tex.id,
            Rect::from_center_size(play.center().round(), Vec2::splat(16.0)),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        marker = marker.icon(icon.asset(), 16.0);
    }
    marker.done();
    if resp.clicked()
        && enabled
        && let Some(e) = state.editor.as_mut()
    {
        e.toggle_play(now);
    }
    // Time and status labels.
    let mut x = play.max.x + 12.0;
    for (text, color) in [(time, Color::TextDim), (status, Color::TextDim)] {
        if text.is_empty() {
            continue;
        }
        let g = galley(ui, &text, size::BODY, Weight::W400, t.color(color));
        let r = Rect::from_min_size(
            Pos2::new(x, rect.min.y + 1.0),
            Vec2::new(g.size().x.ceil(), rect.height() - 1.0),
        );
        paint_text(ui, g, r, Align::Min, 0.0, t.color(color));
        fidelity::mark(ui, format!("timeLabel[{text}]"), r)
            .text(text.clone(), size::BODY, Weight::W400)
            .done();
        x = r.max.x + 12.0;
    }
}
