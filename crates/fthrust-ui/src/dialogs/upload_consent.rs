//! The uploader's Terms and Privacy Policy (read only, opened from the
//! Upload settings link) and Catbox's consent dialog
//! (`_provider_consent_dialog`). The reference also gated enabling uploads
//! on accepting our own documents, and refused every upload once their
//! version changed; they are a link instead.

use egui::Ui;
use fthrust_core::upload::{CATBOX_LEGAL_URL, PRIVACY_TEXT, TERMS_TEXT};

use super::Outcome;
use crate::fonts::Weight;
use crate::state::{AppState, DialogResult};
use crate::tokens::{Color, size};
use crate::widgets::buttons::{self, Kind, Opts};
use crate::widgets::check;
use crate::widgets::dialog;
use crate::widgets::section;

/// Title of the uploader documents dialog.
pub const TERMS_TITLE: &str = "Uploader Terms & Privacy";
/// Title of the Catbox consent dialog.
pub const CATBOX_TITLE: &str = "Connect Catbox";
const BODY: &str = "This separately built component contains all provider network code. FTHR Clips Core remains upload-free.";
const CATBOX_BODY: &str = "Catbox stores the clip, filename, file size, upload time, and your IP address. Files are publicly accessible to anyone with the link and may remain available until deleted. Review Catbox\u{2019}s Terms, Privacy Policy, and Acceptable Use Policy before continuing.";
const CATBOX_LINK: &str = "OPEN CATBOX TERMS, PRIVACY & ACCEPTABLE USE";
const CATBOX_CHECK: &str =
    "I accept Catbox\u{2019}s Terms, Privacy Policy, and Acceptable Use Policy.";

/// Catbox's accept button is enabled once its box is ticked.
pub fn catbox_accept_enabled(checked: bool) -> bool {
    checked
}

/// Each document box shares what the window leaves after the rest of the
/// dialog (about 500 px), between 80 and 150 px.
fn box_height(ui: &Ui) -> f32 {
    let screen = ui.ctx().content_rect().height();
    ((screen - 500.0) / 2.0).clamp(80.0, 150.0)
}

fn document(ui: &mut Ui, state: &AppState, label: &str, text: &str, w: f32) {
    let t = &state.tokens;
    section::label_wide(ui, t, label, size::LABEL, Weight::W600, Color::Text, None);
    ui.add_space(6.0);
    let frame = egui::Frame::NONE
        .fill(t.color(Color::Surface1))
        .stroke(egui::Stroke::new(1.0, t.color(Color::Border)))
        .corner_radius(6)
        .inner_margin(10);
    frame.show(ui, |ui| {
        ui.set_width(w - 22.0);
        egui::ScrollArea::vertical()
            .id_salt(label)
            .max_height(box_height(ui))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(text)
                        .font(crate::fonts::id(size::LABEL, Weight::W400))
                        .color(t.color(Color::TextDim)),
                );
            });
    });
    ui.add_space(10.0);
}

/// The uploader documents body.
pub fn terms_body(ui: &mut Ui, state: &mut AppState) {
    let w = ui.available_width();
    let t = state.tokens.clone();
    dialog::message_body(ui, &t, BODY, w);
    ui.add_space(12.0);
    document(ui, state, "Terms of service", TERMS_TEXT, w);
    document(ui, state, "Privacy policy", PRIVACY_TEXT, w);
}

/// The Catbox consent body.
pub fn catbox_body(ui: &mut Ui, state: &mut AppState) {
    let w = ui.available_width();
    let t = state.tokens.clone();
    dialog::message_body(ui, &t, CATBOX_BODY, w);
    ui.add_space(10.0);
    if buttons::ghost(ui, &t, CATBOX_LINK, "catboxLegalLink").clicked() {
        crate::shell::open_url(CATBOX_LEGAL_URL);
    }
    ui.add_space(10.0);
    check::checkbox(
        ui,
        &t,
        &mut state.catbox_check,
        CATBOX_CHECK,
        "QCheckBox[catbox]",
    );
}

/// Cancel and accept for the Catbox consent.
pub fn catbox_actions(ui: &mut Ui, state: &mut AppState, context: &str) -> Outcome {
    let t = state.tokens.clone();
    let enabled = catbox_accept_enabled(state.catbox_check);
    let accept = buttons::button_with(
        ui,
        &t,
        None,
        Kind::Primary,
        "Accept & continue",
        "QPushButton[Accept & continue]",
        &Opts {
            enabled,
            ..Opts::default()
        },
    )
    .clicked();
    let cancel = buttons::outline(ui, &t, "Cancel", "QPushButton[Cancel]").clicked();
    if accept && enabled {
        return Outcome::Done(Some(DialogResult::CatboxConsent {
            accepted: true,
            context: context.to_owned(),
        }));
    }
    if cancel {
        return Outcome::Done(Some(DialogResult::CatboxConsent {
            accepted: false,
            context: context.to_owned(),
        }));
    }
    Outcome::Open
}
