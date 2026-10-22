//! `ExportPresetDialog`: a form (labels 14 px apart, rows 8 px apart) for
//! name, target size, optional resolution and frame-rate limits, codec and
//! audio bitrate.

use egui::{Pos2, Rect, Sense, Ui, Vec2};
use fthrust_core::export_presets::ExportPreset;

use super::Outcome;
use crate::fonts::Weight;
use crate::state::{AppState, DialogResult};
use crate::tokens::{Color, size};
use crate::widgets::buttons;
use crate::widgets::check;
use crate::widgets::combo::Combo;
use crate::widgets::input::{SpinStyle, spin_box};
use crate::widgets::section;

/// The editable draft.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    /// Name.
    pub name: String,
    /// MB.
    pub size_mb: f64,
    /// Limit resolution.
    pub limit_resolution: bool,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
    /// Limit fps.
    pub limit_fps: bool,
    /// FPS.
    pub fps: f64,
    /// Audio kbps.
    pub audio_kbps: f64,
}

impl Draft {
    fn from_preset(p: Option<&ExportPreset>) -> Draft {
        Draft {
            name: p.map(|p| p.name.clone()).unwrap_or_default(),
            size_mb: p.map(|p| p.target_size_mb).unwrap_or(25.0),
            limit_resolution: p.and_then(|p| p.target_width).is_some(),
            width: p.and_then(|p| p.target_width).unwrap_or(1920) as f64,
            height: p.and_then(|p| p.target_height).unwrap_or(1080) as f64,
            limit_fps: p.and_then(|p| p.target_fps).is_some(),
            fps: p.and_then(|p| p.target_fps).unwrap_or(60.0),
            audio_kbps: p.map(|p| p.audio_bitrate_kbps).unwrap_or(128) as f64,
        }
    }

    fn to_preset(&self, original: Option<&ExportPreset>) -> ExportPreset {
        let mut p = original
            .cloned()
            .unwrap_or_else(ExportPreset::builtin_discord);
        if original.is_none() {
            p.preset_id = String::new();
            p.built_in = false;
        }
        p.name = self.name.trim().to_owned();
        p.target_size_mb = self.size_mb;
        p.target_width = self.limit_resolution.then_some(self.width as u32);
        p.target_height = self.limit_resolution.then_some(self.height as u32);
        p.target_fps = self.limit_fps.then_some(self.fps);
        p.video_codec = "h264".into();
        p.audio_bitrate_kbps = self.audio_kbps as u32;
        p
    }
}

const LABEL_W: f32 = 78.0; // widest label: "Target file size"

fn row(
    ui: &mut Ui,
    state: &mut AppState,
    label: &str,
    h: f32,
    control: impl FnOnce(&mut Ui, &mut AppState),
) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    let top = ui.cursor().min;
    if !label.is_empty() {
        let lw = crate::widgets::galley(ui, label, size::BODY, Weight::W400, egui::Color32::WHITE)
            .size()
            .x
            .ceil();
        section::label_in(
            ui,
            &t,
            Rect::from_min_size(top, Vec2::new(lw, h)),
            label,
            size::BODY,
            Weight::W400,
            Color::Text,
            None,
        );
    }
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + LABEL_W + 14.0, top.y),
        Vec2::new(w - LABEL_W - 14.0, h),
    )));
    control(&mut c, state);
    ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
}

/// The form body.
pub fn body(ui: &mut Ui, state: &mut AppState, preset: Option<&ExportPreset>) {
    let t = state.tokens.clone();
    let mut d = state
        .export_preset_draft
        .clone()
        .unwrap_or_else(|| Draft::from_preset(preset));
    ui.spacing_mut().item_spacing = Vec2::new(0.0, 8.0);
    row(ui, state, "Name", 40.0, |ui, _| {
        let w = ui.available_width();
        let mut c = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(ui.max_rect().min, Vec2::new(w, 40.0))),
        );
        crate::widgets::dialog::input_field(&mut c, &t, &mut d.name, w, "presetName");
    });
    row(ui, state, "Target file size", 34.0, |ui, state| {
        spin_box(
            ui,
            &t,
            &mut state.icons,
            &mut d.size_mb,
            1.0..=10_000.0,
            1.0,
            1,
            " MB",
            SpinStyle::Preset,
            true,
            "presetStepper",
        );
    });
    row(ui, state, "", 26.0, |ui, _| {
        check::checkbox_with(
            ui,
            &t,
            &mut d.limit_resolution,
            "Limit resolution",
            true,
            true,
            "QCheckBox[Limit resolution]",
        );
    });
    let enabled_res = d.limit_resolution;
    row(ui, state, "Resolution", 34.0, |ui, state| {
        let w = ui.available_width();
        let spin_w = ((w - 60.0) / 2.0).floor();
        let top = ui.max_rect().min;
        let mut a = ui.new_child(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(top, Vec2::new(spin_w, 34.0))),
        );
        spin_box(
            &mut a,
            &t,
            &mut state.icons,
            &mut d.width,
            16.0..=8192.0,
            2.0,
            0,
            "",
            SpinStyle::Preset,
            enabled_res,
            "presetStepper#2",
        );
        let x_rect = Rect::from_min_size(Pos2::new(top.x + spin_w, top.y), Vec2::new(60.0, 34.0));
        let g = crate::widgets::galley(ui, "×", size::BODY, Weight::W400, t.color(Color::Text));
        crate::widgets::paint_text(
            ui,
            g,
            x_rect,
            egui::Align::Center,
            0.0,
            t.color(Color::Text),
        );
        crate::fidelity::mark(ui, "QLabel[×]", x_rect)
            .text("×", size::BODY, Weight::W400)
            .done();
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + spin_w + 60.0, top.y),
            Vec2::new(w - spin_w - 60.0, 34.0),
        )));
        spin_box(
            &mut b,
            &t,
            &mut state.icons,
            &mut d.height,
            16.0..=8192.0,
            2.0,
            0,
            "",
            SpinStyle::Preset,
            enabled_res,
            "presetStepper#3",
        );
    });
    row(ui, state, "", 26.0, |ui, _| {
        check::checkbox_with(
            ui,
            &t,
            &mut d.limit_fps,
            "Limit frame rate",
            true,
            true,
            "QCheckBox[Limit frame rate]",
        );
    });
    let enabled_fps = d.limit_fps;
    row(ui, state, "Frame rate", 34.0, |ui, state| {
        spin_box(
            ui,
            &t,
            &mut state.icons,
            &mut d.fps,
            1.0..=240.0,
            1.0,
            2,
            " FPS",
            SpinStyle::Preset,
            enabled_fps,
            "presetStepper#4",
        );
    });
    row(ui, state, "Video codec", 38.0, |ui, state| {
        let mut combo = Combo::new(vec!["H.264 (reviewed runtime)".into()], 0);
        combo.ui(
            ui,
            &t,
            &mut state.icons,
            "WheelSafeComboBox[H.264 (reviewed runtime)]",
        );
    });
    row(ui, state, "Audio", 34.0, |ui, state| {
        spin_box(
            ui,
            &t,
            &mut state.icons,
            &mut d.audio_kbps,
            32.0..=512.0,
            1.0,
            0,
            " kbps AAC",
            SpinStyle::Preset,
            true,
            "presetStepper#5",
        );
    });
    state.export_preset_draft = Some(d);
}

/// Actions: Cancel / Save preset (validated; a warning message on failure).
pub fn actions(
    ui: &mut Ui,
    state: &mut AppState,
    preset: Option<&ExportPreset>,
    enter: bool,
) -> Outcome {
    let t = state.tokens.clone();
    let save = buttons::primary(ui, &t, "Save preset", "QPushButton[Save preset]").clicked();
    let cancel = buttons::outline(ui, &t, "Cancel", "QPushButton[Cancel]").clicked();
    if save || enter {
        let draft = state
            .export_preset_draft
            .clone()
            .unwrap_or_else(|| Draft::from_preset(preset));
        let p = draft.to_preset(preset);
        return match p.validate() {
            Ok(()) => Outcome::Done(Some(DialogResult::ExportPreset(Some(p)))),
            Err(e) => {
                state.dialogs.push_message("Preset is not valid", e);
                Outcome::Open
            }
        };
    }
    if cancel {
        return Outcome::Done(Some(DialogResult::ExportPreset(None)));
    }
    Outcome::Open
}
