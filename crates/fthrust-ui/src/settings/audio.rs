//! The Audio tab (`_make_audio_page`): microphone and Gary Mode (left),
//! notification sounds (right), and audio capture below.

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use serde_json::json;

use super::space;
use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::Icon;
use crate::motion;
use crate::popups::source::monitor_label;
use crate::state::{AppState, BrowseKind};
use crate::tokens::{Color, Tokens, geom, size};
use crate::widgets::buttons::{self, IconButtonStyle, icon_button};
use crate::widgets::combo::Combo;
use crate::widgets::{check, galley, lerp, paint_text, section, slider};

const SOUNDS: [(&str, &str); 7] = [
    ("master", "Master volume"),
    ("clip", "Clip captured"),
    ("screenshot", "Screenshot saved"),
    ("error", "Capture failed"),
    ("startup", "Startup"),
    ("upload_successful", "Upload successful"),
    ("upload_failed", "Upload failed"),
];
const GAIN_WARNING: &str = "⚠ Above 100% applies a digital boost and may distort loud input. Restart capture for Windows engine gain changes to take effect.";
const PLAYBACK_HINT: &str =
    "If this device is missing when capture starts, the default output is captured instead.";
const MODE_HINT: &str = "Combined audio is the default. Enable separate tracks only when you need independent system and microphone mixing in the editor.";
/// Gary threshold gap.
pub const MIN_THRESHOLD_GAP: i64 = 5;

/// `micRefreshBtn`: 26×26, SURFACE_2, 1 px BORDER, r6, 13 px icon; hover ACCENT.
pub fn mic_refresh_button(
    ui: &mut Ui,
    t: &Tokens,
    state: &mut AppState,
    rect: Rect,
    name: &str,
) -> bool {
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let style = IconButtonStyle {
        size: Vec2::splat(26.0),
        bg: Some(Color::Surface2),
        hover_bg: None,
        pressed_bg: None,
        border: Some(Color::Border),
        hover_border: Some(Color::Accent),
        radius: geom::RADIUS,
        hover_tint: Some(Color::Accent),
        enabled: true,
    };
    icon_button(
        &mut c,
        t,
        &mut state.icons,
        Icon::Refresh,
        13.0,
        style,
        name,
    )
    .clicked()
}

/// A plain 17 px label in a fixed-width cell of `h`.
fn cell_label(ui: &mut Ui, t: &Tokens, rect: Rect, text: &str, color: Color, align: Align) {
    let g = galley(ui, text, size::BODY_L, Weight::W400, t.color(color));
    paint_text(ui, g, rect, align, 0.0, t.color(color));
    fidelity::mark(ui, format!("QLabel[{}]", &text[..text.len().min(40)]), rect)
        .text(text, size::BODY_L, Weight::W400)
        .done();
}

/// `_MicLevelMeter`: 36 px tall; a 10 px bar (BG, 1 px HAIRLINE frame)
/// with the ACCENT level, a 2 px TEXT peak, and the Gary MIN (WARNING) /
/// MAX (TEXT) handles with 7 pt bold labels; dragging a handle moves it.
pub fn mic_meter(ui: &mut Ui, state: &mut AppState, width: f32, name: &str) {
    let t = state.tokens.clone();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::click_and_drag());
    let (w, h) = (rect.width(), rect.height());
    let bar_h = 10.0;
    let bar_y = rect.min.y + ((h - bar_h) / 2.0).floor();
    let bar = Rect::from_min_size(Pos2::new(rect.min.x, bar_y), Vec2::new(w, bar_h));
    ui.painter().rect_filled(bar, 0.0, t.color(Color::Bg));
    ui.painter().rect_stroke(
        bar,
        0.0,
        Stroke::new(1.0, t.color(Color::Hairline)),
        StrokeKind::Inside,
    );
    let (level, peak) = state.mic_level.unwrap_or((0.0, 0.0));
    let fill_w = ((w - 2.0) * level).floor().max(0.0);
    if fill_w > 0.0 {
        ui.painter().rect_filled(
            Rect::from_min_size(
                Pos2::new(rect.min.x + 1.0, bar_y + 1.0),
                Vec2::new(fill_w, bar_h - 2.0),
            ),
            0.0,
            t.color(Color::Accent),
        );
    }
    let peak_x = ((w - 2.0) * peak).floor();
    if peak_x > 0.0 {
        ui.painter().rect_filled(
            Rect::from_min_size(
                Pos2::new(rect.min.x + 1.0 + peak_x, bar_y + 1.0),
                Vec2::new(2.0, bar_h - 2.0),
            ),
            0.0,
            t.color(Color::Text),
        );
    }
    let mut gmin = state
        .settings
        .get_i64("gary_min_level")
        .unwrap_or(55)
        .clamp(1, 100);
    let mut gmax = state
        .settings
        .get_i64("gary_max_level")
        .unwrap_or(85)
        .clamp(1, 100);
    let threshold_x = |v: i64| rect.min.x + 1.0 + ((w - 2.0) * v as f32 / 100.0).floor();
    // Drag the nearest handle.
    if (resp.drag_started() || resp.dragged())
        && let Some(p) = ui.ctx().pointer_interact_pos()
    {
        let which = state.gary_drag.unwrap_or_else(|| {
            if (p.x - threshold_x(gmin)).abs() <= (p.x - threshold_x(gmax)).abs() {
                0
            } else {
                1
            }
        });
        state.gary_drag = Some(which);
        let value =
            (((p.x - rect.min.x - 1.0).clamp(0.0, w - 2.0)) * 100.0 / (w - 2.0)).round() as i64;
        if which == 0 {
            gmin = value.clamp(1, gmax - MIN_THRESHOLD_GAP);
        } else {
            gmax = value.clamp(gmin + MIN_THRESHOLD_GAP, 100);
        }
        state.settings.set("gary_min_level", json!(gmin));
        state.settings.set("gary_max_level", json!(gmax));
    }
    if resp.drag_stopped() {
        state.gary_drag = None;
        let _ = state.settings.save();
    }
    let font = crate::fonts::id(9.0, Weight::W700);
    for (value, label, color, is_min) in [
        (gmin, "MIN", Color::Warning, true),
        (gmax, "MAX", Color::Text, false),
    ] {
        let x = threshold_x(value);
        ui.painter().rect_filled(
            Rect::from_min_size(Pos2::new(x - 3.0, bar_y - 4.0), Vec2::new(8.0, 5.0)),
            0.0,
            t.color(color),
        );
        ui.painter().rect_filled(
            Rect::from_min_size(Pos2::new(x, bar_y - 4.0), Vec2::new(2.0, bar_h + 7.0)),
            0.0,
            t.color(color),
        );
        let text = format!("{label} {value}");
        let g = ui
            .painter()
            .layout_no_wrap(text, font.clone(), t.color(color));
        let (tx, align) = if is_min {
            ((x - 48.0).max(rect.min.x), Align::Max)
        } else {
            ((x + 5.0).min(rect.max.x - 48.0), Align::Min)
        };
        let trect = Rect::from_min_size(Pos2::new(tx, rect.min.y), Vec2::new(44.0, 10.0));
        let gx = match align {
            Align::Max => trect.max.x - g.size().x,
            _ => trect.min.x,
        };
        ui.painter()
            .galley(Pos2::new(gx.round(), trect.min.y), g, t.color(color));
    }
    fidelity::mark(ui, name, rect)
        .sample(
            "fill",
            Pos2::new(rect.min.x + 1.0 + (w - 2.0) * level * 0.5, bar.center().y),
            if level > 0.0 {
                Color::Accent
            } else {
                Color::Bg
            },
        )
        .done();
    let _ = Color32::TRANSPARENT;
}

fn gary_button(ui: &mut Ui, t: &Tokens, rect: Rect, text: &str, name: &str) -> bool {
    let resp = ui.interact(rect, ui.scope_id().with(name), Sense::click());
    let blend = motion::hover(
        ui.ctx(),
        resp.id.with("hover"),
        resp.hovered(),
        t.reduce_motion,
    );
    let border = lerp(t.color(Color::BorderHi), t.color(Color::Accent), blend);
    let fg = lerp(t.color(Color::Text), t.color(Color::Accent), blend);
    let radius = CornerRadius::same(geom::RADIUS as u8);
    ui.painter()
        .rect_stroke(rect, radius, Stroke::new(1.0, border), StrokeKind::Inside);
    let g = galley(ui, text, size::MICRO, Weight::W600, fg);
    paint_text(ui, g, rect, Align::Center, 0.0, fg);
    fidelity::mark(ui, name, rect)
        .edge(Color::BorderHi)
        .text(text, size::MICRO, Weight::W600)
        .done();
    resp.clicked()
}

fn left_height(state: &AppState) -> f32 {
    let warn = if state.settings.get_i64("mic_volume").unwrap_or(100) > 100 {
        2.0 * 18.0
    } else {
        0.0
    };
    30.0 + 12.0
        + 36.0
        + 8.0
        + 26.0
        + 12.0
        + 26.0
        + warn
        + 8.0
        + 36.0
        + 10.0
        + 26.0
        + 18.0
        + 30.0
        + 8.0
        + 66.0
}

fn right_height() -> f32 {
    30.0 + 12.0 + 26.0 + 12.0 + 38.0 + 18.0 + 18.0 + 8.0 + 7.0 * 33.0
}

/// The Audio page body (page margins are zero here).
pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    let top = ui.cursor().min;
    let left_w = ((w - 1.0) / 2.0).ceil();
    let right_w = w - 1.0 - left_w;
    let (lh, rh) = (left_height(state), right_height());
    let row_h = lh.max(rh);
    let left_y = top.y + ((row_h - lh) / 2.0).floor();
    // Left column.
    let mut left = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x, left_y),
        Vec2::new(left_w - geom::SECTION_GAP, f32::INFINITY),
    )));
    left.spacing_mut().item_spacing = Vec2::ZERO;
    left_column(&mut left, state);
    // Separator.
    crate::widgets::rule_v(
        ui,
        top.x + left_w,
        top.y,
        top.y + row_h,
        t.color(Color::ShellDivider),
    );
    // Right column.
    let mut right = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + left_w + 1.0 + geom::SECTION_GAP, top.y),
        Vec2::new(right_w - geom::SECTION_GAP, f32::INFINITY),
    )));
    right.spacing_mut().item_spacing = Vec2::ZERO;
    right_column(&mut right, state);
    ui.allocate_exact_size(Vec2::new(w, row_h), Sense::hover());
    // Audio capture.
    space(ui, 24.0);
    let mut full = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        ui.cursor().min,
        Vec2::new(w, f32::INFINITY),
    )));
    full.spacing_mut().item_spacing = Vec2::ZERO;
    let start = full.cursor().min.y;
    section::hsep(&mut full, &t, "audioHsep");
    space(&mut full, 20.0);
    section::flat_header(&mut full, &t, "Audio Capture", "Audio Capture");
    space(&mut full, 12.0);
    let mut on = state.flag("audio_capture_enabled", true);
    if check::checkbox_with(
        &mut full,
        &t,
        &mut on,
        "Enable audio capture",
        true,
        true,
        "QCheckBox[Enable audio capture]",
    )
    .changed()
    {
        state.set_setting("audio_capture_enabled", json!(on));
        state.restart_pending = true;
    }
    space(&mut full, 12.0);
    let rtop = full.cursor().min;
    let label_w = galley(
        &full,
        "Playback device to capture",
        size::BODY_L,
        Weight::W400,
        Color32::WHITE,
    )
    .size()
    .x
    .ceil();
    cell_label(
        &mut full,
        &t,
        Rect::from_min_size(rtop, Vec2::new(label_w, 41.0)),
        "Playback device to capture",
        Color::Text,
        Align::Min,
    );
    fidelity::mark(
        &full,
        "playbackDeviceLabel",
        Rect::from_min_size(rtop, Vec2::new(label_w, 41.0)),
    )
    .text("Playback device to capture", size::BODY_L, Weight::W400)
    .done();
    let refresh_w = (galley(&full, "Refresh", size::LABEL, Weight::W600, Color32::WHITE)
        .size()
        .x
        + 34.0)
        .ceil();
    let combo_w = w - label_w - 12.0 - 12.0 - refresh_w;
    let mut c = full.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(rtop.x + label_w + 12.0, rtop.y + 1.0),
        Vec2::new(combo_w, 38.0),
    )));
    let saved = state
        .settings
        .get_str("audio_capture_device")
        .unwrap_or("")
        .to_owned();
    let mut items = vec![("System default".to_owned(), String::new())];
    items.extend(state.audio_devices.iter().cloned());
    let sel = items.iter().position(|(_, id)| *id == saved).unwrap_or(0);
    let mut combo =
        Combo::new(items.iter().map(|(l, _)| l.clone()).collect(), sel).with_width(combo_w);
    let name = format!("QComboBox[{}]", combo.current());
    if combo.ui(&mut c, &t, &mut state.icons, &name).changed() {
        state.set_setting("audio_capture_device", json!(items[combo.selected].1));
        state.restart_pending = true;
    }
    let mut rb = full.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(rtop.x + w - refresh_w, rtop.y),
        Vec2::new(refresh_w, 41.0),
    )));
    if buttons::outline(&mut rb, &t, "Refresh", "playbackDeviceRefresh").clicked() {
        state.refresh_audio_devices_requested = true;
    }
    full.allocate_exact_size(Vec2::new(w, 41.0), Sense::hover());
    space(&mut full, 8.0);
    section::label_wrapped_fit(
        &mut full,
        &t,
        PLAYBACK_HINT,
        size::BODY_L,
        Weight::W400,
        Color::Text,
        w,
    );
    space(&mut full, 12.0);
    let mut separate = state
        .settings
        .get_str("audio_capture_mode")
        .map(|m| fthrust_core::capture_config::normalize_audio_capture_mode(m) == "separated")
        .unwrap_or(false);
    if check::checkbox_with(
        &mut full,
        &t,
        &mut separate,
        "Keep system and microphone audio as separate tracks",
        true,
        true,
        "separateAudioCheck",
    )
    .changed()
    {
        state.set_setting(
            "audio_capture_mode",
            json!(if separate { "separated" } else { "combined" }),
        );
        state.restart_pending = true;
    }
    space(&mut full, 4.0);
    section::label_wrapped_fit(
        &mut full,
        &t,
        MODE_HINT,
        size::BODY,
        Weight::W400,
        Color::TextDim,
        w,
    );
    let h = full.cursor().min.y - start;
    ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
}

fn left_column(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    section::flat_header(ui, &t, "Microphone", "Microphone");
    space(ui, 12.0);
    let top = ui.cursor().min;
    cell_label(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(100.0, 36.0)),
        "Input device",
        Color::Text,
        Align::Min,
    );
    let combo_w = w - 100.0 - 8.0 - 8.0 - 26.0;
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + 108.0, top.y),
        Vec2::new(combo_w, 36.0),
    )));
    let saved_name = state
        .settings
        .get_str("mic_device_name")
        .unwrap_or("")
        .to_owned();
    let mut items = vec![("System Default".to_owned(), String::new())];
    items.extend(state.mic_devices.iter().cloned());
    let sel = items
        .iter()
        .position(|(n, _)| !saved_name.is_empty() && *n == saved_name)
        .unwrap_or(0);
    let mut combo = Combo::new(items.iter().map(|(l, _)| l.clone()).collect(), sel)
        .with_width(combo_w)
        .with_height(36.0);
    let name = format!("_DropdownCombo[{}]", combo.current());
    if combo.ui(&mut c, &t, &mut state.icons, &name).changed() {
        let (n, id) = &items[combo.selected];
        state.set_setting(
            "mic_device_name",
            if combo.selected == 0 {
                json!(null)
            } else {
                json!(n)
            },
        );
        state.set_setting(
            "mic_device_id",
            if combo.selected == 0 {
                json!(null)
            } else {
                json!(id)
            },
        );
        state.restart_pending = true;
    }
    if mic_refresh_button(
        ui,
        &t,
        state,
        Rect::from_min_size(Pos2::new(top.x + w - 26.0, top.y + 5.0), Vec2::splat(26.0)),
        "micRefreshBtn",
    ) {
        state.refresh_audio_devices_requested = true;
    }
    ui.allocate_exact_size(Vec2::new(w, 36.0), Sense::hover());
    space(ui, 8.0);
    let mut mono = true;
    check::checkbox_with(
        ui,
        &t,
        &mut mono,
        "Downmix stereo microphone to mono",
        false,
        true,
        "QCheckBox[Downmix stereo microphone to mono]",
    );
    space(ui, 12.0);
    // Gain row.
    let top = ui.cursor().min;
    cell_label(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(100.0, 26.0)),
        "Microphone gain",
        Color::Text,
        Align::Min,
    );
    let mut gain = state.settings.get_i64("mic_volume").unwrap_or(100) as f32;
    let mut s = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + 108.0, top.y),
        Vec2::new(w - 108.0 - 8.0 - 42.0, 26.0),
    )));
    if slider::settings_slider(&mut s, &t, &mut gain, 0.0..=200.0, "micGainSlider").changed() {
        state.set_setting("mic_volume", json!(gain as i64));
    }
    cell_label(
        ui,
        &t,
        Rect::from_min_size(Pos2::new(top.x + w - 42.0, top.y), Vec2::new(42.0, 26.0)),
        &format!("{}%", gain as i64),
        Color::Text,
        Align::Max,
    );
    ui.allocate_exact_size(Vec2::new(w, 26.0), Sense::hover());
    if gain > 100.0 {
        section::label_wrapped_fit(
            ui,
            &t,
            GAIN_WARNING,
            size::MICRO,
            Weight::W400,
            Color::Warning,
            w,
        );
    }
    space(ui, 8.0);
    let top = ui.cursor().min;
    cell_label(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(100.0, 36.0)),
        "Live level",
        Color::Text,
        Align::Min,
    );
    let mut m = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + 108.0, top.y),
        Vec2::new(w - 108.0, 36.0),
    )));
    mic_meter(&mut m, state, w - 108.0, "micLevelMeter");
    ui.allocate_exact_size(Vec2::new(w, 36.0), Sense::hover());
    space(ui, 10.0);
    let mut loopback = state.flag("mic_loopback", false);
    if check::checkbox_with(
        ui,
        &t,
        &mut loopback,
        "Monitor (route mic to speakers so I can hear it)",
        true,
        true,
        "QCheckBox[Monitor (route mic to speakers so I ",
    )
    .changed()
    {
        state.set_setting("mic_loopback", json!(loopback));
    }
    space(ui, 18.0);
    section::flat_header(ui, &t, "Gary Mode", "Gary Mode");
    space(ui, 8.0);
    let top = ui.cursor().min;
    let panel = Rect::from_min_size(top, Vec2::new(w, 66.0));
    fidelity::mark(ui, "garyPanel", panel).done();
    let preview = Rect::from_min_size(Pos2::new(top.x + 10.0, top.y + 4.0), Vec2::new(76.0, 58.0));
    ui.painter().rect_filled(
        preview,
        CornerRadius::same(geom::RADIUS as u8),
        t.color(Color::Bg),
    );
    let gary_path = state
        .settings
        .get_str("gary_image_path")
        .filter(|p| !p.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate::assets_dir().join("gary.png"));
    if let Some(tex) = state.icons.file_texture(ui.ctx(), &gary_path, 76.0) {
        let (iw, ih) = (tex.size.x, tex.size.y);
        let scale = (74.0 / iw).min(56.0 / ih).min(1.0);
        let size = Vec2::new((iw * scale).round(), (ih * scale).round());
        let irect = Rect::from_center_size(preview.center().round(), size);
        ui.painter().image(
            tex.id,
            irect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    ui.painter().rect_stroke(
        preview,
        CornerRadius::same(geom::RADIUS as u8),
        Stroke::new(1.0, t.color(Color::BorderHi)),
        StrokeKind::Inside,
    );
    fidelity::mark(ui, "garyPreview", preview)
        .edge(Color::BorderHi)
        .done();
    let mut g = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + 98.0, top.y + 20.0),
        Vec2::new(200.0, 26.0),
    )));
    let mut gary = state.flag("gary_mode_enabled", false);
    if check::checkbox(
        &mut g,
        &t,
        &mut gary,
        "Enable Gary Mode",
        "QCheckBox[Enable Gary Mode]",
    )
    .changed()
    {
        state.set_setting("gary_mode_enabled", json!(gary));
    }
    let bw = |text: &str| {
        (galley(ui, text, size::MICRO, Weight::W600, Color32::WHITE)
            .size()
            .x
            + 16.0)
            .ceil()
    };
    let (cw, dw) = (bw("Choose"), bw("Default"));
    let dx = top.x + w - 10.0 - dw;
    let cx = dx - 12.0 - cw;
    if gary_button(
        ui,
        &t,
        Rect::from_min_size(Pos2::new(cx, top.y + 18.0), Vec2::new(cw, 30.0)),
        "Choose",
        "garyImageButton[Choose]",
    ) {
        state.browse_requested = Some(BrowseKind::GaryImage);
    }
    if gary_button(
        ui,
        &t,
        Rect::from_min_size(Pos2::new(dx, top.y + 18.0), Vec2::new(dw, 30.0)),
        "Default",
        "garyImageButton[Default]",
    ) {
        state.set_setting("gary_image_path", json!(null));
        state.icons.clear();
    }
    ui.allocate_exact_size(Vec2::new(w, 66.0), Sense::hover());
}

fn right_column(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    section::flat_header(ui, &t, "Notifications", "Notifications");
    space(ui, 12.0);
    let mut on = state.flag("notification_sounds_enabled", true);
    if check::checkbox_with(
        ui,
        &t,
        &mut on,
        "Enable notification sounds",
        true,
        true,
        "QCheckBox[Enable notification sounds]",
    )
    .changed()
    {
        state.set_setting("notification_sounds_enabled", json!(on));
    }
    space(ui, 12.0);
    let top = ui.cursor().min;
    cell_label(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(150.0, 38.0)),
        "Monitor:",
        Color::Text,
        Align::Min,
    );
    let saved = state
        .settings
        .get_str("notification_monitor")
        .unwrap_or("auto")
        .to_owned();
    let mut items = vec![("Auto (highest Hz)".to_owned(), "auto".to_owned())];
    for m in &state.monitors {
        items.push((
            monitor_label(&m.name, m.width, m.height, m.refresh_hz),
            m.name.clone(),
        ));
    }
    let sel = items.iter().position(|(_, id)| *id == saved).unwrap_or(0);
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + 158.0, top.y),
        Vec2::new(w - 158.0, 38.0),
    )));
    let mut combo =
        Combo::new(items.iter().map(|(l, _)| l.clone()).collect(), sel).with_width(w - 158.0);
    let name = format!("_DropdownCombo[{}]", combo.current());
    if combo.ui(&mut c, &t, &mut state.icons, &name).changed() {
        state.set_setting("notification_monitor", json!(items[combo.selected].1));
    }
    ui.allocate_exact_size(Vec2::new(w, 38.0), Sense::hover());
    space(ui, 18.0);
    section::label_wide(
        ui,
        &t,
        "Notification volume",
        size::MICRO,
        Weight::W600,
        Color::Text,
        None,
    );
    space(ui, 8.0);
    for (i, (key, label)) in SOUNDS.iter().enumerate() {
        let top = ui.cursor().min;
        cell_label(
            ui,
            &t,
            Rect::from_min_size(top, Vec2::new(150.0, 26.0)),
            label,
            Color::Text,
            Align::Min,
        );
        let setting = format!("sound_volume_{key}");
        let mut v = state.settings.get_i64(&setting).unwrap_or(100) as f32;
        let mut s = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + 158.0, top.y),
            Vec2::new(w - 158.0 - 8.0 - 42.0, 26.0),
        )));
        if slider::settings_slider(
            &mut s,
            &t,
            &mut v,
            0.0..=100.0,
            &format!("soundSlider[{key}]"),
        )
        .changed()
        {
            let key: &'static str = SOUNDS[i].0;
            let _ = key;
            state.settings.set(&setting, json!(v as i64));
            let _ = state.settings.save();
        }
        let vrect = Rect::from_min_size(Pos2::new(top.x + w - 42.0, top.y), Vec2::new(42.0, 26.0));
        let text = format!("{}%", v as i64);
        let g = galley(ui, &text, size::BODY_L, Weight::W400, t.color(Color::Text));
        paint_text(ui, g, vrect, Align::Max, 0.0, t.color(Color::Text));
        let marker = if i == 0 {
            "QLabel[100%]#2".to_owned()
        } else {
            format!("QLabel[100%]#{}", i + 2)
        };
        fidelity::mark(ui, marker, vrect)
            .text(&text, size::BODY_L, Weight::W400)
            .done();
        ui.allocate_exact_size(Vec2::new(w, 26.0), Sense::hover());
        space(ui, 7.0);
    }
}
