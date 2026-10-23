//! Editor keyboard shortcuts (`core/navigation_bindings.py`, category
//! "Clip editor", plus the Application sidebar steps and the fixed F11).
//! Nothing fires while a text field has focus or a modal dialog is open.

use super::EditorRequest;
use crate::library::page::triggered_actions;
use crate::state::AppState;

/// Runs once per frame before the sessions service.
pub fn handle(ctx: &egui::Context, state: &mut AppState) {
    if !state.dialogs.modal.is_empty() {
        return;
    }
    for action in triggered_actions(ctx, state, "Application") {
        match action {
            "sidebar_next" => super::sessions::sidebar_step(state, 1),
            "sidebar_previous" => super::sessions::sidebar_step(state, -1),
            _ => {}
        }
    }
    if state.editor.is_none() {
        return;
    }
    let now = state.editor_now();
    let f11 = state.nav.is_enabled()
        && state.nav.is_category_enabled("Clip editor")
        && !ctx.egui_wants_keyboard_input()
        && ctx.input(|i| i.key_pressed(egui::Key::F11));
    if f11 {
        set_fullscreen(ctx, state, !state.window.fullscreen);
    }
    for action in triggered_actions(ctx, state, "Clip editor") {
        if action == "editor_close" && state.window.fullscreen {
            set_fullscreen(ctx, state, false);
            continue;
        }
        let Some(e) = state.editor.as_mut() else {
            return;
        };
        match action {
            "editor_split" => {
                e.split(now);
            }
            "editor_delete" => {
                e.set_selected_deleted(true, now);
            }
            "editor_restore" => {
                e.set_selected_deleted(false, now);
            }
            "editor_crop" => {
                e.apply_quick_crop(&state.settings, now);
            }
            "editor_export" => e.requests.push(EditorRequest::Export),
            "editor_minimize" => e.requests.push(EditorRequest::Park),
            "editor_close" => e.requests.push(EditorRequest::Close),
            "editor_next" => e.requests.push(EditorRequest::Navigate(1)),
            "editor_previous" => e.requests.push(EditorRequest::Navigate(-1)),
            "editor_play" => e.toggle_play(now),
            "editor_undo" => e.undo(now),
            "editor_redo" | "editor_redo_alt" => e.redo(now),
            _ => {}
        }
    }
}

fn set_fullscreen(ctx: &egui::Context, state: &mut AppState, on: bool) {
    state.window.fullscreen = on;
    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
}
