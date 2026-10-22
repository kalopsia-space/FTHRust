//! The Clip tab (`_make_clip_page`): folders, imported clip folders, video
//! encoding, watermark and clip editor options.

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2};
use serde_json::{Value, json};

use super::space;
use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::Icon;
use crate::state::{AppState, BrowseKind};
use crate::tokens::{Color, Tokens, size};
use crate::widgets::buttons::{self, IconButtonStyle, Kind, Opts, icon_button};
use crate::widgets::combo::{self, Combo};
use crate::widgets::{check, galley, paint_text, section};

const NVENC_PRESETS: [(u8, &str); 7] = [
    (1, "P1 — Fastest"),
    (2, "P2"),
    (3, "P3"),
    (4, "P4 — Balanced"),
    (5, "P5"),
    (6, "P6"),
    (7, "P7 — Best Quality"),
];

/// Read-only folder field: SURFACE_1, TEXT_DIM, 1 px BORDER (square),
/// padding 7×9, 40 px tall.
fn folder_field(ui: &mut Ui, t: &Tokens, rect: Rect, text: &str) {
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, t.color(Color::Surface1));
    ui.painter().rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, t.color(Color::Border)),
        StrokeKind::Inside,
    );
    let g = galley(ui, text, size::BODY, Weight::W400, t.color(Color::TextDim));
    let clip = ui.clip_rect();
    ui.set_clip_rect(rect.intersect(clip));
    paint_text(ui, g, rect, Align::Min, 10.0, t.color(Color::TextDim));
    ui.set_clip_rect(clip);
    fidelity::mark(
        ui,
        format!("QLineEdit[{}]", crate::fidelity::marker_label(text)),
        rect,
    )
    .sample(
        "fill",
        Pos2::new(rect.min.x + 4.0, rect.center().y),
        Color::Surface1,
    )
    .edge(Color::Border)
    .text(text, size::BODY, Weight::W400)
    .done();
}

fn folder_row(
    ui: &mut Ui,
    state: &mut AppState,
    label: &str,
    path: &str,
    with_default: bool,
    browse_suffix: &str,
) -> (bool, bool) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    let top = ui.cursor().min;
    let row_h = 41.0;
    section::label_in(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(180.0, row_h)),
        label,
        size::MICRO,
        Weight::W600,
        Color::Text,
        None,
    );
    let browse_w = (galley(ui, "BROWSE", size::LABEL, Weight::W600, Color32::WHITE)
        .size()
        .x
        + 34.0)
        .ceil();
    let default_w = if with_default {
        (galley(ui, "USE DEFAULT", size::LABEL, Weight::W600, Color32::WHITE)
            .size()
            .x
            + 34.0)
            .ceil()
            + 8.0
    } else {
        0.0
    };
    let field_w = w - 180.0 - 8.0 - browse_w - 8.0 - default_w;
    folder_field(
        ui,
        &t,
        Rect::from_min_size(Pos2::new(top.x + 188.0, top.y), Vec2::new(field_w, 40.0)),
        path,
    );
    let bx = top.x + 188.0 + field_w + 8.0;
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(bx, top.y),
        Vec2::new(browse_w, row_h),
    )));
    let browse = buttons::button_with(
        &mut b,
        &t,
        None,
        Kind::Outline,
        "BROWSE",
        &format!("QPushButton[BROWSE]{browse_suffix}"),
        &Opts::default(),
    )
    .clicked();
    let mut default = false;
    if with_default {
        let mut d = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(bx + browse_w + 8.0, top.y),
            Vec2::new(default_w - 8.0, row_h),
        )));
        default = buttons::outline(&mut d, &t, "USE DEFAULT", "QPushButton[USE DEFAULT]").clicked();
    }
    ui.allocate_exact_size(Vec2::new(w, row_h), egui::Sense::hover());
    (browse, default)
}

fn encoding_row(
    ui: &mut Ui,
    state: &mut AppState,
    label: &str,
    control_h: f32,
    control: impl FnOnce(&mut Ui, &mut AppState),
) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    let top = ui.cursor().min;
    section::label_in(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(140.0, control_h)),
        label,
        size::MICRO,
        Weight::W600,
        Color::Text,
        None,
    );
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + 152.0, top.y),
        Vec2::new(w - 152.0, control_h),
    )));
    control(&mut c, state);
    ui.allocate_exact_size(Vec2::new(w, control_h), egui::Sense::hover());
}

/// Encoder combo items: `Automatic (recommended)` + capability labels, or
/// the detecting placeholder.
pub fn encoder_items(state: &AppState) -> (Vec<(String, String)>, bool) {
    match &state.encoder_capabilities {
        None => (
            vec![("Detecting available encoders…".into(), "auto".into())],
            false,
        ),
        Some(caps) => {
            let mut items = vec![("Automatic (recommended)".to_owned(), "auto".to_owned())];
            items.extend(caps.iter().map(|c| (c.label.clone(), c.key.clone())));
            (items, true)
        }
    }
}

/// The detection label text (`_on_encoder_capabilities_ready`).
pub fn detection_text(state: &AppState) -> String {
    match &state.encoder_capabilities {
        None => "Checking this device…".into(),
        Some(caps) if caps.is_empty() => {
            "No usable encoder was detected; Automatic remains available.".into()
        }
        Some(caps) => {
            let prefix = if state.encoder_from_cache {
                "Saved device profile · "
            } else {
                ""
            };
            format!(
                "{prefix}Available: {}",
                caps.iter()
                    .map(|c| c.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

/// The Clip page body.
pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    section::flat_header(ui, &t, "Clip Settings", "Clip Settings");
    space(ui, 12.0);
    section::flat_header(ui, &t, "Folders", "Folders");
    space(ui, 12.0);
    let clips_dir = state
        .settings
        .clips_directory()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| {
            state
                .settings
                .get_str("clips_directory")
                .unwrap_or("")
                .to_owned()
        });
    let (browse, default) = folder_row(ui, state, "CLIPS STORAGE FOLDER", &clips_dir, true, "");
    if browse {
        state.browse_requested = Some(BrowseKind::ClipsFolder);
    }
    if default {
        state.settings.remove("clips_directory");
        let _ = state.settings.save();
        state.clips_directory_changed = true;
    }
    space(ui, 8.0);
    let rec_dir = state
        .settings
        .recording_directory()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| {
            state
                .settings
                .get_str("recording_directory")
                .unwrap_or("")
                .to_owned()
        });
    let (browse, _) = folder_row(ui, state, "MANUAL RECORDING FOLDER", &rec_dir, false, "#2");
    if browse {
        state.browse_requested = Some(BrowseKind::RecordingFolder);
    }
    space(ui, 24.0);
    // Import clips.
    section::flat_header(ui, &t, "Import Clips", "Import Clips");
    space(ui, 10.0);
    let top = ui.cursor().min;
    let add_w = (galley(ui, "Add folder", size::LABEL, Weight::W600, Color32::WHITE)
        .size()
        .x
        + 34.0)
        .ceil();
    let scan_w = (galley(
        ui,
        "Scan for clips",
        size::LABEL,
        Weight::W600,
        Color32::WHITE,
    )
    .size()
    .x + 38.0)
        .ceil();
    let mut a = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x, top.y + 1.0),
        Vec2::new(add_w, 41.0),
    )));
    if buttons::outline(&mut a, &t, "Add folder", "QPushButton[Add folder]").clicked() {
        state.browse_requested = Some(BrowseKind::ImportFolder);
    }
    let mut sc = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + add_w + 8.0, top.y),
        Vec2::new(scan_w, 43.0),
    )));
    if buttons::secondary(&mut sc, &t, "Scan for clips", "QPushButton[Scan for clips]").clicked() {
        state.scan_clips_requested = true;
    }
    ui.allocate_exact_size(Vec2::new(w, 43.0), egui::Sense::hover());
    space(ui, 12.0);
    let folders: Vec<String> = state
        .settings
        .get("imported_clip_folders")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if folders.is_empty() {
        section::label_wide(
            ui,
            &t,
            "No import folders added yet.",
            size::BODY,
            Weight::W400,
            Color::TextMuted,
            None,
        );
    } else {
        let mut remove: Option<usize> = None;
        for (i, folder) in folders.iter().enumerate() {
            let row_top = ui.cursor().min;
            let h = 36.0;
            let row = Rect::from_min_size(row_top, Vec2::new(w, h));
            ui.painter()
                .rect_filled(row, CornerRadius::ZERO, t.color(Color::Surface1));
            ui.painter().rect_stroke(
                row,
                CornerRadius::ZERO,
                Stroke::new(1.0, t.color(Color::Border)),
                StrokeKind::Inside,
            );
            let mut display = folder.clone();
            if !std::path::Path::new(folder).is_dir() {
                display.push_str("  —  MISSING (remove link or reconnect drive)");
            }
            section::label_in(
                ui,
                &t,
                Rect::from_min_max(
                    row.min + Vec2::new(11.0, 7.0),
                    Pos2::new(row.max.x - 40.0, row.max.y - 7.0),
                ),
                &display,
                size::BODY,
                Weight::W400,
                Color::TextDim,
                Some(&format!("importFolder#{}", i + 1)),
            );
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
                Pos2::new(row.max.x - 7.0 - 22.0, row.min.y + 7.0),
                Vec2::splat(22.0),
            )));
            let style = IconButtonStyle {
                size: Vec2::splat(22.0),
                bg: None,
                hover_bg: None,
                pressed_bg: None,
                border: Some(Color::Border),
                hover_border: Some(Color::Error),
                radius: 0.0,
                hover_tint: Some(Color::Error),
                enabled: true,
            };
            if icon_button(
                &mut c,
                &t,
                &mut state.icons,
                Icon::Close,
                14.0,
                style,
                &format!("importFolderRemove#{}", i + 1),
            )
            .clicked()
            {
                remove = Some(i);
            }
            ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::hover());
            if i + 1 < folders.len() {
                space(ui, 4.0);
            }
        }
        if let Some(i) = remove {
            let mut kept = folders.clone();
            kept.remove(i);
            state.set_setting("imported_clip_folders", json!(kept));
            state.imported_folders_changed = true;
        }
    }
    space(ui, 4.0);
    space(ui, 24.0);
    section::hsep(ui, &t, "clipHsep");
    space(ui, 18.0);
    // Video encoding.
    section::flat_header(ui, &t, "Video Encoding", "Video Encoding");
    space(ui, 12.0);
    let (items, ready) = encoder_items(state);
    let saved_encoder = state
        .settings
        .get_str("encoder_pref")
        .unwrap_or("auto")
        .to_owned();
    let sel = items
        .iter()
        .position(|(_, k)| *k == saved_encoder)
        .unwrap_or(0);
    let mut picked_encoder: Option<String> = None;
    encoding_row(ui, state, "Encoder", combo::HEIGHT, |ui, state| {
        let t = state.tokens.clone();
        let mut c = Combo::new(items.iter().map(|(l, _)| l.clone()).collect(), sel);
        c.enabled = ready;
        let name = format!("_DropdownCombo[{}]", c.current());
        if c.ui(ui, &t, &mut state.icons, &name).changed() {
            picked_encoder = Some(items[c.selected].1.clone());
        }
    });
    if let Some(k) = picked_encoder {
        state.set_setting("encoder_pref", json!(k));
        state.encoder_dirty = true;
    }
    space(ui, 8.0);
    let encoder_key = state
        .settings
        .get_str("encoder_pref")
        .unwrap_or("auto")
        .to_owned();
    let codecs: Vec<(String, String)> = match &state.encoder_capabilities {
        Some(caps) if ready => {
            let mut v = vec![("Automatic".to_owned(), "auto".to_owned())];
            let names: Vec<String> =
                fthrust_media::capabilities::available_codecs(caps, &encoder_key);
            for codec in names {
                let label = caps
                    .iter()
                    .flat_map(|c| c.codec_names.iter())
                    .find(|(k, _)| *k == codec)
                    .map(|(_, l)| l.clone())
                    .unwrap_or_else(|| codec.clone());
                v.push((label, codec));
            }
            v
        }
        _ => vec![("Automatic".to_owned(), "auto".to_owned())],
    };
    let saved_codec = state
        .settings
        .get_str("codec_pref")
        .unwrap_or("auto")
        .to_owned();
    let csel = codecs
        .iter()
        .position(|(_, k)| *k == saved_codec)
        .unwrap_or(0);
    let mut picked_codec: Option<String> = None;
    encoding_row(ui, state, "Codec", combo::HEIGHT, |ui, state| {
        let t = state.tokens.clone();
        let mut c = Combo::new(codecs.iter().map(|(l, _)| l.clone()).collect(), csel);
        c.enabled = ready && codecs.len() > 1;
        let name = format!("_DropdownCombo[{}]", c.current());
        if c.ui(ui, &t, &mut state.icons, &name).changed() {
            picked_codec = Some(codecs[c.selected].1.clone());
        }
    });
    if let Some(k) = picked_codec {
        state.set_setting("codec_pref", json!(k));
        state.encoder_dirty = true;
    }
    space(ui, 8.0);
    let supports_presets = state
        .encoder_capabilities
        .as_ref()
        .and_then(|caps| caps.iter().find(|c| c.key == encoder_key))
        .map(|c| c.supports_presets)
        .unwrap_or(false);
    if supports_presets {
        let saved = state
            .settings
            .get_i64("encoder_preset")
            .unwrap_or(4)
            .clamp(1, 7) as u8;
        let psel = NVENC_PRESETS
            .iter()
            .position(|(v, _)| *v == saved)
            .unwrap_or(3);
        let mut picked: Option<u8> = None;
        encoding_row(ui, state, "NVIDIA preset", combo::HEIGHT, |ui, state| {
            let t = state.tokens.clone();
            let mut c = Combo::new(
                NVENC_PRESETS.iter().map(|(_, l)| l.to_string()).collect(),
                psel,
            );
            let name = format!("_DropdownCombo[{}]", c.current());
            if c.ui(ui, &t, &mut state.icons, &name).changed() {
                picked = Some(NVENC_PRESETS[c.selected].0);
            }
        });
        if let Some(v) = picked {
            state.set_setting("encoder_preset", json!(v));
            state.encoder_dirty = true;
        }
    }
    // Qt keeps the preset row's 8 px spacer even while the row is hidden.
    space(ui, 8.0);
    let active = state.active_encoder.clone().unwrap_or_else(|| "—".into());
    let lh = crate::fonts::qt_wrapped_line_height(size::BODY, Weight::W400);
    encoding_row(ui, state, "Active encoder", lh, |ui, state| {
        let t = state.tokens.clone();
        section::label_in(
            ui,
            &t,
            Rect::from_min_size(ui.max_rect().min, Vec2::new(ui.available_width(), lh)),
            &active,
            size::BODY,
            Weight::W400,
            Color::Accent,
            None,
        );
    });
    space(ui, 10.0);
    // Actions row: [Apply] Redetect, detection text (stretch), spacing 12.
    let top = ui.cursor().min;
    let mut x = top.x;
    if state.encoder_dirty {
        let aw = (galley(ui, "Apply", size::LABEL, Weight::W600, Color32::WHITE)
            .size()
            .x
            + 36.0)
            .ceil();
        let mut a = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(x, top.y),
            Vec2::new(aw, 41.0),
        )));
        if buttons::primary(&mut a, &t, "Apply", "QPushButton[Apply]").clicked() {
            state.encoder_dirty = false;
            state.engine_restart_requested = true;
        }
        x += aw + 12.0;
    }
    let rw = (galley(ui, "Redetect", size::LABEL, Weight::W600, Color32::WHITE)
        .size()
        .x
        + 34.0)
        .ceil();
    let mut r = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(x, top.y),
        Vec2::new(rw, 41.0),
    )));
    if buttons::outline(&mut r, &t, "Redetect", "QPushButton[Redetect]").clicked() {
        state.redetect_encoders_requested = true;
        state.encoder_capabilities = None;
    }
    x += rw + 12.0;
    let text = detection_text(state);
    section::label_in(
        ui,
        &t,
        Rect::from_min_size(Pos2::new(x, top.y), Vec2::new(w - (x - top.x), 41.0)),
        &text,
        size::BODY,
        Weight::W400,
        Color::TextDim,
        None,
    );
    ui.allocate_exact_size(Vec2::new(w, 41.0), egui::Sense::hover());
    space(ui, 24.0);
    // Watermark.
    section::flat_header(ui, &t, "Watermark", "Watermark");
    space(ui, 12.0);
    let mut on = state.flag("watermark_enabled", false);
    if check::checkbox_with(
        ui,
        &t,
        &mut on,
        "Add animated Capture Card to exports and shares",
        true,
        true,
        "QCheckBox[Add animated Capture Card to exports",
    )
    .changed()
    {
        state.set_setting("watermark_enabled", json!(on));
    }
    space(ui, 10.0);
    space(ui, 18.0);
    section::flat_header(ui, &t, "Clip Editor", "Clip Editor");
    space(ui, 12.0);
    for (key, text, default) in [
        ("timeline_preview_strip_enabled", "Seek bar preview", true),
        (
            "hide_deleted_timeline_segments",
            "Hide deleted segments in the editor seek bar",
            false,
        ),
        (
            "grid_show_edited_duration",
            "Show clip length excluding deleted segments in the grid",
            false,
        ),
    ] {
        let mut on = state.flag(key, default);
        if check::checkbox_with(
            ui,
            &t,
            &mut on,
            text,
            true,
            true,
            &format!("QCheckBox[{}]", crate::fidelity::marker_label(text)),
        )
        .changed()
        {
            state.set_setting(key, json!(on));
        }
        space(ui, 10.0);
    }
}
