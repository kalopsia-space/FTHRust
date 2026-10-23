//! `HotkeyPopup`: one row per action (keyboard binding + Clear, controller
//! binding + Clear) and the Linux notes: either the portal note or the
//! compositor's setup instructions built from the private socket.

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2};
use fthrust_core::hotkeys::{HotkeyAction, normalize_keyboard_combo};

use crate::fidelity;
use crate::fonts::Weight;
use crate::motion;
use crate::shell::toolbar::clip_length_label;
use crate::state::AppState;
use crate::tokens::{Color, Tokens, geom, size};
use crate::widgets::{galley, lerp, paint_text, section};

/// `setMinimumWidth(550)`.
pub const MIN_WIDTH: f32 = 550.0;
const MARGINS: (f32, f32, f32, f32) = (18.0, 14.0, 18.0, 16.0);
const SPACING: f32 = 12.0;
const ROW_SPACING: f32 = 10.0;
const ROW_HINT: f32 = 86.0;
const ACTION_W: f32 = 108.0;
const CLEAR_W: f32 = 46.0;

/// `format_keyboard_combo`: normalised, or `Unset`.
pub fn display_combo(value: &str) -> String {
    let s = normalize_keyboard_combo(value).unwrap_or_default();
    if s.is_empty() { "Unset".into() } else { s }
}

/// `detect_compositor` (env based).
pub fn detect_compositor() -> String {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let wayland = !env("WAYLAND_DISPLAY").is_empty()
        || env("XDG_SESSION_TYPE").eq_ignore_ascii_case("wayland");
    let x11 = !env("DISPLAY").is_empty();
    if !wayland && !x11 {
        return "none".into();
    }
    if wayland {
        let desktop = env("XDG_CURRENT_DESKTOP").to_lowercase();
        let session = env("XDG_SESSION_DESKTOP").to_lowercase();
        if !env("HYPRLAND_INSTANCE_SIGNATURE").is_empty()
            || desktop.contains("hyprland")
            || session.contains("hyprland")
        {
            return "hyprland".into();
        }
        if desktop.contains("kde") || session.contains("plasma") || session.contains("kde") {
            return "kwin".into();
        }
        if desktop.contains("gnome") || session.contains("gnome") {
            return "gnome".into();
        }
        if desktop.contains("sway") {
            return "sway".into();
        }
        return "wayland".into();
    }
    "x11".into()
}

/// `HotkeyManager.setup_instructions` with the `fthrust --hotkey` client
/// command (`fthrust_core::hotkeys::hyprland::setup_instructions`).
pub fn setup_instructions(compositor: &str, client_exe: &str, sock: &str) -> String {
    fthrust_core::hotkeys::hyprland::setup_instructions(
        compositor,
        std::path::Path::new(client_exe),
        std::path::Path::new(sock),
    )
}

/// The rows, in order: CAPTURE CLIP, CLIP <len> per custom length, SCREENSHOT, CONFIRM GAME, DISMISS GAME.
pub fn actions(state: &AppState) -> Vec<(String, String)> {
    let mut out = vec![("CAPTURE CLIP".to_owned(), "save_clip".to_owned())];
    let mut customs = state.hotkeys.custom_clip_lengths();
    customs.sort_unstable();
    for n in customs {
        out.push((
            format!("CLIP {}", clip_length_label(n as i64)),
            HotkeyAction::SaveCustomClip(n).id(),
        ));
    }
    out.push(("SCREENSHOT".into(), "save_screenshot".into()));
    out.push(("CONFIRM GAME".into(), "confirm_game_detection".into()));
    out.push(("DISMISS GAME".into(), "dismiss_game_detection".into()));
    out
}

/// `_record_button_qss`: SURFACE_1 (ACCENT_SOFT recording), 1 px BORDER
/// (ACCENT recording), r6, 15 px 600 text left with 8 px padding; TEXT, or
/// TEXT_DIM when unset, ACCENT when recording; hover ACCENT border + text.
#[allow(clippy::too_many_arguments)]
fn binding_button(
    ui: &mut Ui,
    t: &Tokens,
    rect: Rect,
    text: &str,
    muted: bool,
    recording: bool,
    enabled: bool,
    id_salt: &str,
    name: &str,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let resp = ui.interact(rect, ui.scope_id().with(id_salt), sense);
    let hovered = enabled && resp.hovered();
    let blend = motion::hover(ui.ctx(), resp.id.with("hover"), hovered, t.reduce_motion);
    let (bg, border, fg) = if recording {
        (
            t.color(Color::AccentSoft),
            t.color(Color::Accent),
            t.color(Color::Accent),
        )
    } else {
        let base = if !enabled {
            Color::TextMuted
        } else if muted {
            Color::TextDim
        } else {
            Color::Text
        };
        (
            t.color(Color::Surface1),
            lerp(t.color(Color::Border), t.color(Color::Accent), blend),
            lerp(t.color(base), t.color(Color::Accent), blend),
        )
    };
    let radius = CornerRadius::same(geom::RADIUS as u8);
    ui.painter().rect_filled(rect, radius, bg);
    ui.painter()
        .rect_stroke(rect, radius, Stroke::new(1.0, border), StrokeKind::Inside);
    let g = galley(ui, text, size::BODY, Weight::W600, fg);
    paint_text(ui, g, rect, Align::Min, 9.0, fg);
    fidelity::mark(ui, name, rect)
        .sample(
            "fill",
            Pos2::new(rect.min.x + 3.0, rect.center().y),
            if recording {
                Color::AccentSoft
            } else {
                Color::Surface1
            },
        )
        .edge(if recording {
            Color::Accent
        } else {
            Color::Border
        })
        .text(text, size::BODY, Weight::W600)
        .done();
    resp
}

/// `_clear_button_qss`: transparent, 1 px BORDER, r6, TEXT_DIM 12 px 600,
/// padding 0 7; hover ACCENT; disabled HAIRLINE border / TEXT_GHOST.
pub fn clear_button(
    ui: &mut Ui,
    t: &Tokens,
    rect: Rect,
    text: &str,
    enabled: bool,
    name: &str,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let resp = ui.interact(rect, ui.scope_id().with(name), sense);
    let hovered = enabled && resp.hovered();
    let blend = motion::hover(ui.ctx(), resp.id.with("hover"), hovered, t.reduce_motion);
    let (border, fg, border_token) = if enabled {
        (
            lerp(t.color(Color::Border), t.color(Color::Accent), blend),
            lerp(t.color(Color::TextDim), t.color(Color::Accent), blend),
            Color::Border,
        )
    } else {
        (
            t.color(Color::Hairline),
            t.color(Color::TextGhost),
            Color::Hairline,
        )
    };
    let radius = CornerRadius::same(geom::RADIUS as u8);
    ui.painter()
        .rect_stroke(rect, radius, Stroke::new(1.0, border), StrokeKind::Inside);
    let g = galley(ui, text, size::MICRO, Weight::W600, fg);
    paint_text(ui, g, rect, Align::Center, 0.0, fg);
    fidelity::mark(ui, name, rect)
        .edge(border_token)
        .text(text, size::MICRO, Weight::W600)
        .done();
    resp
}

/// Chord text for an egui key event (`Ctrl+Shift+F10`).
fn chord_for(modifiers: egui::Modifiers, key: egui::Key) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if modifiers.ctrl || modifiers.command {
        parts.push("Ctrl");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    let name = key.name();
    let mut s = parts.join("+");
    if !s.is_empty() {
        s.push('+');
    }
    s.push_str(name);
    s
}

/// The popup body.
pub fn content(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let outer = ui.max_rect();
    let inner = Rect::from_min_max(
        outer.min + Vec2::new(MARGINS.0, MARGINS.1),
        Pos2::new(outer.max.x - MARGINS.2, f32::INFINITY),
    );
    let mut body = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    body.spacing_mut().item_spacing = Vec2::new(0.0, SPACING);
    let w = inner.width();
    // Footer text first: Qt's layout gives the wrapped label's hint slack to the title.
    let portal = state.hotkeys_portal_active;
    let instructions = (!portal).then(|| {
        setup_instructions(
            &state.compositor,
            &state.client_exe,
            &state.hotkey_socket_path,
        )
    });
    // Qt sizes the popup at its hint width (the widest row hint: borders,
    // margins, label, 100 px keyboard button, Clear, 108 px controller
    // button, Clear and four 8 px gaps) and shows it at the 550 px minimum.
    let hint_w = 2.0 + 24.0 + ACTION_W + 100.0 + CLEAR_W + 108.0 + CLEAR_W + 4.0 * 8.0;
    let line = crate::fonts::qt_wrapped_line_height_named(crate::fonts::MONO_FAMILY, size::MICRO);
    let slack = instructions
        .as_deref()
        .map(|s| mono_rows(&body, s, hint_w).saturating_sub(mono_rows(&body, s, w)) as f32 * line)
        .unwrap_or(0.0);
    let title_h = crate::fonts::qt_wrapped_line_height(size::MICRO, Weight::W600) + slack;
    let top = body.cursor().min;
    section::label_in(
        &mut body,
        &t,
        Rect::from_min_size(top, Vec2::new(w, title_h)),
        "HOTKEYS",
        size::MICRO,
        Weight::W600,
        Color::Text,
        None,
    );
    body.allocate_exact_size(Vec2::new(w, title_h), egui::Sense::hover());
    // Rows inside a fixed-height scroll area.
    let rows = actions(state);
    let n = rows.len().max(1);
    let scroll_h = (n as f32 * ROW_HINT).min(540.0);
    let top = body.cursor().min;
    let share = (scroll_h - (n as f32 - 1.0) * ROW_SPACING) / n as f32;
    let mut cum = 0.0f32;
    let mut prev = 0.0f32;
    let mut y = top.y;
    let mut commit: Option<(String, String)> = None;
    let mut clears: Vec<String> = Vec::new();
    let mut toggle_recording: Option<String> = None;
    let mut scroll = body.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(top, Vec2::new(w, scroll_h))),
    );
    scroll.set_clip_rect(Rect::from_min_size(top, Vec2::new(w, scroll_h)));
    for (i, (label, action)) in rows.iter().enumerate() {
        cum += share;
        let h = cum.round() - prev;
        prev = cum.round();
        let row = Rect::from_min_size(Pos2::new(top.x, y), Vec2::new(w, h));
        let name = if i == 0 {
            "hotkeyActionRow".to_owned()
        } else {
            format!("hotkeyActionRow#{}", i + 1)
        };
        let suffix = |base: &str| {
            if i == 0 {
                base.to_owned()
            } else {
                format!("{base}#{}", i + 1)
            }
        };
        scroll.painter().rect_filled(
            row,
            CornerRadius::same(geom::RADIUS as u8),
            t.color(Color::Surface1),
        );
        scroll.painter().rect_stroke(
            row,
            CornerRadius::same(geom::RADIUS as u8),
            Stroke::new(1.0, t.color(Color::Border)),
            StrokeKind::Inside,
        );
        fidelity::mark(&scroll, name, row)
            .sample(
                "fill",
                Pos2::new(row.min.x + 4.0, row.center().y),
                Color::Surface1,
            )
            .edge(Color::Border)
            .done();
        // Grid: margins (12, 10, 12, 10) inside the 1 px border, hspacing 8, vspacing 6.
        let grid = Rect::from_min_max(
            row.min + Vec2::new(13.0, 11.0),
            row.max - Vec2::new(13.0, 11.0),
        );
        let row1_h = 30.0;
        let row0_h = grid.height() - 6.0 - row1_h;
        let stretch_w = ((grid.width() - ACTION_W - 2.0 * CLEAR_W - 4.0 * 8.0) / 2.0).floor();
        let x1 = grid.min.x + ACTION_W + 8.0;
        let x2 = x1 + stretch_w + 8.0;
        let x3 = x2 + CLEAR_W + 8.0;
        let x4 = x3 + stretch_w + 8.0;
        section::label_in(
            &mut scroll,
            &t,
            Rect::from_min_size(grid.min, Vec2::new(ACTION_W, grid.height())),
            label,
            size::LABEL,
            Weight::W600,
            Color::Text,
            Some(&format!("QLabel[{label}]")),
        );
        section::label_in(
            &mut scroll,
            &t,
            Rect::from_min_size(
                Pos2::new(x1, grid.min.y),
                Vec2::new(stretch_w + 8.0 + CLEAR_W, row0_h),
            ),
            "Keyboard",
            size::MICRO,
            Weight::W600,
            Color::TextDim,
            Some(&suffix("QLabel[Keyboard]")),
        );
        section::label_in(
            &mut scroll,
            &t,
            Rect::from_min_size(
                Pos2::new(x3, grid.min.y),
                Vec2::new(stretch_w + 8.0 + CLEAR_W, row0_h),
            ),
            "Controller",
            size::MICRO,
            Weight::W600,
            Color::TextDim,
            Some(&suffix("QLabel[Controller]")),
        );
        let by = grid.min.y + row0_h + 6.0;
        let recording = state.hotkey_recording.as_deref() == Some(action.as_str());
        let bound = state.hotkeys.get(action).unwrap_or("").to_owned();
        let text = if recording {
            "Press keys".to_owned()
        } else {
            display_combo(&bound)
        };
        let kb_rect = Rect::from_min_size(Pos2::new(x1, by), Vec2::new(stretch_w, row1_h));
        let kname = format!("_KeyboardBindingButton[{text}]");
        if binding_button(
            &mut scroll,
            &t,
            kb_rect,
            &text,
            bound.is_empty(),
            recording,
            true,
            &format!("kb:{action}"),
            &kname,
        )
        .clicked()
        {
            toggle_recording = Some(action.clone());
        }
        let clear_rect = Rect::from_min_size(Pos2::new(x2, by), Vec2::new(CLEAR_W, row1_h));
        let clear_name = if i == 0 {
            "QPushButton[Clear]".to_owned()
        } else {
            format!("QPushButton[Clear]#{}", 2 * i + 1)
        };
        if clear_button(
            &mut scroll,
            &t,
            clear_rect,
            "Clear",
            !bound.is_empty(),
            &clear_name,
        )
        .clicked()
        {
            clears.push(action.clone());
        }
        let ctrl_rect = Rect::from_min_size(Pos2::new(x3, by), Vec2::new(stretch_w, row1_h));
        let ctrl_text = state
            .hotkeys
            .controller(action)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| "Add".into());
        binding_button(
            &mut scroll,
            &t,
            ctrl_rect,
            &ctrl_text,
            true,
            false,
            false,
            &format!("ctrl:{action}"),
            &suffix(&format!("_ControllerBindingButton[{ctrl_text}]")),
        );
        let clear2 = Rect::from_min_size(Pos2::new(x4, by), Vec2::new(CLEAR_W, row1_h));
        clear_button(
            &mut scroll,
            &t,
            clear2,
            "Clear",
            false,
            &format!("QPushButton[Clear]#{}", 2 * i + 2),
        );
        y += h + ROW_SPACING;
    }
    body.allocate_exact_size(Vec2::new(w, scroll_h), egui::Sense::hover());
    // Linux notes.
    if portal {
        section::label_wrapped(
            &mut body,
            &t,
            "Your desktop manages these shortcuts. If it keeps a different key, change it in the desktop shortcut settings.",
            size::MICRO,
            Weight::W400,
            Color::TextDim,
            w,
            6.0,
        );
        let top = body.cursor().min;
        let g = galley(
            &body,
            "EDIT IN DESKTOP SETTINGS",
            size::MICRO,
            Weight::W600,
            Color32::WHITE,
        );
        let bw = g.size().x.round() + 14.0 + 2.0;
        let rect = Rect::from_min_size(top, Vec2::new(bw, 30.0));
        if clear_button(
            &mut body,
            &t,
            rect,
            "EDIT IN DESKTOP SETTINGS",
            true,
            "QPushButton[EDIT IN DESKTOP SETTINGS]",
        )
        .clicked()
        {
            state.open_desktop_shortcuts_requested = true;
        }
        body.allocate_exact_size(Vec2::new(w, 30.0), egui::Sense::hover());
    } else if let Some(text) = &instructions {
        instructions_label(&mut body, &t, text, w);
        if state.compositor == "hyprland" {
            // FTHRust addition: one click copies the `save_clip` bind line.
            let top = body.cursor().min;
            let g = galley(
                &body,
                "COPY LINE",
                size::MICRO,
                Weight::W600,
                Color32::WHITE,
            );
            let bw = g.size().x.round() + 14.0 + 2.0;
            let rect = Rect::from_min_size(top, Vec2::new(bw, 30.0));
            if clear_button(
                &mut body,
                &t,
                rect,
                "COPY LINE",
                true,
                "QPushButton[COPY LINE]",
            )
            .clicked()
            {
                let combo = state.hotkeys.get("save_clip").unwrap_or("").to_owned();
                let line = fthrust_core::hotkeys::hyprland::bind_line(
                    "save_clip",
                    &combo,
                    std::path::Path::new(&state.client_exe),
                    std::path::Path::new(&state.hotkey_socket_path),
                );
                ui.ctx().copy_text(line.clone());
                state.last_copied_bind_line = Some(line);
            }
            body.allocate_exact_size(Vec2::new(w, 30.0), egui::Sense::hover());
        }
    }
    // Input handling for a recording in progress.
    if let Some(action) = toggle_recording {
        state.hotkey_recording = if state.hotkey_recording.as_deref() == Some(action.as_str()) {
            None
        } else {
            Some(action)
        };
    }
    if let Some(action) = state.hotkey_recording.clone() {
        let events = ui.ctx().input(|i| i.events.clone());
        for ev in events {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = ev
            {
                if key == egui::Key::Escape {
                    state.hotkey_recording = None;
                    break;
                }
                commit = Some((action.clone(), chord_for(modifiers, key)));
                break;
            }
        }
        ui.ctx().request_repaint();
    }
    if let Some((action, chord)) = commit {
        state.hotkeys.set_keyboard(&action, &chord);
        state.hotkey_recording = None;
        let _ = state.hotkeys.save(&state.paths);
        state.hotkeys_changed = true;
    }
    for action in clears {
        state.hotkeys.set_keyboard(&action, "");
        let _ = state.hotkeys.save(&state.paths);
        state.hotkeys_changed = true;
    }
    let bottom = body.cursor().min.y - SPACING + MARGINS.3;
    ui.allocate_rect(
        Rect::from_min_max(outer.min, Pos2::new(outer.max.x, bottom)),
        egui::Sense::hover(),
    );
}

fn mono_rows(ui: &Ui, text: &str, w: f32) -> usize {
    let font = egui::FontId::new(
        size::MICRO,
        egui::FontFamily::Name(crate::fonts::MONO_FAMILY.into()),
    );
    ui.painter()
        .layout(text.to_owned(), font, Color32::WHITE, w.max(1.0))
        .rows
        .len()
        .max(1)
}

/// The compositor instructions: monospace 12 px TEXT_DIM, word-wrapped,
/// `padding-top: 6px`.
fn instructions_label(body: &mut Ui, t: &Tokens, text: &str, w: f32) {
    let text_h = mono_rows(body, text, w) as f32
        * crate::fonts::qt_wrapped_line_height_named(crate::fonts::MONO_FAMILY, size::MICRO);
    let h = text_h + 6.0;
    let (rect, _) = body.allocate_exact_size(Vec2::new(w, h), egui::Sense::hover());
    let font = egui::FontId::new(
        size::MICRO,
        egui::FontFamily::Name(crate::fonts::MONO_FAMILY.into()),
    );
    let g = body
        .painter()
        .layout(text.to_owned(), font, t.color(Color::TextDim), w);
    body.painter().galley(
        Pos2::new(rect.min.x, rect.min.y + 6.0),
        g,
        t.color(Color::TextDim),
    );
    fidelity::mark(
        body,
        format!("QLabel[{}]", crate::fidelity::marker_label(text)),
        rect,
    )
    .text(text, size::MICRO, Weight::W400)
    .done();
}
