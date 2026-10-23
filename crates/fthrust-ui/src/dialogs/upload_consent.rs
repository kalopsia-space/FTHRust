//! The uploader consent dialogs (`_legal_install_dialog`,
//! `_provider_consent_dialog`): the Terms and Privacy Policy with two
//! checkboxes, and Catbox's policies with one.

use egui::{Ui, Vec2};
use fthrust_core::upload::{CATBOX_LEGAL_URL, PRIVACY_TEXT, TERMS_TEXT};

use super::Outcome;
use crate::fonts::Weight;
use crate::state::{AppState, DialogResult};
use crate::tokens::{Color, size};
use crate::widgets::buttons::{self, Kind, Opts};
use crate::widgets::check;
use crate::widgets::dialog;
use crate::widgets::section;

/// Title of the uploader consent dialog.
pub const TITLE: &str = "Enable FTHR Upload Extension";
/// Title of the Catbox consent dialog.
pub const CATBOX_TITLE: &str = "Connect Catbox";
const BODY: &str = "This separately built component contains all provider network code. FTHR Clips Core remains upload-free.";
const TERMS_CHECK: &str = "I have read and accept the Terms of Service.";
const PRIVACY_CHECK: &str = "I have read and accept the Privacy Policy.";
const ACCEPT: &str = "ACCEPT & ENABLE UPLOADER";
const CATBOX_BODY: &str = "Catbox stores the clip, filename, file size, upload time, and your IP address. Files are publicly accessible to anyone with the link and may remain available until deleted. Review Catbox\u{2019}s Terms, Privacy Policy, and Acceptable Use Policy before continuing.";
const CATBOX_LINK: &str = "OPEN CATBOX TERMS, PRIVACY & ACCEPTABLE USE";
const CATBOX_CHECK: &str =
    "I accept Catbox\u{2019}s Terms, Privacy Policy, and Acceptable Use Policy.";

/// Accept is enabled once both boxes are ticked.
pub fn accept_enabled(checks: [bool; 2]) -> bool {
    checks[0] && checks[1]
}

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

/// The uploader consent body.
pub fn body(ui: &mut Ui, state: &mut AppState) {
    let w = ui.available_width();
    let t = state.tokens.clone();
    dialog::message_body(ui, &t, BODY, w);
    ui.add_space(12.0);
    document(ui, state, "Terms of service", TERMS_TEXT, w);
    document(ui, state, "Privacy policy", PRIVACY_TEXT, w);
    let [mut terms, mut privacy] = state.consent_checks;
    check::checkbox(ui, &t, &mut terms, TERMS_CHECK, "QCheckBox[terms]");
    ui.add_space(6.0);
    check::checkbox(ui, &t, &mut privacy, PRIVACY_CHECK, "QCheckBox[privacy]");
    state.consent_checks = [terms, privacy];
    let _ = Vec2::ZERO;
}

/// Cancel and accept for the uploader consent.
pub fn actions(ui: &mut Ui, state: &mut AppState) -> Outcome {
    let t = state.tokens.clone();
    let accept = buttons::button_with(
        ui,
        &t,
        None,
        Kind::Primary,
        ACCEPT,
        "QPushButton[accept]",
        &Opts {
            enabled: accept_enabled(state.consent_checks),
            ..Opts::default()
        },
    )
    .clicked();
    let cancel = buttons::outline(ui, &t, "Cancel", "QPushButton[Cancel]").clicked();
    if accept && accept_enabled(state.consent_checks) {
        return Outcome::Done(Some(DialogResult::UploadConsent(true)));
    }
    if cancel {
        return Outcome::Done(Some(DialogResult::UploadConsent(false)));
    }
    Outcome::Open
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
    let mut on = state.consent_checks[0];
    check::checkbox(ui, &t, &mut on, CATBOX_CHECK, "QCheckBox[catbox]");
    state.consent_checks[0] = on;
}

/// Cancel and accept for the Catbox consent.
pub fn catbox_actions(ui: &mut Ui, state: &mut AppState, context: &str) -> Outcome {
    let t = state.tokens.clone();
    let enabled = catbox_accept_enabled(state.consent_checks[0]);
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
