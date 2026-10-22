//! Frameless window behaviour on Linux (`ui/frameless_window_linux.py`,
//! `main.py` `_bar_mouse_*`): an 8 px invisible resize border handled by the
//! compositor (`xdg_toplevel` resize), compositor-driven move from the title
//! strip, double-click to maximise, and geometry persistence through
//! `fthrust_core::window_geometry`.

use egui::{Context, CursorIcon, Pos2, Rect, ResizeDirection, Vec2, ViewportCommand};
use fthrust_core::settings::SettingsStore;
use fthrust_core::window_geometry::{self, Placement, Rect as GRect, SavedGeometry, Screen};

use crate::state::WindowState;
use crate::tokens::geom;

/// Default window size (`setMinimumSize(1100, 720)`, `default_size`).
pub const DEFAULT_SIZE: (u32, u32) = (1100, 720);

/// The resize edge under `pos` for a window of `size`, within `margin`.
pub fn resize_edge_at(pos: Pos2, size: Vec2, margin: f32) -> Option<ResizeDirection> {
    if size.x <= 0.0 || size.y <= 0.0 {
        return None;
    }
    let left = pos.x < margin;
    let right = pos.x >= size.x - margin;
    let top = pos.y < margin;
    let bottom = pos.y >= size.y - margin;
    Some(match (left, right, top, bottom) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (_, true, true, _) => ResizeDirection::NorthEast,
        (true, _, _, true) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, _, _, _) => ResizeDirection::West,
        (_, true, _, _) => ResizeDirection::East,
        (_, _, true, _) => ResizeDirection::North,
        (_, _, _, true) => ResizeDirection::South,
        _ => return None,
    })
}

fn cursor_for(dir: ResizeDirection) -> CursorIcon {
    match dir {
        ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
        ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
    }
}

/// The viewport command for a primary press at `pos`: a resize on the
/// border (never while maximised or fullscreen), a maximise toggle on a
/// title-strip double-click, else a move from the strip.
pub fn frame_action(
    pos: Pos2,
    size: Vec2,
    strip: Rect,
    double_click: bool,
    maximized: bool,
) -> Option<ViewportCommand> {
    if !maximized && let Some(dir) = resize_edge_at(pos, size, geom::RESIZE_MARGIN) {
        return Some(ViewportCommand::BeginResize(dir));
    }
    if strip.contains(pos) {
        return Some(if double_click {
            ViewportCommand::Maximized(!maximized)
        } else {
            ViewportCommand::StartDrag
        });
    }
    None
}

/// Reads this frame's pointer input and sends the matching command.
/// `over_button` is true while the pointer is on a strip button (those
/// presses belong to the button, not to the window).
pub fn handle_frame_input(ctx: &Context, strip: Rect, over_button: bool, window: &WindowState) {
    let size = ctx.content_rect().size();
    if window.fullscreen {
        return;
    }
    let (hover, pressed, double) = ctx.input(|i| {
        (
            i.pointer.hover_pos(),
            i.pointer
                .primary_pressed()
                .then(|| i.pointer.interact_pos())
                .flatten(),
            i.pointer
                .button_double_clicked(egui::PointerButton::Primary),
        )
    });
    if let Some(p) = hover
        && !window.maximized
        && let Some(dir) = resize_edge_at(p, size, geom::RESIZE_MARGIN)
    {
        ctx.set_cursor_icon(cursor_for(dir));
    }
    let Some(p) = pressed else { return };
    if over_button || ctx.egui_is_using_pointer() {
        return;
    }
    if let Some(cmd) = frame_action(p, size, strip, double, window.maximized) {
        ctx.send_viewport_cmd(cmd);
    }
}

/// The saved geometry from settings (`main_window_geometry`).
pub fn saved_geometry(settings: &SettingsStore) -> SavedGeometry {
    settings
        .get("main_window_geometry")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Where to place the window: the saved geometry clamped onto a visible
/// screen, else 1100×720 centred and maximised (the reference defaults).
pub fn restore_geometry(settings: &SettingsStore, screens: &[Screen]) -> Placement {
    let saved = saved_geometry(settings);
    let fallback = || {
        let screen = screens.iter().find(|s| s.primary).or(screens.first());
        let (w, h) = DEFAULT_SIZE;
        let (x, y) = screen
            .map(|s| {
                (
                    s.available.x + (s.available.width as i32 - w as i32) / 2,
                    s.available.y + (s.available.height as i32 - h as i32) / 2,
                )
            })
            .unwrap_or((0, 0));
        Placement {
            rect: GRect {
                x,
                y,
                width: w,
                height: h,
            },
            screen_name: screen.map(|s| s.name.clone()).unwrap_or_default(),
            maximized: saved.maximized.flatten().unwrap_or(true),
        }
    };
    if saved.normal_geometry.is_none() {
        return fallback();
    }
    window_geometry::restore(&saved, screens).unwrap_or_else(fallback)
}

/// Writes the window's normal geometry (even while maximised) to settings.
pub fn persist_geometry(
    settings: &mut SettingsStore,
    normal: GRect,
    maximized: bool,
    screen_name: &str,
) {
    let saved = SavedGeometry {
        normal_geometry: Some(normal),
        screen_name: Some(screen_name.to_owned()),
        maximized: Some(Some(maximized)),
        saved_dpr: None,
        saved_logical_dpi: None,
    };
    if let Ok(v) = serde_json::to_value(&saved) {
        settings.set("main_window_geometry", v);
    }
    if let Err(e) = settings.save() {
        tracing::warn!("window geometry not saved: {e}");
    }
}
