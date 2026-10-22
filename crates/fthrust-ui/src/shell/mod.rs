//! The main window shell: title strip, capture toolbar, navigation rail,
//! page stack and error bar (spec 5.1).

pub mod error_bar;
pub mod nav_rail;
pub mod page_stack;
pub mod title_strip;
pub mod toolbar;
pub mod window;

use egui::{Pos2, Rect, Ui, Vec2};

use crate::state::{AppState, Page};
use crate::tokens::{Color, geom};
use toolbar::PopupKind;

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
    let resp = frame(ui, state);
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
    window::handle_frame_input(&ctx, resp.strip, resp.over_button, &state.window);
    if state.window.close_requested {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// What the frame reported this pass.
#[derive(Clone, Debug)]
pub struct FrameResponse {
    /// Title strip rect.
    pub strip: Rect,
    /// Pointer over a strip button.
    pub over_button: bool,
    /// Close / minimise / maximise / Ko-fi presses.
    pub close: bool,
    /// Minimise.
    pub minimise: bool,
    /// Maximise.
    pub maximise: bool,
    /// Ko-fi.
    pub kofi: bool,
    /// Error-bar action fired.
    pub action: Option<String>,
}

impl Default for FrameResponse {
    fn default() -> Self {
        FrameResponse {
            strip: Rect::NOTHING,
            over_button: false,
            close: false,
            minimise: false,
            maximise: false,
            kofi: false,
            action: None,
        }
    }
}

/// Lays out the whole window content (no viewport commands): strip,
/// toolbar, rail + page stack, error bar.
pub fn frame(ui: &mut Ui, state: &mut AppState) -> FrameResponse {
    let ctx = ui.ctx().clone();
    // The whole viewport, whatever margin the root ui carries.
    let screen = ctx.content_rect();
    ui.painter()
        .rect_filled(screen, 0.0, state.tokens.color(Color::Bg));
    let mut out = FrameResponse::default();
    // Title strip.
    let strip = Rect::from_min_size(screen.min, Vec2::new(screen.width(), geom::TITLE_BAR_H));
    let ts = ui
        .scope_builder(egui::UiBuilder::new().max_rect(strip), |ui| {
            title_strip::ui(ui, state)
        })
        .inner;
    out.strip = strip;
    out.over_button = ts.over_button;
    out.close = ts.close;
    out.minimise = ts.minimise;
    out.maximise = ts.maximise;
    out.kofi = ts.kofi;
    // Error bar at the bottom when visible.
    let bar_h = if state.error_bar.visible() {
        geom::HW_BAR_H
    } else {
        0.0
    };
    // Toolbar.
    let toolbar_rect = Rect::from_min_size(
        Pos2::new(screen.min.x, strip.max.y),
        Vec2::new(screen.width(), geom::CAPTURE_BAR_H),
    );
    let tb = ui
        .scope_builder(egui::UiBuilder::new().max_rect(toolbar_rect), |ui| {
            toolbar::ui(ui, state)
        })
        .inner;
    state.toolbar_anchors = tb.anchors;
    if let Some(kind) = tb.toggled {
        let was_open = match kind {
            PopupKind::Capture => state.popups.capture.open,
            PopupKind::Source => state.popups.source.open,
            PopupKind::Game => state.popups.game.open,
            PopupKind::Hotkeys => state.popups.hotkeys.open,
        };
        close_popups(state);
        if !was_open {
            match kind {
                PopupKind::Capture => state.popups.capture.show(),
                PopupKind::Source => state.popups.source.show(),
                PopupKind::Game => state.popups.game.show(),
                PopupKind::Hotkeys => state.popups.hotkeys.show(),
            }
        }
    }
    if tb.record {
        state.record_requested = true;
    }
    // Rail + page stack.
    let row = Rect::from_min_max(
        Pos2::new(screen.min.x, toolbar_rect.max.y),
        Pos2::new(screen.max.x, screen.max.y - bar_h),
    );
    let rail_rect = Rect::from_min_size(row.min, Vec2::new(nav_rail::WIDTH, row.height()));
    let rail = nav_rail::ui(ui, state, rail_rect);
    if let Some(page) = rail.page
        && page != state.page
    {
        close_popups(state);
        let now = ctx.input(|i| i.time);
        let reduce = state.tokens.reduce_motion;
        state.page_slide.begin(state.page, page, now, reduce);
        state.page = page;
    }
    if let Some(n) = rail.editor {
        state.editor_requested = Some(n);
    }
    if let Some(n) = rail.close_editor {
        state.editor_close_requested = Some(n);
    }
    let stack_rect = Rect::from_min_max(Pos2::new(rail_rect.max.x, row.min.y), row.max);
    page_stack::ui(ui, state, stack_rect, |ui, state, page| match page {
        Page::Clips => crate::pages::clips_placeholder(ui, state),
        Page::Settings => crate::pages::settings_page(ui, state),
    });
    crate::popups::draw_all(ui, state);
    crate::dialogs::service(state);
    crate::dialogs::draw_all(ui, state);
    crate::dialogs::dispatch_results(state);
    if bar_h > 0.0 {
        let bar_rect = Rect::from_min_size(
            Pos2::new(screen.min.x, screen.max.y - bar_h),
            Vec2::new(screen.width(), bar_h),
        );
        out.action = ui
            .scope_builder(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
                error_bar::ui(ui, state)
            })
            .inner;
    }
    out
}

/// Hides every top-bar popup (page switches, Escape).
pub fn close_popups(state: &mut AppState) {
    state.popups.capture.close();
    state.popups.source.close();
    state.popups.game.close();
    state.popups.hotkeys.close();
}

/// Opens a URL with the desktop's handler.
pub fn open_url(url: &str) {
    if let Err(e) = std::process::Command::new("xdg-open").arg(url).spawn() {
        tracing::warn!("xdg-open {url}: {e}");
    }
}
