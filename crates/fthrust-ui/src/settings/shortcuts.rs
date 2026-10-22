//! The Shortcuts tab (`KeyboardShortcutSettings`): keyboard navigation
//! bindings per category with chord recording, conflict reporting and
//! defaults restore.

use egui::{Align, Color32, Pos2, Rect, Sense, Ui, Vec2};
use fthrust_core::navigation::{BINDING_DEFINITIONS, NavigationBindings};

use super::space;
use crate::fidelity;
use crate::fonts::Weight;
use crate::state::AppState;
use crate::tokens::{Color, size};
use crate::widgets::buttons::{self, Kind, Opts};
use crate::widgets::{check, galley, paint_text, section};

const HINT: &str = "Click a binding, hold the keys together, then release to save. Tab + S means hold Tab and press S. Shortcuts work while FTHR is active; text fields keep their normal editing keys. Up/Down scrolls Clips; Shift+Up/Down switches sidebar destinations.";
/// Page margins (QVBoxLayout default).
pub const MARGIN: f32 = 9.0;
/// Item spacing (QVBoxLayout default).
pub const SPACING: f32 = 6.0;

/// Restores every default binding and writes them to settings.
pub fn restore_defaults(state: &mut AppState) {
    let mut fresh = NavigationBindings::from_settings(&state.settings);
    for def in &BINDING_DEFINITIONS {
        let _ = fresh.set(def.action, "");
    }
    for def in &BINDING_DEFINITIONS {
        let _ = fresh.set(def.action, def.default);
    }
    state.nav = fresh;
    state.nav.write_to(&mut state.settings);
    let _ = state.settings.save();
    state.shortcut_error.clear();
}

fn save_nav(state: &mut AppState) {
    state.nav.write_to(&mut state.settings);
    let _ = state.settings.save();
}

/// Chord text for a key event (`Tab+S`, `Shift+Down`).
fn chord_token(key: egui::Key) -> String {
    match key {
        egui::Key::ArrowUp => "Up".into(),
        egui::Key::ArrowDown => "Down".into(),
        egui::Key::ArrowLeft => "Left".into(),
        egui::Key::ArrowRight => "Right".into(),
        egui::Key::Enter => "Enter".into(),
        egui::Key::Escape => "Esc".into(),
        egui::Key::Space => "Space".into(),
        other => other.name().to_owned(),
    }
}

/// The Shortcuts page body: the page is its own scroll area with 9 px
/// margins and 6 px spacing.
pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let outer_w = ui.available_width();
    let origin = ui.cursor().min;
    let mut body = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        origin + Vec2::new(MARGIN, MARGIN),
        Vec2::new(outer_w - 2.0 * MARGIN, f32::INFINITY),
    )));
    body.spacing_mut().item_spacing = Vec2::ZERO;
    let w = body.available_width();
    section::label_wide(
        &mut body,
        &t,
        "Beta",
        size::BODY_L,
        Weight::W400,
        Color::Accent,
        None,
    );
    space(&mut body, SPACING);
    section::label_wrapped_fit(
        &mut body,
        &t,
        HINT,
        size::BODY,
        Weight::W400,
        Color::TextDim,
        w,
    );
    space(&mut body, SPACING);
    let mut enabled = state.nav.is_enabled();
    if check::checkbox_with(
        &mut body,
        &t,
        &mut enabled,
        "Enable shortcuts",
        true,
        true,
        "QCheckBox[Enable shortcuts]",
    )
    .changed()
    {
        state.nav.set_enabled(enabled);
        save_nav(state);
    }
    space(&mut body, SPACING);
    // Error label (empty labels are font-height tall in Qt).
    let err = state.shortcut_error.clone();
    if err.is_empty() {
        let h = crate::fonts::qt_height(size::BODY, Weight::W400);
        let (rect, _) = body.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        fidelity::mark(&body, "shortcutError", rect).done();
    } else {
        section::label_wrapped_fit(
            &mut body,
            &t,
            &err,
            size::BODY,
            Weight::W400,
            Color::Delete,
            w,
        );
    }
    space(&mut body, SPACING);
    let mut group: Option<&str> = None;
    let mut clear_idx = 0usize;
    let mut record: Option<String> = None;
    let mut clears: Vec<String> = Vec::new();
    for def in &BINDING_DEFINITIONS {
        if group != Some(def.category) {
            space(&mut body, 14.0);
            section::label_wide(
                &mut body,
                &t,
                def.category,
                size::BODY_L,
                Weight::W600,
                Color::Accent,
                None,
            );
            space(&mut body, SPACING);
            let mut on = state.nav.is_category_enabled(def.category);
            let label = format!("Enable {} shortcuts", def.category.to_lowercase());
            if check::checkbox_with(
                &mut body,
                &t,
                &mut on,
                &label,
                enabled,
                true,
                &format!("QCheckBox[{}]", &label[..label.len().min(40)]),
            )
            .changed()
            {
                state.nav.set_category_enabled(def.category, on);
                save_nav(state);
            }
            space(&mut body, SPACING);
            group = Some(def.category);
        }
        let top = body.cursor().min;
        let clear_w = (galley(&body, "Clear", size::LABEL, Weight::W600, Color32::WHITE)
            .size()
            .x
            + 34.0)
            .ceil();
        let bw = 150.0f32.max(
            (galley(
                &body,
                state.nav.get(def.action).unwrap_or(""),
                size::LABEL,
                Weight::W600,
                Color32::WHITE,
            )
            .size()
            .x + 34.0)
                .ceil(),
        );
        let label_w = w - SPACING - bw - SPACING - clear_w;
        section::label_in(
            &mut body,
            &t,
            Rect::from_min_size(top, Vec2::new(label_w, 41.0)),
            def.label,
            size::BODY,
            Weight::W400,
            Color::Text,
            None,
        );
        let recording = state.shortcut_recording.as_deref() == Some(def.action);
        let value = state
            .nav
            .get(def.action)
            .unwrap_or("")
            .replace("Return", "Enter");
        let text = if recording {
            let pending = state.shortcut_pending.join("+");
            if pending.is_empty() {
                "Press shortcut…".to_owned()
            } else {
                pending
            }
        } else if value.is_empty() {
            "Unassigned".to_owned()
        } else {
            value.clone()
        };
        let mut b = body.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + label_w + SPACING, top.y),
            Vec2::new(bw, 41.0),
        )));
        let opts = Opts {
            min_width: bw,
            ..Opts::default()
        };
        if buttons::button_with(
            &mut b,
            &t,
            None,
            Kind::Outline,
            &text,
            &format!("BindingButton[{text}]"),
            &opts,
        )
        .clicked()
        {
            record = Some(def.action.to_owned());
        }
        clear_idx += 1;
        let cname = if clear_idx == 1 {
            "QPushButton[Clear]".to_owned()
        } else {
            format!("QPushButton[Clear]#{clear_idx}")
        };
        let mut c = body.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + w - clear_w, top.y),
            Vec2::new(clear_w, 41.0),
        )));
        if buttons::outline(&mut c, &t, "Clear", &cname).clicked() {
            clears.push(def.action.to_owned());
        }
        body.allocate_exact_size(Vec2::new(w, 41.0), Sense::hover());
        space(&mut body, SPACING);
    }
    space(&mut body, 18.0);
    if buttons::outline(
        &mut body,
        &t,
        "Restore default shortcuts",
        "QPushButton[Restore default shortcuts]",
    )
    .clicked()
    {
        restore_defaults(state);
    }
    let bottom = body.cursor().min.y + MARGIN;
    ui.allocate_rect(
        Rect::from_min_max(origin, Pos2::new(origin.x + outer_w, bottom)),
        Sense::hover(),
    );
    // Recording: keys held together are collected; the chord commits when
    // every key is released (Tab counts as a key, not a modifier).
    if let Some(action) = record {
        if state.shortcut_recording.as_deref() == Some(action.as_str()) {
            state.shortcut_recording = None;
            state.shortcut_pending.clear();
        } else {
            state.shortcut_recording = Some(action);
            state.shortcut_pending.clear();
            state.shortcut_down.clear();
            state.shortcut_error.clear();
        }
    }
    if let Some(action) = state.shortcut_recording.clone() {
        let events = ui.ctx().input(|i| i.events.clone());
        let mut commit = false;
        for ev in events {
            if let egui::Event::Key {
                key,
                pressed,
                modifiers,
                repeat,
                ..
            } = ev
            {
                if repeat {
                    continue;
                }
                let name = chord_token(key);
                if pressed {
                    state.shortcut_down.insert(name.clone());
                    let mut tokens: Vec<String> = Vec::new();
                    if modifiers.alt {
                        tokens.push("Alt".into());
                    }
                    if modifiers.ctrl || modifiers.command {
                        tokens.push("Ctrl".into());
                    }
                    if modifiers.shift {
                        tokens.push("Shift".into());
                    }
                    tokens.push(name);
                    for tk in tokens {
                        if !state.shortcut_pending.contains(&tk) {
                            state.shortcut_pending.push(tk);
                        }
                    }
                } else {
                    state.shortcut_down.remove(&name);
                    if state.shortcut_down.is_empty() && !state.shortcut_pending.is_empty() {
                        commit = true;
                    }
                }
            }
        }
        if commit {
            let chord = state.shortcut_pending.join("+");
            match state.nav.set(&action, &chord) {
                Ok(()) => {
                    state.shortcut_error.clear();
                    save_nav(state);
                }
                Err(e) => state.shortcut_error = e.to_string(),
            }
            state.shortcut_recording = None;
            state.shortcut_pending.clear();
        }
        ui.ctx().request_repaint();
    }
    for action in clears {
        let _ = state.nav.set(&action, "");
        save_nav(state);
    }
    let _ = paint_text;
    let _ = Align::Min;
}
