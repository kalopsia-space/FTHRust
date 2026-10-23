//! The Visuals tab (`_make_visuals_page_unified_legacy`): overlay options
//! (380 px) and the composite preview stage.

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use serde_json::{Value, json};

use super::space;
use crate::fidelity;
use crate::fonts::Weight;
use crate::state::{AppState, BrowseKind};
use crate::tokens::{Color, Tokens, geom, size};
use crate::widgets::buttons::{self, Kind, Opts};
use crate::widgets::combo::Combo;
use crate::widgets::{check, galley, paint_text, section, slider};

/// Source label when no custom background is set.
pub const DEFAULT_PREVIEW_SOURCE: &str = "DEFAULT · DESKTOP";
const OPTIONS_W: f32 = 380.0;
const KEYBOARD_HINT: &str = "Capture a Noboard/NohBoard-style window, then click its solid background above. Changes are applied to the live preview immediately.";

fn option_header(ui: &mut Ui, t: &Tokens, text: &str) {
    section::label_wide(ui, t, text, size::MICRO, Weight::W600, Color::Accent, None);
    space(ui, 8.0);
}

fn small_label(ui: &mut Ui, t: &Tokens, rect: Rect, text: &str, color: Color) {
    section::label_in(ui, t, rect, text, size::MICRO, Weight::W600, color, None);
}

/// A fraction rect `{x, y, w, h}` from settings.
fn frac_rect(v: Option<&Value>, default: crate::state::FracRect) -> crate::state::FracRect {
    let g = |k: &str, d: f64| {
        v.and_then(|r| r.get(k))
            .and_then(Value::as_f64)
            .unwrap_or(d)
    };
    let (w, h) = (
        g("w", default.2).clamp(0.05, 1.0),
        g("h", default.3).clamp(0.05, 1.0),
    );
    (
        g("x", default.0).clamp(0.0, 1.0 - w),
        g("y", default.1).clamp(0.0, 1.0 - h),
        w,
        h,
    )
}

/// The Visuals page body.
pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    section::flat_header(ui, &t, "Visual Overlays", "Visual Overlays");
    space(ui, geom::ROW_GAP);
    let top = ui.cursor().min;
    // Options column.
    let mut opt = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        top,
        Vec2::new(OPTIONS_W - geom::ROW_GAP, f32::INFINITY),
    )));
    opt.spacing_mut().item_spacing = Vec2::ZERO;
    options_column(&mut opt, state);
    let options_h = opt.cursor().min.y - top.y;
    let preview_hint_h = 21.0 + 8.0 + 500.0;
    let columns_h = options_h.max(preview_hint_h);
    fidelity::mark(
        ui,
        "overlayOptionsPanel",
        Rect::from_min_size(top, Vec2::new(OPTIONS_W, columns_h)),
    )
    .done();
    // Preview column.
    let px = top.x + OPTIONS_W + 16.0;
    let panel = Rect::from_min_size(
        Pos2::new(px, top.y),
        Vec2::new(w - OPTIONS_W - 16.0, columns_h),
    );
    fidelity::mark(ui, "overlayPreviewPanel", panel).done();
    let inner_x = px + geom::ROW_GAP;
    let inner_w = panel.width() - geom::ROW_GAP;
    let title_h = crate::fonts::qt_height(size::LABEL, Weight::W600);
    let g = galley(
        ui,
        "Composite preview",
        size::LABEL,
        Weight::W600,
        t.color(Color::Text),
    );
    let trect = Rect::from_min_size(
        Pos2::new(inner_x, top.y),
        Vec2::new(g.size().x.round() + 1.0, title_h),
    );
    paint_text(ui, g, trect, Align::Min, 0.0, t.color(Color::Text));
    fidelity::mark(ui, "QLabel[Composite preview]", trect)
        .text("Composite preview", size::LABEL, Weight::W600)
        .done();
    let bg_path = state
        .settings
        .get_str("input_overlay_preview_background")
        .unwrap_or("")
        .to_owned();
    let source = if bg_path.is_empty() {
        DEFAULT_PREVIEW_SOURCE.to_owned()
    } else {
        std::path::Path::new(&bg_path)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let sg = galley(
        ui,
        &source,
        size::MICRO,
        Weight::W600,
        t.color(Color::TextDim),
    );
    let sw = sg.size().x.round();
    let srect = Rect::from_min_size(Pos2::new(panel.max.x - sw, top.y), Vec2::new(sw, title_h));
    paint_text(ui, sg, srect, Align::Min, 0.0, t.color(Color::TextDim));
    fidelity::mark(ui, format!("QLabel[{source}]"), srect)
        .text(&source, size::MICRO, Weight::W600)
        .done();
    let stage = Rect::from_min_size(
        Pos2::new(inner_x, top.y + title_h + 8.0),
        Vec2::new(inner_w, columns_h - title_h - 8.0),
    );
    overlay_stage(ui, state, stage, &bg_path);
    ui.allocate_exact_size(Vec2::new(w, columns_h), Sense::hover());
}

fn options_column(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    // Camera.
    option_header(ui, &t, "Camera");
    let mut cam = state.flag("camera_enabled", false);
    if check::checkbox_with(
        ui,
        &t,
        &mut cam,
        "Burn camera overlay into clips",
        true,
        true,
        "QCheckBox[Burn camera overlay into clips]",
    )
    .changed()
    {
        state.set_setting("camera_enabled", json!(cam));
    }
    space(ui, 8.0);
    let top = ui.cursor().min;
    small_label(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(58.0, 36.0)),
        "Device",
        Color::Text,
    );
    let combo_w = w - 58.0 - 8.0 - 8.0 - 26.0;
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + 66.0, top.y),
        Vec2::new(combo_w, 36.0),
    )));
    let items: Vec<String> = if state.camera_devices.is_empty() {
        vec!["Scanning cameras…".into()]
    } else {
        state.camera_devices.clone()
    };
    let saved = state
        .settings
        .get_i64("camera_device_index")
        .unwrap_or(0)
        .max(0) as usize;
    let mut combo = Combo::new(items.clone(), saved.min(items.len() - 1))
        .with_width(combo_w)
        .with_height(36.0);
    combo.enabled = !state.camera_devices.is_empty();
    let name = format!("_DropdownCombo[{}]", combo.current());
    if combo.ui(&mut c, &t, &mut state.icons, &name).changed() {
        state.set_setting("camera_device_index", json!(combo.selected));
    }
    if super::audio::mic_refresh_button(
        ui,
        &t,
        state,
        Rect::from_min_size(Pos2::new(top.x + w - 26.0, top.y + 5.0), Vec2::splat(26.0)),
        "micRefreshBtn",
    ) {
        state.refresh_cameras_requested = true;
    }
    ui.allocate_exact_size(Vec2::new(w, 36.0), Sense::hover());
    space(ui, 18.0);
    // Image layers.
    option_header(ui, &t, "Image layers");
    let layers: Vec<Value> = state
        .settings
        .get("image_overlays")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut burn = !layers.is_empty() && state.flag("image_overlay_enabled", false);
    if check::checkbox_with(
        ui,
        &t,
        &mut burn,
        "Burn image layers into clips",
        true,
        true,
        "QCheckBox[Burn image layers into clips]",
    )
    .changed()
    {
        state.set_setting("image_overlay_enabled", json!(burn));
    }
    space(ui, 8.0);
    let top = ui.cursor().min;
    let sel_w = w - 8.0 - 92.0;
    let mut c = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(top, Vec2::new(sel_w, 38.0))),
    );
    let names: Vec<String> = if layers.is_empty() {
        vec!["No image layers".into()]
    } else {
        layers
            .iter()
            .map(|l| {
                l.get("path")
                    .and_then(Value::as_str)
                    .and_then(|p| {
                        std::path::Path::new(p)
                            .file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| "Image".into())
            })
            .collect()
    };
    let mut combo = Combo::new(
        names,
        state
            .selected_image_layer
            .min(layers.len().saturating_sub(1)),
    )
    .with_width(sel_w);
    combo.enabled = !layers.is_empty();
    let name = format!("_DropdownCombo[{}]", combo.current());
    if combo.ui(&mut c, &t, &mut state.icons, &name).changed() {
        state.selected_image_layer = combo.selected;
    }
    let count = format!(
        "{} IMAGE LAYER{}",
        layers.len(),
        if layers.len() != 1 { "S" } else { "" }
    );
    let crect = Rect::from_min_size(Pos2::new(top.x + w - 92.0, top.y), Vec2::new(92.0, 38.0));
    let g = galley(
        ui,
        &count,
        size::MICRO,
        Weight::W600,
        t.color(Color::TextMuted),
    );
    paint_text(ui, g, crect, Align::Max, 0.0, t.color(Color::TextMuted));
    fidelity::mark(ui, format!("QLabel[{count}]"), crect)
        .text(&count, size::MICRO, Weight::W600)
        .done();
    ui.allocate_exact_size(Vec2::new(w, 38.0), Sense::hover());
    space(ui, 8.0);
    // Add images (primary, stretch) / Remove (outline) / Clear all (secondary, 43 tall).
    let top = ui.cursor().min;
    let remove_w = (galley(ui, "Remove", size::LABEL, Weight::W600, Color32::WHITE)
        .size()
        .x
        + 34.0)
        .ceil();
    let clear_w = (galley(ui, "Clear all", size::LABEL, Weight::W600, Color32::WHITE)
        .size()
        .x
        + 38.0)
        .ceil();
    let add_w = w - 8.0 - remove_w - 8.0 - clear_w;
    let mut a = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x, top.y + 1.0),
        Vec2::new(add_w, 41.0),
    )));
    if buttons::button_with(
        &mut a,
        &t,
        None,
        Kind::Primary,
        "Add images",
        "QPushButton[Add images]",
        &Opts {
            min_width: add_w,
            ..Opts::default()
        },
    )
    .clicked()
    {
        state.browse_requested = Some(BrowseKind::ImageLayers);
    }
    let mut r = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + add_w + 8.0, top.y + 1.0),
        Vec2::new(remove_w, 41.0),
    )));
    let has = !layers.is_empty();
    if buttons::button_with(
        &mut r,
        &t,
        None,
        Kind::Outline,
        "Remove",
        "QPushButton[Remove]",
        &Opts {
            enabled: has,
            min_width: remove_w,
            ..Opts::default()
        },
    )
    .clicked()
    {
        let mut kept = layers.clone();
        if state.selected_image_layer < kept.len() {
            kept.remove(state.selected_image_layer);
        }
        state.set_setting("image_overlays", Value::Array(kept));
    }
    let mut cl = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + w - clear_w, top.y),
        Vec2::new(clear_w, 43.0),
    )));
    if buttons::button_with(
        &mut cl,
        &t,
        None,
        Kind::Secondary,
        "Clear all",
        "QPushButton[Clear all]",
        &Opts {
            enabled: has,
            min_width: clear_w,
            ..Opts::default()
        },
    )
    .clicked()
    {
        state.set_setting("image_overlays", json!([]));
    }
    ui.allocate_exact_size(Vec2::new(w, 43.0), Sense::hover());
    space(ui, 12.0);
    // Fit / Opacity row.
    let top = ui.cursor().min;
    small_label(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(30.0, 38.0)),
        "Fit",
        Color::Text,
    );
    let fit_hint = galley(ui, "Inside frame", size::BODY, Weight::W400, Color32::WHITE)
        .size()
        .x
        .ceil()
        + 24.0
        + 22.0
        + 2.0;
    let shared = w - 30.0 - 8.0 - 8.0 - 52.0 - 8.0 - 8.0 - 34.0;
    let widths = crate::shell::toolbar::stretch_widths(shared, &[1, 1], &[fit_hint, 84.0]);
    let fit = state
        .settings
        .get_str("image_overlay_fit")
        .unwrap_or("fit")
        .to_owned();
    let mut fc = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + 38.0, top.y),
        Vec2::new(widths[0], 38.0),
    )));
    let mut combo = Combo::new(
        vec!["Inside frame".into(), "Fill frame".into()],
        if fit == "fill" { 1 } else { 0 },
    )
    .with_width(widths[0]);
    combo.enabled = has;
    let name = format!("_DropdownCombo[{}]", combo.current());
    if combo.ui(&mut fc, &t, &mut state.icons, &name).changed() {
        state.set_setting(
            "image_overlay_fit",
            json!(if combo.selected == 1 { "fill" } else { "fit" }),
        );
    }
    let ox = top.x + 38.0 + widths[0] + 8.0;
    small_label(
        ui,
        &t,
        Rect::from_min_size(Pos2::new(ox, top.y), Vec2::new(52.0, 38.0)),
        "Opacity",
        Color::Text,
    );
    let mut opacity = state
        .settings
        .get_i64("image_overlay_opacity")
        .unwrap_or(100) as f32;
    let mut sl = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(ox + 60.0, top.y + 6.0),
        Vec2::new(widths[1], 26.0),
    )));
    if slider::settings_slider(
        &mut sl,
        &t,
        &mut opacity,
        10.0..=100.0,
        "imageOpacitySlider",
    )
    .changed()
    {
        state.set_setting("image_overlay_opacity", json!(opacity as i64));
    }
    let vrect = Rect::from_min_size(Pos2::new(top.x + w - 34.0, top.y), Vec2::new(34.0, 38.0));
    let text = format!("{}%", opacity as i64);
    let g = galley(ui, &text, size::BODY, Weight::W400, t.color(Color::TextDim));
    paint_text(ui, g, vrect, Align::Min, 0.0, t.color(Color::TextDim));
    fidelity::mark(ui, format!("QLabel[{text}]"), vrect)
        .text(&text, size::BODY, Weight::W400)
        .done();
    ui.allocate_exact_size(Vec2::new(w, 38.0), Sense::hover());
    space(ui, 20.0);
    // Third-party keyboard (Windows capture source; shown disabled on Linux).
    option_header(ui, &t, "Third-party keyboard");
    let kb = state
        .settings
        .get("third_party_keyboard")
        .cloned()
        .unwrap_or(json!({}));
    let mut kb_on = kb.get("enabled").and_then(Value::as_bool).unwrap_or(false)
        && kb.get("hwnd").and_then(Value::as_i64).unwrap_or(0) != 0;
    check::checkbox_with(
        ui,
        &t,
        &mut kb_on,
        "Show external keyboard in clips",
        false,
        true,
        "QCheckBox[Show external keyboard in clips]",
    );
    space(ui, 8.0);
    let top = ui.cursor().min;
    small_label(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(58.0, 36.0)),
        "Window",
        Color::Text,
    );
    let mut wc = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + 66.0, top.y),
        Vec2::new(combo_w, 36.0),
    )));
    let mut combo = Combo::new(vec!["No titled windows detected".into()], 0)
        .with_width(combo_w)
        .with_height(36.0);
    combo.enabled = false;
    combo.ui(
        &mut wc,
        &t,
        &mut state.icons,
        "_DropdownCombo[No titled windows detected]",
    );
    super::audio::mic_refresh_button(
        ui,
        &t,
        state,
        Rect::from_min_size(Pos2::new(top.x + w - 26.0, top.y + 5.0), Vec2::splat(26.0)),
        "micRefreshBtn#2",
    );
    ui.allocate_exact_size(Vec2::new(w, 36.0), Sense::hover());
    space(ui, 8.0);
    // Keyboard source preview: 132 px, BG, 1 px BORDER_HI inset, empty text.
    let top = ui.cursor().min;
    let prect = Rect::from_min_size(top, Vec2::new(w, 132.0));
    ui.painter().rect_filled(prect, 0.0, t.color(Color::Bg));
    ui.painter().rect_stroke(
        prect.shrink(1.0),
        0.0,
        Stroke::new(1.0, t.color(Color::BorderHi)),
        StrokeKind::Inside,
    );
    let g = ui.painter().layout_no_wrap(
        "SELECT A WINDOW TO PREVIEW".into(),
        crate::fonts::id(11.0, Weight::W400),
        t.color(Color::TextDim),
    );
    let gs = g.size();
    ui.painter().galley(
        (prect.center() - gs / 2.0).round(),
        g,
        t.color(Color::TextDim),
    );
    fidelity::mark(ui, "keyboardSourcePreview", prect).done();
    ui.allocate_exact_size(Vec2::new(w, 132.0), Sense::hover());
    space(ui, 8.0);
    let top = ui.cursor().min;
    small_label(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(58.0, 41.0)),
        "Key color",
        Color::Text,
    );
    let color_hex = kb
        .get("color")
        .and_then(Value::as_str)
        .unwrap_or("#00ff00")
        .to_uppercase();
    let swatch = Rect::from_min_size(Pos2::new(top.x + 66.0, top.y + 7.0), Vec2::new(36.0, 26.0));
    ui.painter().rect_filled(
        swatch,
        CornerRadius::same(geom::RADIUS_SM as u8),
        crate::tokens::parse_hex(&color_hex).unwrap_or(Color32::GREEN),
    );
    ui.painter().rect_stroke(
        swatch,
        CornerRadius::same(geom::RADIUS_SM as u8),
        Stroke::new(1.0, t.color(Color::BorderHi)),
        StrokeKind::Inside,
    );
    let vg = galley(
        ui,
        &color_hex,
        size::BODY,
        Weight::W400,
        t.color(Color::TextDim),
    );
    let vrect = Rect::from_min_size(
        Pos2::new(top.x + 110.0, top.y),
        Vec2::new(vg.size().x.round(), 41.0),
    );
    paint_text(ui, vg, vrect, Align::Min, 0.0, t.color(Color::TextDim));
    fidelity::mark(ui, format!("QLabel[{color_hex}]"), vrect)
        .text(&color_hex, size::BODY, Weight::W400)
        .done();
    let pick_w = (galley(
        ui,
        "Pick from preview",
        size::LABEL,
        Weight::W600,
        Color32::WHITE,
    )
    .size()
    .x + 34.0)
        .ceil();
    let mut pk = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + w - pick_w, top.y),
        Vec2::new(pick_w, 41.0),
    )));
    buttons::button_with(
        &mut pk,
        &t,
        None,
        Kind::Outline,
        "Pick from preview",
        "QPushButton[Pick from preview]",
        &Opts {
            enabled: false,
            ..Opts::default()
        },
    );
    ui.allocate_exact_size(Vec2::new(w, 41.0), Sense::hover());
    space(ui, 8.0);
    let top = ui.cursor().min;
    let lh = crate::fonts::qt_wrapped_line_height(size::MICRO, Weight::W600);
    small_label(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(58.0, lh)),
        "Intensity",
        Color::Text,
    );
    let mut intensity = kb.get("intensity").and_then(Value::as_i64).unwrap_or(58) as f32;
    let mut sl = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + 66.0, top.y - 4.0),
        Vec2::new(w - 66.0 - 8.0 - 34.0, 26.0),
    )));
    if slider::settings_slider(
        &mut sl,
        &t,
        &mut intensity,
        0.0..=100.0,
        "keyboardIntensitySlider",
    )
    .changed()
    {
        let mut kb2 = kb.clone();
        kb2["intensity"] = json!(intensity as i64);
        state.set_setting("third_party_keyboard", kb2);
    }
    let text = format!("{}%", intensity as i64);
    let vrect = Rect::from_min_size(Pos2::new(top.x + w - 34.0, top.y), Vec2::new(34.0, lh));
    let g = galley(ui, &text, size::BODY, Weight::W400, t.color(Color::TextDim));
    paint_text(ui, g, vrect, Align::Min, 0.0, t.color(Color::TextDim));
    fidelity::mark(ui, format!("QLabel[{text}]"), vrect)
        .text(&text, size::BODY, Weight::W400)
        .done();
    ui.allocate_exact_size(Vec2::new(w, lh), Sense::hover());
    space(ui, 6.0);
    section::label_wrapped_fit(
        ui,
        &t,
        KEYBOARD_HINT,
        size::BODY,
        Weight::W400,
        Color::TextDim,
        w,
    );
    space(ui, 20.0);
    // Preview background.
    option_header(ui, &t, "Preview background");
    let bg_path = state
        .settings
        .get_str("input_overlay_preview_background")
        .unwrap_or("")
        .to_owned();
    let shown = if bg_path.is_empty() {
        "DEFAULT · DESKTOP SCREENSHOT".to_owned()
    } else {
        bg_path.clone()
    };
    let top = ui.cursor().min;
    let frect = Rect::from_min_size(top, Vec2::new(w, 38.0));
    ui.painter().rect_filled(
        frect,
        CornerRadius::same(geom::RADIUS as u8),
        t.color(Color::Surface3),
    );
    ui.painter().rect_stroke(
        frect,
        CornerRadius::same(geom::RADIUS as u8),
        Stroke::new(1.0, t.color(Color::Border)),
        StrokeKind::Inside,
    );
    let g = galley(ui, &shown, size::BODY, Weight::W400, t.color(Color::Text));
    paint_text(ui, g, frect, Align::Min, 12.0, t.color(Color::Text));
    fidelity::mark(
        ui,
        format!("QLineEdit[{}]", crate::fidelity::marker_label(&shown)),
        frect,
    )
    .sample(
        "fill",
        Pos2::new(frect.min.x + 4.0, frect.center().y),
        Color::Surface3,
    )
    .edge(Color::Border)
    .text(&shown, size::BODY, Weight::W400)
    .done();
    ui.allocate_exact_size(Vec2::new(w, 38.0), Sense::hover());
    space(ui, 6.0);
    let top = ui.cursor().min;
    let cw = (galley(
        ui,
        "Choose image",
        size::LABEL,
        Weight::W600,
        Color32::WHITE,
    )
    .size()
    .x + 34.0)
        .ceil();
    let dw = (galley(ui, "Use default", size::LABEL, Weight::W600, Color32::WHITE)
        .size()
        .x
        + 38.0)
        .ceil();
    let mut c1 = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x, top.y + 1.0),
        Vec2::new(cw, 41.0),
    )));
    if buttons::outline(&mut c1, &t, "Choose image", "QPushButton[Choose image]").clicked() {
        state.browse_requested = Some(BrowseKind::PreviewBackground);
    }
    let mut c2 = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + cw + 8.0, top.y),
        Vec2::new(dw, 43.0),
    )));
    if buttons::secondary(&mut c2, &t, "Use default", "QPushButton[Use default]").clicked() {
        state.set_setting("input_overlay_preview_background", json!(""));
    }
    ui.allocate_exact_size(Vec2::new(w, 43.0), Sense::hover());
}

/// `UnifiedOverlayPreview`: a 16:9 canvas fitted inside the widget (1 px
/// margin), the desktop screenshot cover-drawn under a BG tint, a 1 px
/// BORDER_HI frame, and draggable overlay boxes (camera, keyboard, image
/// layers) with a 20 px header strip and a 13 px grip.
fn overlay_stage(ui: &mut Ui, state: &mut AppState, rect: Rect, bg_path: &str) {
    let t = state.tokens.clone();
    ui.painter().rect_filled(rect, 0.0, t.color(Color::Bg));
    let avail_w = (rect.width() - 2.0).max(1.0);
    let avail_h = (rect.height() - 2.0).max(1.0);
    let cw = avail_w.min(avail_h * 16.0 / 9.0);
    let ch = cw * 9.0 / 16.0;
    let canvas = Rect::from_min_size(
        Pos2::new(
            rect.min.x + 1.0 + ((avail_w - cw) / 2.0).floor(),
            rect.min.y + 1.0 + ((avail_h - ch) / 2.0).floor(),
        ),
        Vec2::new(cw.floor(), ch.floor()),
    );
    let bg = if bg_path.is_empty() {
        crate::assets_dir().join("preview_desktop.png")
    } else {
        std::path::PathBuf::from(bg_path)
    };
    match state.icons.file_texture(ui.ctx(), &bg, 0.0) {
        Some(tex) => {
            // Cover: crop the source to the canvas aspect.
            let (sw, sh) = (tex.size.x, tex.size.y);
            let target_ratio = canvas.width() / canvas.height().max(1.0);
            let source_ratio = sw / sh.max(1.0);
            let uv = if source_ratio > target_ratio {
                let wanted = sh * target_ratio;
                let left = (sw - wanted) / 2.0 / sw;
                Rect::from_min_max(Pos2::new(left, 0.0), Pos2::new(left + wanted / sw, 1.0))
            } else {
                let wanted = sw / target_ratio;
                let top = (sh - wanted) / 2.0 / sh;
                Rect::from_min_max(Pos2::new(0.0, top), Pos2::new(1.0, top + wanted / sh))
            };
            ui.painter().image(tex.id, canvas, uv, Color32::WHITE);
            ui.painter()
                .rect_filled(canvas, 0.0, t.color(Color::Bg).gamma_multiply(34.0 / 255.0));
        }
        None => {
            ui.painter()
                .rect_filled(canvas, 0.0, t.color(Color::Surface2));
            let horizon = canvas.min.y + canvas.height() * 0.57;
            ui.painter().rect_filled(
                Rect::from_min_max(Pos2::new(canvas.min.x, horizon), canvas.max),
                0.0,
                t.color(Color::Surface1),
            );
            for (fx, fy, fw, fh) in [(0.08, 0.14, 0.26, 0.30), (0.66, 0.20, 0.20, 0.24)] {
                ui.painter().rect_filled(
                    Rect::from_min_size(
                        Pos2::new(
                            canvas.min.x + canvas.width() * fx,
                            canvas.min.y + canvas.height() * fy,
                        ),
                        Vec2::new(canvas.width() * fw, canvas.height() * fh),
                    ),
                    0.0,
                    t.color(Color::Border),
                );
            }
            let g = ui.painter().layout_no_wrap(
                "CLIP PREVIEW".into(),
                crate::fonts::id(11.0, Weight::W700),
                t.color(Color::TextMuted),
            );
            ui.painter().galley(
                canvas.min + Vec2::new(10.0, 8.0),
                g,
                t.color(Color::TextMuted),
            );
        }
    }
    ui.painter().rect_stroke(
        canvas,
        0.0,
        Stroke::new(1.0, t.color(Color::BorderHi)),
        StrokeKind::Outside,
    );
    fidelity::mark(ui, "unifiedOverlayPreview", rect).done();
    // Overlay boxes: image layers, keyboard, camera (camera on top).
    let layers: Vec<Value> = state
        .settings
        .get("image_overlays")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let kb = state
        .settings
        .get("third_party_keyboard")
        .cloned()
        .unwrap_or(json!({}));
    let camera_on = state.flag("camera_enabled", false);
    let kb_on = kb.get("enabled").and_then(Value::as_bool).unwrap_or(false)
        && kb.get("hwnd").and_then(Value::as_i64).unwrap_or(0) != 0;
    let mut boxes: Vec<(String, crate::state::FracRect, bool, String)> = Vec::new();
    for (i, layer) in layers.iter().enumerate() {
        boxes.push((
            format!("image:{i}"),
            frac_rect(layer.get("rect"), (0.1, 0.1, 0.3, 0.3)),
            layer
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            format!("IMAGE {}", i + 1),
        ));
    }
    boxes.push((
        "keyboard".into(),
        frac_rect(kb.get("rect"), (0.3, 0.7, 0.4, 0.25)),
        kb_on,
        "KEYBOARD".into(),
    ));
    boxes.push((
        "camera".into(),
        frac_rect(
            state.settings.get("camera_overlay_rect"),
            (0.72, 0.64, 0.25, 0.34),
        ),
        camera_on,
        "CAMERA".into(),
    ));
    let pixel = |r: (f64, f64, f64, f64)| {
        Rect::from_min_size(
            Pos2::new(
                canvas.min.x + (canvas.width() as f64 * r.0) as f32,
                canvas.min.y + (canvas.height() as f64 * r.1) as f32,
            ),
            Vec2::new(
                (canvas.width() as f64 * r.2) as f32,
                (canvas.height() as f64 * r.3) as f32,
            ),
        )
    };
    let selected_image = state.selected_image_layer;
    let mut moved: Option<(String, crate::state::FracRect)> = None;
    for (kind, frac, enabled, label) in &boxes {
        let bx = pixel(*frac);
        let selected = kind
            .strip_prefix("image:")
            .and_then(|i| i.parse::<usize>().ok())
            == Some(selected_image)
            && !layers.is_empty();
        let content =
            Rect::from_min_max(bx.min + Vec2::new(2.0, 22.0), bx.max - Vec2::new(2.0, 2.0));
        let tint = t.color(Color::Accent).gamma_multiply(if *enabled {
            22.0 / 255.0
        } else {
            10.0 / 255.0
        });
        ui.painter().rect_filled(content, 0.0, tint);
        // The live camera frame, letterboxed ("fit") in the box.
        let live = (kind == "camera" && *enabled)
            .then(|| camera_texture(ui, state))
            .flatten();
        if let Some(tex) = &live {
            let size = tex.size_vec2();
            let scale = (content.width() / size.x).min(content.height() / size.y);
            ui.painter().image(
                tex.id(),
                Rect::from_center_size(content.center(), size * scale),
                Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        if !kind.starts_with("image") && live.is_none() {
            let text = if kind == "camera" {
                "CAMERA PREVIEW"
            } else {
                "KEYBOARD PREVIEW"
            };
            let g = ui.painter().layout_no_wrap(
                text.into(),
                crate::fonts::id(11.0, Weight::W700),
                t.color(Color::TextDim),
            );
            ui.painter().galley(
                (content.center() - g.size() / 2.0).round(),
                g,
                t.color(Color::TextDim),
            );
        }
        let border = t.color(if *enabled {
            Color::Accent
        } else {
            Color::TextMuted
        });
        ui.painter().rect_stroke(
            bx,
            0.0,
            Stroke::new(if selected { 3.0 } else { 2.0 }, border),
            StrokeKind::Middle,
        );
        ui.painter().rect_filled(
            Rect::from_min_size(bx.min, Vec2::new(bx.width(), 20.0)),
            0.0,
            t.color(Color::Bg).gamma_multiply(185.0 / 255.0),
        );
        let g = ui.painter().layout_no_wrap(
            label.clone(),
            crate::fonts::id(11.0, Weight::W700),
            t.color(if *enabled {
                Color::Text
            } else {
                Color::TextDim
            }),
        );
        ui.painter().galley(
            Pos2::new(
                bx.min.x + 8.0,
                bx.min.y + ((20.0 - g.size().y) / 2.0).round(),
            ),
            g,
            t.color(Color::Text),
        );
        ui.painter().rect_filled(
            Rect::from_min_size(
                Pos2::new(bx.max.x - 13.0, bx.max.y - 13.0),
                Vec2::splat(13.0),
            ),
            0.0,
            border,
        );
        fidelity::mark(ui, format!("overlayBox[{label}]"), bx).done();
        // Drag to move; the grip (24×24 at the corner) resizes.
        let resp = ui.interact(
            bx,
            ui.scope_id().with(("overlay", kind)),
            Sense::click_and_drag(),
        );
        if resp.drag_started() {
            let grip = Rect::from_min_size(
                Pos2::new(bx.max.x - 18.0, bx.max.y - 18.0),
                Vec2::splat(24.0),
            );
            let on_grip = ui
                .ctx()
                .pointer_interact_pos()
                .is_some_and(|p| grip.contains(p));
            state.overlay_drag = Some((kind.clone(), on_grip, *frac));
        }
        if resp.dragged()
            && let Some((dk, resize, start)) = state.overlay_drag.clone()
            && dk == *kind
        {
            let delta = resp.drag_delta();
            let (dx, dy) = (
                (delta.x / canvas.width()) as f64,
                (delta.y / canvas.height()) as f64,
            );
            let (mut x, mut y, mut w, mut h) =
                state.overlay_live.get(kind).copied().unwrap_or(start);
            if resize {
                w = (w + dx).clamp(0.05, 1.0 - x);
                h = (h + dy).clamp(0.05, 1.0 - y);
            } else {
                x = (x + dx).clamp(0.0, 1.0 - w);
                y = (y + dy).clamp(0.0, 1.0 - h);
            }
            state.overlay_live.insert(kind.clone(), (x, y, w, h));
        }
        if resp.drag_stopped()
            && let Some(live) = state.overlay_live.remove(kind)
        {
            moved = Some((kind.clone(), live));
            state.overlay_drag = None;
        }
    }
    // Live rects override while dragging.
    if let Some((kind, (x, y, w, h))) = moved {
        let value = json!({"x": (x * 1000.0).round() / 1000.0, "y": (y * 1000.0).round() / 1000.0, "w": (w * 1000.0).round() / 1000.0, "h": (h * 1000.0).round() / 1000.0});
        if kind == "camera" {
            state.set_setting("camera_overlay_rect", value);
        } else if kind == "keyboard" {
            let mut kb2 = kb.clone();
            kb2["rect"] = value;
            state.set_setting("third_party_keyboard", kb2);
        } else if let Some(i) = kind
            .strip_prefix("image:")
            .and_then(|i| i.parse::<usize>().ok())
        {
            let mut l = layers.clone();
            if let Some(layer) = l.get_mut(i) {
                layer["rect"] = value;
            }
            state.set_setting("image_overlays", Value::Array(l));
        }
    }
    if !state.overlay_live.is_empty() {
        ui.ctx().request_repaint();
    }
}

/// The camera preview as a texture, re-uploaded when a new frame arrives.
fn camera_texture(ui: &Ui, state: &mut AppState) -> Option<egui::TextureHandle> {
    let (seq, img) = state.camera_preview.as_ref()?;
    match &state.camera_preview_texture {
        Some((s, tex)) if s == seq => Some(tex.clone()),
        _ => {
            let tex =
                ui.ctx()
                    .load_texture("camera-preview", img.clone(), egui::TextureOptions::LINEAR);
            state.camera_preview_texture = Some((*seq, tex.clone()));
            Some(tex)
        }
    }
}
