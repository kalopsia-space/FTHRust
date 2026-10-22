//! `CustomClipLengthDialog`: a minutes-only picker (1–15) with a hint and a
//! memory warning from two minutes up.

use egui::{Pos2, Rect, Sense, Ui, Vec2};
use fthrust_core::capture_config::validate_clip_length;

use super::Outcome;
use crate::fonts::Weight;
use crate::state::{AppState, DialogResult};
use crate::tokens::{Color, size};
use crate::widgets::buttons;
use crate::widgets::input::{SpinStyle, spin_box};
use crate::widgets::section;

/// `validate_normal_clip_length`: 5..=900 seconds.
pub fn validate(seconds: i64) -> Result<u32, String> {
    validate_clip_length(seconds).map_err(|e| e.to_string())
}

/// Body: "Length" + spin box (1–15 min), hint, memory warning.
pub fn body(ui: &mut Ui, state: &mut AppState, seconds: u32) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    let mut minutes = state
        .clip_length_draft
        .unwrap_or_else(|| (seconds as f64 / 60.0).round().clamp(1.0, 15.0));
    let top = ui.cursor().min;
    let lw = crate::widgets::galley(ui, "Length", size::BODY, Weight::W400, egui::Color32::WHITE)
        .size()
        .x
        .ceil();
    section::label_in(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(lw, 34.0)),
        "Length",
        size::BODY,
        Weight::W400,
        Color::TextDim,
        None,
    );
    let mut s = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + lw + 10.0, top.y),
        Vec2::new(w - lw - 10.0, 34.0),
    )));
    spin_box(
        &mut s,
        &t,
        &mut state.icons,
        &mut minutes,
        1.0..=15.0,
        1.0,
        0,
        " min",
        SpinStyle::Number,
        true,
        "fthrNumberSpin",
    );
    ui.allocate_exact_size(Vec2::new(w, 34.0), Sense::hover());
    state.clip_length_draft = Some(minutes);
    section::label_wide(
        ui,
        &t,
        "Choose 1–15 minutes.",
        size::MICRO,
        Weight::W400,
        Color::TextMuted,
        None,
    );
    if minutes * 60.0 >= 120.0 {
        section::label_wrapped(
            ui,
            &t,
            "Longer clips use more memory, especially at high quality.",
            size::MICRO,
            Weight::W400,
            Color::TextMuted,
            w,
            0.0,
        );
    }
}

/// Actions: Cancel / Add length.
pub fn actions(ui: &mut Ui, state: &mut AppState, enter: bool) -> Outcome {
    let t = state.tokens.clone();
    let add = buttons::primary(ui, &t, "Add length", "QPushButton[Add length]").clicked();
    let cancel = buttons::outline(ui, &t, "Cancel", "QPushButton[Cancel]").clicked();
    if add || enter {
        let seconds = (state.clip_length_draft.unwrap_or(1.0) * 60.0) as i64;
        return match validate(seconds) {
            Ok(s) => Outcome::Done(Some(DialogResult::ClipLength(Some(s)))),
            Err(e) => {
                state.dialogs.push_message("Clip length is not valid", e);
                Outcome::Open
            }
        };
    }
    if cancel {
        return Outcome::Done(Some(DialogResult::ClipLength(None)));
    }
    Outcome::Open
}
