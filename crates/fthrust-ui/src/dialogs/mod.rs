//! The dialogs the shell shows from `state.dialogs`: one modal at a time
//! (question, input, custom clip length, export preset, diagnostic report)
//! and stacked non-modal messages. Outcomes are queued as `DialogResult`s.

pub mod custom_clip_length;
pub mod diagnostic_report;
pub mod export_jobs;
pub mod export_preset;

use egui::{Pos2, Ui, Vec2};
use fthrust_core::capture_config::custom_clip_lengths;
use serde_json::json;

use crate::fonts::Weight;
use crate::state::{AppState, DialogKind, DialogResult};
use crate::tokens::{Color, size};
use crate::widgets::buttons;
use crate::widgets::dialog::{self, Frame};
use crate::widgets::section;

/// What a dialog body reported.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Still open.
    Open,
    /// Closed with a result.
    Done(Option<DialogResult>),
}

/// Draws the front modal (if any) and every message, centred over the
/// window, and queues their results.
pub fn draw_all(ui: &mut Ui, state: &mut AppState) {
    let ctx = ui.ctx().clone();
    // A dialog pushed this frame must not see the key press that opened it
    // (Enter committing a rename would otherwise answer its own question).
    let fresh = state.draft_generation != state.dialogs.generation;
    if fresh {
        state.draft_generation = state.dialogs.generation;
        clear_drafts(state);
    }
    // Enter/Escape are read once and handed to the topmost dialog only:
    // messages stack above the modal, so the last message owns the keys.
    let keys = Keys::read(&ctx);
    let message_count = state.dialogs.messages.len();
    if let Some(kind) = state.dialogs.modal.front().cloned() {
        let modal_keys = if message_count == 0 && !fresh {
            keys
        } else {
            Keys::NONE
        };
        let outcome = draw_centered(&ctx, state, &kind, true, modal_keys);
        if let Outcome::Done(result) = outcome {
            state.dialogs.modal.pop_front();
            clear_drafts(state);
            if let Some(r) = result {
                state.dialogs.results.push(r);
            }
        }
    }
    let messages = state.dialogs.messages.clone();
    let mut keep = Vec::new();
    for (i, kind) in messages.iter().enumerate() {
        // A message pushed this frame (i >= message_count) never sees the
        // keys that opened it.
        let k = if i + 1 == message_count {
            keys
        } else {
            Keys::NONE
        };
        if draw_centered(&ctx, state, kind, false, k) == Outcome::Open {
            keep.push(kind.clone());
        }
    }
    state.dialogs.messages = keep;
}

fn clear_drafts(state: &mut AppState) {
    state.input_draft = None;
    state.clip_length_draft = None;
    state.export_preset_draft = None;
}

/// The keys a dialog may act on this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    /// Escape pressed.
    pub escape: bool,
    /// Enter pressed.
    pub enter: bool,
}

impl Keys {
    /// No keys.
    pub const NONE: Keys = Keys {
        escape: false,
        enter: false,
    };

    fn read(ctx: &egui::Context) -> Keys {
        ctx.input(|i| Keys {
            escape: i.key_pressed(egui::Key::Escape),
            enter: i.key_pressed(egui::Key::Enter),
        })
    }
}

fn draw_centered(
    ctx: &egui::Context,
    state: &mut AppState,
    kind: &DialogKind,
    modal: bool,
    keys: Keys,
) -> Outcome {
    let t = state.tokens.clone();
    let mut outcome = Outcome::Open;
    let Keys { escape, enter } = keys;
    let title = title_of(kind);
    let width = width_of(kind);
    let mut icons = std::mem::take(&mut state.icons);
    // The frame calls body then actions in sequence; the cell lets both borrow the state.
    let cell = std::cell::RefCell::new(&mut *state);
    let frame = Frame::new(&title, width).show_centered(
        ctx,
        &t,
        &mut icons,
        modal,
        |ui| body(ui, &mut cell.borrow_mut(), kind),
        |ui| outcome = actions(ui, &mut cell.borrow_mut(), kind, enter),
    );
    state.icons = icons;
    if frame.close_clicked || escape {
        outcome = Outcome::Done(cancel_result(kind));
    }
    outcome
}

/// Draws a dialog at `pos` inside `ui` (tests render dialogs at the origin).
pub fn draw_at(ui: &mut Ui, state: &mut AppState, kind: &DialogKind, pos: Pos2) -> Outcome {
    let t = state.tokens.clone();
    let mut outcome = Outcome::Open;
    let enter = ui.ctx().input(|i| i.key_pressed(egui::Key::Enter));
    let title = title_of(kind);
    let width = width_of(kind);
    let mut icons = std::mem::take(&mut state.icons);
    let cell = std::cell::RefCell::new(&mut *state);
    let frame = Frame::new(&title, width).show_at(
        ui,
        &t,
        &mut icons,
        pos,
        |ui| body(ui, &mut cell.borrow_mut(), kind),
        |ui| outcome = actions(ui, &mut cell.borrow_mut(), kind, enter),
    );
    state.icons = icons;
    if frame.close_clicked {
        outcome = Outcome::Done(cancel_result(kind));
    }
    outcome
}

/// The dialog's title (the bench's dump reports it).
pub fn title_of(kind: &DialogKind) -> String {
    match kind {
        DialogKind::Message { title, .. }
        | DialogKind::Question { title, .. }
        | DialogKind::Input { title, .. } => title.clone(),
        DialogKind::CustomClipLength { .. } => "Custom clip length".into(),
        DialogKind::ExportPreset { preset } => {
            if preset.is_some() {
                "Edit export preset".into()
            } else {
                "New export preset".into()
            }
        }
        DialogKind::DiagnosticReport { .. } => "Diagnostic report exported".into(),
    }
}

fn width_of(kind: &DialogKind) -> f32 {
    match kind {
        DialogKind::Message { .. } => 340.0,
        DialogKind::Question { .. } | DialogKind::Input { .. } => 360.0,
        DialogKind::CustomClipLength { .. } => 340.0,
        DialogKind::ExportPreset { .. } => 420.0,
        DialogKind::DiagnosticReport { .. } => 480.0,
    }
}

fn cancel_result(kind: &DialogKind) -> Option<DialogResult> {
    match kind {
        DialogKind::Question { tag, .. } => Some(DialogResult::Answer {
            tag: tag.clone(),
            yes: false,
        }),
        DialogKind::Input { tag, .. } => Some(DialogResult::Input {
            tag: tag.clone(),
            text: None,
        }),
        DialogKind::CustomClipLength { .. } => Some(DialogResult::ClipLength(None)),
        DialogKind::ExportPreset { .. } => Some(DialogResult::ExportPreset(None)),
        _ => None,
    }
}

fn body(ui: &mut Ui, state: &mut AppState, kind: &DialogKind) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    match kind {
        DialogKind::Message { message, .. } | DialogKind::Question { message, .. } => {
            dialog::message_body(ui, &t, message, w);
        }
        DialogKind::Input { label, text, .. } => {
            section::label_wide(ui, &t, label, size::LABEL, Weight::W600, Color::Text, None);
            let mut draft = state.input_draft.clone().unwrap_or_else(|| text.clone());
            let resp = dialog::input_field(ui, &t, &mut draft, w, "fthrDialogInput");
            if state.input_draft.is_none() {
                resp.request_focus();
            }
            state.input_draft = Some(draft);
        }
        DialogKind::CustomClipLength { seconds } => custom_clip_length::body(ui, state, *seconds),
        DialogKind::ExportPreset { preset } => export_preset::body(ui, state, preset.as_ref()),
        DialogKind::DiagnosticReport { zip } => diagnostic_report::body(ui, state, zip),
    }
}

fn actions(ui: &mut Ui, state: &mut AppState, kind: &DialogKind, enter: bool) -> Outcome {
    let t = state.tokens.clone();
    match kind {
        DialogKind::Message { .. } => {
            if buttons::primary(ui, &t, "OK", "QPushButton[OK]").clicked() || enter {
                return Outcome::Done(None);
            }
        }
        DialogKind::Question { tag, .. } => {
            let yes = buttons::primary(ui, &t, "Yes", "QPushButton[Yes]").clicked();
            let no = buttons::outline(ui, &t, "No", "QPushButton[No]").clicked();
            if yes {
                return Outcome::Done(Some(DialogResult::Answer {
                    tag: tag.clone(),
                    yes: true,
                }));
            }
            if no || enter {
                // "No" has the focus, so Enter answers No like the reference.
                return Outcome::Done(Some(DialogResult::Answer {
                    tag: tag.clone(),
                    yes: false,
                }));
            }
        }
        DialogKind::Input { tag, text, .. } => {
            let ok = buttons::primary(ui, &t, "OK", "QPushButton[OK]").clicked();
            let cancel = buttons::outline(ui, &t, "Cancel", "QPushButton[Cancel]").clicked();
            if ok || enter {
                let value = state.input_draft.clone().unwrap_or_else(|| text.clone());
                return Outcome::Done(Some(DialogResult::Input {
                    tag: tag.clone(),
                    text: Some(value),
                }));
            }
            if cancel {
                return Outcome::Done(Some(DialogResult::Input {
                    tag: tag.clone(),
                    text: None,
                }));
            }
        }
        DialogKind::CustomClipLength { .. } => {
            return custom_clip_length::actions(ui, state, enter);
        }
        DialogKind::ExportPreset { preset } => {
            return export_preset::actions(ui, state, preset.as_ref(), enter);
        }
        DialogKind::DiagnosticReport { .. } => {
            if buttons::primary(ui, &t, "Done", "QPushButton[Done]").clicked() || enter {
                return Outcome::Done(None);
            }
        }
    }
    Outcome::Open
}

/// Applies results the shell owns (custom clip lengths) and leaves the
/// rest queued for the page that asked.
pub fn dispatch_results(state: &mut AppState) {
    let results = state.dialogs.take_results();
    for r in results {
        match r {
            DialogResult::ClipLength(Some(seconds)) => {
                let mut lengths =
                    custom_clip_lengths(state.settings.get_or_default("custom_clip_lengths"));
                if !lengths.contains(&seconds) {
                    lengths.push(seconds);
                    lengths.sort_unstable();
                    state.set_setting("custom_clip_lengths", json!(lengths));
                    state.hotkeys.sync_custom_clips(&lengths);
                    let _ = state.hotkeys.save(&state.paths);
                    state.hotkeys_changed = true;
                    state.restart_pending = true;
                }
            }
            DialogResult::ClipLength(None) => {}
            other => state.dialogs.results.push(other),
        }
    }
}

/// Serves shell-level requests that end in a dialog (the diagnostic export).
pub fn service(state: &mut AppState) {
    if state.diagnostic_export_requested {
        state.diagnostic_export_requested = false;
        match diagnostic_report::export(state) {
            Ok(zip) => {
                state.diagnostic_report_zip = Some(zip.clone());
                state
                    .dialogs
                    .push_modal(DialogKind::DiagnosticReport { zip });
            }
            Err(e) => state.dialogs.push_message("Diagnostic report failed", e),
        }
    }
    let _ = Vec2::ZERO;
}
