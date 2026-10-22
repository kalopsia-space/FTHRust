//! The General tab (`_make_general_page`): System, Notifications, Upload,
//! Export Presets, Troubleshooting, Settings Presets, App Credits.

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2};
use fthrust_core::export_presets::{ExportPreset, ExportPresetStore};
use fthrust_core::presets::PresetsStore;
use serde_json::json;

use super::space;
use crate::fidelity;
use crate::fonts::Weight;
use crate::motion;
use crate::shell::toolbar::capture_summary;
use crate::state::{AppState, DialogKind, DialogResult};
use crate::tokens::{Color, Tokens, geom, size};
use crate::widgets::buttons::{self, Kind, Opts};
use crate::widgets::combo::Combo;
use crate::widgets::{check, galley, lerp, paint_text, section};

/// The reference's credits text.
pub const CREDITS: &str = "FTHRClips\n\nHaakon\nFounder & Head of Design\n\nTom\nCo-founder & Head of Development\n\nSpecial thanks to our helpers\n\nbenno0_ on Discord, originally suggested the upload feature\nassassin_gamer on Discord, suggested the stretch feature for the clip editor\nxc_ on Discord, suggested the import clips folder\n\nAlso thanks to\n\npetiseba\npokusan_\n\nFor being labrats :D\n\nMuch love to everyone testing our alpha :D";
const DIAGNOSTIC_HINT: &str = "After reproducing an alpha bug, export a privacy-redacted report containing bounded hardware, capture, audio, playback, export, and library diagnostics. Nothing is uploaded automatically.";
const CAPTURE_CARD_MODES: [(&str, &str); 4] = [
    ("All notifications", "all"),
    ("Clips, recordings and screenshots only", "capture"),
    ("Game detection / window switching only", "detection"),
    ("Off", "off"),
];
const NO_PRESETS: &str = "— no presets —";

/// Combo items for the export presets (`name · Built-in` first).
pub fn export_preset_items(store: &ExportPresetStore) -> Vec<(String, String)> {
    let mut all = store.all().unwrap_or_default();
    all.sort_by_key(|p| !p.built_in);
    all.iter()
        .map(|p| {
            (
                format!("{}{}", p.name, if p.built_in { " · Built-in" } else { "" }),
                p.preset_id.clone(),
            )
        })
        .collect()
}

fn fmt_g(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// `ExportPresetsWidget._sync` detail text.
pub fn export_preset_detail(p: &ExportPreset) -> String {
    let mut parts = vec![format!("{} MB target", fmt_g(p.target_size_mb))];
    if let (Some(w), Some(h)) = (p.target_width, p.target_height) {
        parts.push(format!("{w}×{h} max"));
    }
    if let Some(fps) = p.target_fps {
        parts.push(format!("{} FPS max", fmt_g(fps)));
    }
    parts.push(format!("AAC {} kbps", p.audio_bitrate_kbps));
    parts.join(" · ")
}

/// Settings preset names, or the placeholder.
pub fn settings_preset_names(state: &AppState) -> Vec<String> {
    let names = PresetsStore::new(&state.paths)
        .list_names()
        .unwrap_or_default();
    if names.is_empty() {
        vec![NO_PRESETS.to_owned()]
    } else {
        names
    }
}

/// Saves the current capture keys as a preset.
pub fn save_settings_preset(state: &mut AppState, name: &str) {
    let mut values = serde_json::Map::new();
    for key in fthrust_core::presets::PRESET_KEYS {
        if let Some(v) = state.settings.get(key) {
            values.insert(key.to_owned(), v.clone());
        }
    }
    if let Err(e) = PresetsStore::new(&state.paths).save(name, &values) {
        state
            .dialogs
            .push_message("Preset not saved", e.to_string());
    }
}

/// Loads a preset's values into the settings; restart pending.
pub fn load_settings_preset(state: &mut AppState, name: &str) -> bool {
    match PresetsStore::new(&state.paths).load(name) {
        Ok(Some(values)) => {
            for (k, v) in values {
                let key: &'static str = fthrust_core::presets::PRESET_KEYS
                    .iter()
                    .find(|p| **p == k)
                    .copied()
                    .unwrap_or("");
                if !key.is_empty() {
                    state.set_setting(key, v);
                }
            }
            state.restart_pending = true;
            true
        }
        _ => false,
    }
}

/// Deletes a preset.
pub fn delete_settings_preset(state: &mut AppState, name: &str) -> bool {
    PresetsStore::new(&state.paths)
        .delete(name)
        .unwrap_or(false)
}

/// `Support Catbox hosting`: SURFACE_2, 1 px BORDER, r6, TEXT_DIM 15 px 400
/// left-aligned with 18 px padding, 44 px tall, full width; hover
/// SURFACE_3 / BORDER_HI / TEXT.
fn support_button(ui: &mut Ui, t: &Tokens, text: &str, name: &str) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 44.0), egui::Sense::click());
    let blend = motion::hover(
        ui.ctx(),
        resp.id.with("hover"),
        resp.hovered(),
        t.reduce_motion,
    );
    let bg = lerp(t.color(Color::Surface2), t.color(Color::Surface3), blend);
    let border = lerp(t.color(Color::Border), t.color(Color::BorderHi), blend);
    let fg = lerp(t.color(Color::TextDim), t.color(Color::Text), blend);
    let radius = CornerRadius::same(geom::RADIUS as u8);
    ui.painter().rect_filled(rect, radius, bg);
    ui.painter()
        .rect_stroke(rect, radius, Stroke::new(1.0, border), StrokeKind::Inside);
    let g = galley(ui, text, size::BODY, Weight::W400, fg);
    paint_text(ui, g, rect, Align::Min, 19.0, fg);
    fidelity::mark(ui, name, rect)
        .sample(
            "fill",
            Pos2::new(rect.min.x + 5.0, rect.center().y),
            Color::Surface2,
        )
        .edge(Color::Border)
        .text(text, size::BODY, Weight::W400)
        .done();
    resp
}

fn checkbox_setting(
    ui: &mut Ui,
    state: &mut AppState,
    key: &'static str,
    default: bool,
    label: &str,
) {
    let t = state.tokens.clone();
    let mut on = state.flag(key, default);
    if check::checkbox_with(
        ui,
        &t,
        &mut on,
        label,
        true,
        true,
        &format!("QCheckBox[{}]", &label[..label.len().min(40)]),
    )
    .changed()
    {
        state.set_setting(key, json!(on));
    }
}

/// The General page body.
pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    // System.
    section::flat_header(ui, &t, "System", "System");
    space(ui, 12.0);
    // (Autostart and close-to-tray are Windows-only; their spacers remain.)
    space(ui, 8.0);
    space(ui, 6.0);
    checkbox_setting(
        ui,
        state,
        "show_kofi_widget",
        true,
        "Show Ko-fi donation widget",
    );
    space(ui, 24.0);
    // Notifications.
    section::flat_header(ui, &t, "Notifications", "Notifications");
    space(ui, 12.0);
    let before = state.flag("error_notifications_enabled", true);
    checkbox_setting(
        ui,
        state,
        "error_notifications_enabled",
        true,
        "Show capture error messages",
    );
    let after = state.flag("error_notifications_enabled", true);
    if before != after {
        state.error_bar.enabled = after;
        if !after {
            state.error_bar.clear();
        }
    }
    space(ui, 12.0);
    section::label_wide(
        ui,
        &t,
        "Capture card",
        size::BODY_L,
        Weight::W400,
        Color::Text,
        None,
    );
    let saved_mode = state
        .settings
        .get_str("capture_card_mode")
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if state.flag("capture_card_enabled", true) {
                "all".into()
            } else {
                "off".into()
            }
        });
    let sel = CAPTURE_CARD_MODES
        .iter()
        .position(|(_, m)| *m == saved_mode)
        .unwrap_or(0);
    let mut combo = Combo::new(
        CAPTURE_CARD_MODES
            .iter()
            .map(|(l, _)| l.to_string())
            .collect(),
        sel,
    );
    let name = format!("QComboBox[{}]", combo.current());
    if combo.ui(ui, &t, &mut state.icons, &name).changed() {
        state.set_setting(
            "capture_card_mode",
            json!(CAPTURE_CARD_MODES[combo.selected].1),
        );
    }
    space(ui, 24.0);
    // Upload (UploadSettingsWidget, no_scroll; its page margins add 32 px below).
    // The widget's own page has margins (0, 0, 12, 32): 12 px narrower rows.
    let upload_top = ui.cursor().min;
    let mut up = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        upload_top,
        Vec2::new(w - 12.0, f32::INFINITY),
    )));
    up.spacing_mut().item_spacing = Vec2::ZERO;
    section::flat_header(&mut up, &t, "Upload", "Upload");
    space(&mut up, 12.0);
    checkbox_setting(
        &mut up,
        state,
        "upload_enabled",
        false,
        "Enable optional clip uploader",
    );
    space(&mut up, 16.0);
    if support_button(
        &mut up,
        &t,
        "Support Catbox hosting",
        "QPushButton[Support Catbox hosting]",
    )
    .clicked()
    {
        crate::shell::open_url("https://catbox.moe/support.php");
    }
    let upload_h = up.cursor().min.y - upload_top.y + 32.0;
    ui.allocate_exact_size(Vec2::new(w, upload_h), egui::Sense::hover());
    space(ui, 24.0);
    // Export presets.
    section::flat_header(ui, &t, "Export Presets", "Export Presets");
    space(ui, 10.0);
    space(ui, 10.0);
    export_presets_widget(ui, state, w);
    space(ui, 24.0);
    // Troubleshooting.
    section::flat_header(ui, &t, "Troubleshooting", "Troubleshooting");
    space(ui, 10.0);
    section::label_wrapped_fit(
        ui,
        &t,
        DIAGNOSTIC_HINT,
        size::BODY,
        Weight::W400,
        Color::TextMuted,
        w,
    );
    space(ui, 8.0);
    if buttons::outline(
        ui,
        &t,
        "Export diagnostic report",
        "QPushButton[Export diagnostic report]",
    )
    .clicked()
    {
        state.diagnostic_export_requested = true;
    }
    if let Some(zip) = state.diagnostic_report_zip.clone() {
        space(ui, 8.0);
        let label = zip
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        if buttons::outline(ui, &t, &label, "diagnosticReportFile").clicked() {
            crate::shell::open_url(
                &zip.parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            );
        }
    }
    space(ui, 24.0);
    // Settings presets.
    section::flat_header(ui, &t, "Settings Presets", "Settings Presets");
    space(ui, 12.0);
    settings_presets_row(ui, state, w);
    space(ui, 24.0);
    // Credits.
    section::flat_header(ui, &t, "App Credits", "App Credits");
    space(ui, 10.0);
    section::label_wrapped_fit(ui, &t, CREDITS, size::BODY, Weight::W400, Color::TextDim, w);
}

fn export_presets_widget(ui: &mut Ui, state: &mut AppState, w: f32) {
    let t = state.tokens.clone();
    let store = ExportPresetStore::new(&state.paths);
    let items = export_preset_items(&store);
    let selected_id = state.export_preset_selected.clone();
    let sel = items
        .iter()
        .position(|(_, id)| *id == selected_id)
        .unwrap_or(0);
    let selected: Option<ExportPreset> = items
        .get(sel)
        .and_then(|(_, id)| store.get(id).ok().flatten());
    let built_in = selected.as_ref().map(|p| p.built_in).unwrap_or(true);
    // Row: combo (stretch) + New (primary) + Edit + Delete (outline), spacing 8.
    let labels = ["New", "Edit", "Delete"];
    let widths: Vec<f32> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let g = galley(ui, l, size::LABEL, Weight::W600, Color32::WHITE);
            (g.size().x + if i == 0 { 36.0 } else { 34.0 }).ceil()
        })
        .collect();
    let combo_w = w - widths.iter().sum::<f32>() - 8.0 * 3.0;
    let top = ui.cursor().min;
    let row_h = 41.0;
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x, top.y + 1.0),
        Vec2::new(combo_w, 38.0),
    )));
    let mut combo =
        Combo::new(items.iter().map(|(l, _)| l.clone()).collect(), sel).with_width(combo_w);
    let name = format!("WheelSafeComboBox[{}]", combo.current());
    if combo.ui(&mut c, &t, &mut state.icons, &name).changed() {
        state.export_preset_selected = items[combo.selected].1.clone();
    }
    let mut x = top.x + combo_w + 8.0;
    let mut clicked = [false; 3];
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(x, top.y), Vec2::new(widths[i], row_h));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        let kind = if i == 0 { Kind::Primary } else { Kind::Outline };
        let opts = Opts {
            enabled: i == 0 || !built_in,
            ..Opts::default()
        };
        clicked[i] = buttons::button_with(
            &mut child,
            &t,
            None,
            kind,
            label,
            &format!("QPushButton[{label}]"),
            &opts,
        )
        .clicked();
        x += widths[i] + 8.0;
    }
    ui.allocate_exact_size(Vec2::new(w, row_h), egui::Sense::hover());
    space(ui, 8.0);
    let detail = selected
        .as_ref()
        .map(export_preset_detail)
        .unwrap_or_default();
    section::label_wrapped_fit(ui, &t, &detail, size::BODY, Weight::W400, Color::TextDim, w);
    if clicked[0] {
        state
            .dialogs
            .push_modal(DialogKind::ExportPreset { preset: None });
    }
    if clicked[1]
        && let Some(p) = selected.clone()
        && !p.built_in
    {
        state
            .dialogs
            .push_modal(DialogKind::ExportPreset { preset: Some(p) });
    }
    if clicked[2]
        && let Some(p) = selected.clone()
        && !p.built_in
    {
        state.dialogs.push_modal(DialogKind::Question {
            title: "Delete export preset?".into(),
            message: format!("Delete “{}”?", p.name),
            tag: format!("export-preset-delete:{}", p.preset_id),
        });
    }
    // Dialog results.
    let results = state.dialogs.take_results();
    for r in results {
        match r {
            DialogResult::ExportPreset(Some(p)) => match store.save(&p) {
                Ok(saved) => state.export_preset_selected = saved.preset_id,
                Err(e) => state
                    .dialogs
                    .push_message("Preset not saved", e.to_string()),
            },
            DialogResult::Answer { tag, yes } if tag.starts_with("export-preset-delete:") => {
                if yes {
                    let _ = store.delete(tag.trim_start_matches("export-preset-delete:"));
                    state.export_preset_selected.clear();
                }
            }
            DialogResult::Input {
                tag,
                text: Some(name),
            } if tag == "settings-preset-save" => {
                let name = name.trim().to_owned();
                if !name.is_empty() {
                    save_settings_preset(state, &name);
                    state.settings_preset_selected = name;
                }
            }
            DialogResult::Answer { tag, yes } if tag.starts_with("settings-preset-delete:") => {
                if yes {
                    delete_settings_preset(
                        state,
                        tag.trim_start_matches("settings-preset-delete:"),
                    );
                    state.settings_preset_selected.clear();
                }
            }
            other => state.dialogs.results.push(other),
        }
    }
}

fn settings_presets_row(ui: &mut Ui, state: &mut AppState, w: f32) {
    let t = state.tokens.clone();
    let names = settings_preset_names(state);
    let sel = names
        .iter()
        .position(|n| *n == state.settings_preset_selected)
        .unwrap_or(0);
    let labels = ["Load", "Save", "Delete"];
    let widths: Vec<f32> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let g = galley(ui, l, size::LABEL, Weight::W600, Color32::WHITE);
            (g.size().x + if i == 0 { 36.0 } else { 34.0 }).ceil()
        })
        .collect();
    let combo_w = (w - widths.iter().sum::<f32>() - 8.0 * 3.0).max(160.0);
    let top = ui.cursor().min;
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x, top.y + 1.0),
        Vec2::new(combo_w, 38.0),
    )));
    let mut combo = Combo::new(names.clone(), sel).with_width(combo_w);
    let name = format!("_DropdownCombo[{}]", combo.current());
    if combo.ui(&mut c, &t, &mut state.icons, &name).changed() {
        state.settings_preset_selected = names[combo.selected].clone();
    }
    let mut x = top.x + combo_w + 8.0;
    let mut clicked = [false; 3];
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(x, top.y), Vec2::new(widths[i], 41.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        let kind = if i == 0 { Kind::Primary } else { Kind::Outline };
        // The export-presets row above already has a "Delete": Qt numbers the second.
        let marker = if i == 2 {
            "QPushButton[Delete]#2".to_owned()
        } else {
            format!("QPushButton[{label}]")
        };
        clicked[i] =
            buttons::button_with(&mut child, &t, None, kind, label, &marker, &Opts::default())
                .clicked();
        x += widths[i] + 8.0;
    }
    ui.allocate_exact_size(Vec2::new(w, 41.0), egui::Sense::hover());
    let current = names.get(sel).cloned().unwrap_or_default();
    let has_presets = current != NO_PRESETS;
    if clicked[0] && has_presets {
        load_settings_preset(state, &current);
        let summary = capture_summary(&state.settings);
        state.status_hint = Some(format!("Loaded preset {current} ({summary})"));
    }
    if clicked[1] {
        state.dialogs.push_modal(DialogKind::Input {
            title: "Save Preset".into(),
            label: "Name:".into(),
            text: String::new(),
            tag: "settings-preset-save".into(),
        });
    }
    if clicked[2] && has_presets {
        state.dialogs.push_modal(DialogKind::Question {
            title: "Delete Preset".into(),
            message: format!("Delete preset “{current}”?"),
            tag: format!("settings-preset-delete:{current}"),
        });
    }
}
