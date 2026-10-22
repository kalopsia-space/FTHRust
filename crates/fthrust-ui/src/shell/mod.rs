//! The main window shell: title strip, capture toolbar, navigation rail,
//! page stack and error bar (spec 5.1).

pub mod title_strip;
pub mod window;

use egui::{Pos2, Rect, Ui, Vec2};

use crate::state::AppState;
use crate::tokens::{Color, geom};

/// Draws one frame of the shell into the root `ui` and handles
/// frameless-window input.
pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let ctx = ui.ctx().clone();
    let (maximized, fullscreen) = ctx.input(|i| {
        (
            i.viewport().maximized.unwrap_or(false),
            i.viewport().fullscreen.unwrap_or(false),
        )
    });
    state.window.maximized = maximized;
    state.window.fullscreen = fullscreen;
    let screen = ctx.content_rect();
    if !maximized && !fullscreen {
        state.window.last_inner = fthrust_core::window_geometry::Rect {
            x: 0,
            y: 0,
            width: screen.width().round() as u32,
            height: screen.height().round() as u32,
        };
    }
    ui.painter()
        .rect_filled(screen, 0.0, state.tokens.color(Color::Bg));
    let strip = Rect::from_min_size(screen.min, Vec2::new(screen.width(), geom::TITLE_BAR_H));
    let resp = ui
        .scope_builder(egui::UiBuilder::new().max_rect(strip), |ui| {
            title_strip::ui(ui, state)
        })
        .inner;
    if resp.close {
        state.window.close_requested = true;
    }
    if resp.minimise {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
    if resp.maximise {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!state.window.maximized));
    }
    if resp.kofi {
        open_url("https://ko-fi.com/fthrclips");
    }
    let below = Rect::from_min_max(Pos2::new(screen.min.x, strip.max.y), screen.max);
    ui.scope_builder(egui::UiBuilder::new().max_rect(below), |ui| {
        ui.painter()
            .rect_filled(below, 0.0, state.tokens.color(Color::Bg));
    });
    window::handle_frame_input(&ctx, strip, resp.over_button, &state.window);
    if state.window.close_requested {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// Opens a URL with the desktop's handler.
pub fn open_url(url: &str) {
    if let Err(e) = std::process::Command::new("xdg-open").arg(url).spawn() {
        tracing::warn!("xdg-open {url}: {e}");
    }
}
