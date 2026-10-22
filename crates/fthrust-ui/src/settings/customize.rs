//! The Customize tab (`ui/customize_page.py`): a fixed action bar, the info
//! bar and six accordion sections editing the theme draft (`theme.json`);
//! visuals change on Apply, sounds apply immediately.

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use fthrust_core::theme::{
    BUILTIN_FONT_FAMILIES, COLOR_GROUPS, COLOR_LABELS, CUSTOMIZABLE_ICONS,
    CUSTOMIZABLE_SOUND_LABELS, default_capture_card_colors, default_colors,
};
use serde_json::json;

use super::space;
use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::{Icon, IconCache};
use crate::motion;
use crate::state::{AppState, BrowseKind};
use crate::tokens::{Color, Tokens, geom, size};
use crate::widgets::buttons::{self, Kind, Opts};
use crate::widgets::color::{mini_swatch, swatch};
use crate::widgets::combo::Combo;
use crate::widgets::{accordion, galley, lerp, paint_text, section, slider};

const INFO: &str = "Themes include typography, colors, icons, capture-card styling, and sounds  —  Fonts: .ttf, .otf  |  Icons: .png, .jpg, .jpeg, .bmp, .ico, .svg  |  Sounds: .mp3, .wav, .ogg, .flac, .m4a, .wma, .aac";
const SECTIONS: [(&str, &str); 6] = [
    ("01", "Typography"),
    ("02", "Colors"),
    ("03", "Icon Colors"),
    ("04", "Icons"),
    ("05", "Capture Card"),
    ("06", "Sounds & Volumes"),
];
const CC_TOKENS: [(&str, &str); 7] = [
    ("CAPTURE_CARD_BG", "Background"),
    ("CAPTURE_CARD_ACCENT", "Left Accent"),
    ("CAPTURE_CARD_TEXT", "Text"),
    ("CAPTURE_CARD_DIVIDER", "Divider"),
    ("CAPTURE_CARD_STATS_DIM", "Stats Label"),
    ("CAPTURE_CARD_PROGRESS_TRACK", "Progress Track"),
    ("CAPTURE_CARD_PROGRESS_FILL", "Progress Fill"),
];

/// Writes a colour into the theme draft and saves it (visuals unchanged).
pub fn set_draft_color(state: &mut AppState, token: &str, hex: &str) {
    state
        .theme
        .data
        .colors
        .insert(token.to_owned(), hex.to_owned());
    save_theme(state);
}

fn save_theme(state: &mut AppState) {
    if let Err(e) = state.theme.save() {
        state.dialogs.push_message("Theme not saved", e.to_string());
    }
}

/// Applies the saved theme to the live tokens, fonts and icons.
pub fn apply_theme(ctx: &egui::Context, state: &mut AppState) {
    let mut tokens = Tokens::default_palette();
    tokens.wayland = state.tokens.wayland;
    tokens.reduce_motion = state.tokens.reduce_motion;
    crate::theme::apply(&mut tokens, &state.theme);
    state.tokens = tokens;
    crate::fonts::install(ctx, &state.tokens);
    crate::tokens::install_style(ctx, &state.tokens);
    state.icons.clear();
}

/// Restores every theme default and applies it.
pub fn reset_all(ctx: &egui::Context, state: &mut AppState) {
    state.theme.reset_all();
    save_theme(state);
    apply_theme(ctx, state);
}

/// `label_body(TEXT, 15)` / small style button with a 1 px border:
/// `Reset` (TEXT_DIM, hover ERROR) and `Import` (SURFACE_3, TEXT, hover ACCENT).
#[allow(clippy::too_many_arguments)]
fn small_button(
    ui: &mut Ui,
    t: &Tokens,
    text: &str,
    pad: Vec2,
    filled: bool,
    hover_error: bool,
    name: &str,
) -> bool {
    let g = galley(ui, text, size::LABEL, Weight::W600, Color32::WHITE);
    let w = (g.size().x + pad.x * 2.0 + 2.0).ceil();
    let h = (crate::fonts::qt_height(size::LABEL, Weight::W600) + pad.y * 2.0 + 2.0).round();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let blend = motion::hover(
        ui.ctx(),
        resp.id.with("hover"),
        resp.hovered(),
        t.reduce_motion,
    );
    let hover = if hover_error {
        Color::Error
    } else {
        Color::Accent
    };
    let (base_fg, bg) = if filled {
        (Color::Text, Some(t.color(Color::Surface3)))
    } else {
        (Color::TextDim, None)
    };
    let border = lerp(t.color(Color::Border), t.color(hover), blend);
    let fg = lerp(t.color(base_fg), t.color(hover), blend);
    let radius = CornerRadius::same(geom::RADIUS as u8);
    if let Some(bg) = bg {
        ui.painter().rect_filled(rect, radius, bg);
    }
    ui.painter()
        .rect_stroke(rect, radius, Stroke::new(1.0, border), StrokeKind::Inside);
    paint_text(ui, g, rect, Align::Center, 0.0, fg);
    fidelity::mark(ui, name, rect)
        .edge(Color::Border)
        .text(text, size::LABEL, Weight::W600)
        .done();
    resp.clicked()
}

/// The Customize page: action bar (fixed) above its own scroll area.
pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let rect = ui.max_rect();
    fidelity::mark(ui, "customizePage", rect).done();
    // Action bar, margins (0, 0, 12, 12), spacing 12: Export (secondary 43), Import (outline), stretch, Reset all, Apply (primary).
    let top = rect.min;
    let bar_w = rect.width() - 12.0;
    let widths: Vec<f32> = ["Export theme", "Import theme", "Reset all", "Apply theme"]
        .iter()
        .enumerate()
        .map(|(i, l)| {
            (galley(ui, l, size::LABEL, Weight::W600, Color32::WHITE)
                .size()
                .x
                + if i == 0 || i == 3 { 36.0 } else { 34.0 })
            .ceil()
                + if i == 0 { 2.0 } else { 0.0 }
        })
        .collect();
    let mut x = top.x;
    let mut ex = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(x, top.y),
        Vec2::new(widths[0], 43.0),
    )));
    if buttons::secondary(&mut ex, &t, "Export theme", "QPushButton[Export theme]").clicked() {
        state.theme_export_requested = true;
    }
    x += widths[0] + 12.0;
    let mut im = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(x, top.y + 1.0),
        Vec2::new(widths[1], 41.0),
    )));
    if buttons::outline(&mut im, &t, "Import theme", "QPushButton[Import theme]").clicked() {
        state.browse_requested = Some(BrowseKind::ThemeArchive);
    }
    let ax = top.x + bar_w - widths[3];
    let rx = ax - 12.0 - widths[2];
    let mut rs = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(rx, top.y + 1.0),
        Vec2::new(widths[2], 41.0),
    )));
    if buttons::outline(&mut rs, &t, "Reset all", "QPushButton[Reset all]").clicked() {
        let ctx = ui.ctx().clone();
        reset_all(&ctx, state);
    }
    let mut ap = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(ax, top.y + 1.0),
        Vec2::new(widths[3], 41.0),
    )));
    if buttons::primary(&mut ap, &t, "Apply theme", "QPushButton[Apply theme]").clicked() {
        let ctx = ui.ctx().clone();
        apply_theme(&ctx, state);
    }
    // Scroll area below: container margins (0, 0, 12, 32), spacing 12.
    let scroll_rect = Rect::from_min_max(Pos2::new(rect.min.x, top.y + 43.0 + 12.0), rect.max);
    fidelity::mark(ui, "customizeScroll", scroll_rect).done();
    let height_id = egui::Id::unique("customize-scroll-height");
    let last_h: f32 = ui
        .ctx()
        .data_mut(|d| d.get_temp(height_id))
        .unwrap_or(f32::INFINITY);
    let bar = if last_h > scroll_rect.height() {
        8.0
    } else {
        0.0
    };
    let container_w = scroll_rect.width() - bar;
    fidelity::mark(
        ui,
        "customizeContainer",
        Rect::from_min_size(
            scroll_rect.min,
            Vec2::new(container_w, scroll_rect.height()),
        ),
    )
    .done();
    let mut area = ui.new_child(egui::UiBuilder::new().max_rect(scroll_rect));
    crate::widgets::scroll::vertical(&area, &t, "customizeScroll")
        .max_height(scroll_rect.height())
        .show(&mut area, |ui| {
            ui.set_width(scroll_rect.width());
            let body = Rect::from_min_size(
                Pos2::new(scroll_rect.min.x, ui.cursor().min.y),
                Vec2::new(container_w - 12.0, f32::INFINITY),
            );
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(body));
            c.spacing_mut().item_spacing = Vec2::ZERO;
            let w = body.width();
            // Info bar: SURFACE_2, 1 px BORDER, r8, padding 12×16, 14 px TEXT_MUTED, wrapped.
            let line = crate::fonts::qt_wrapped_line_height(size::LABEL, Weight::W400);
            let g = c.painter().layout(
                INFO.to_owned(),
                crate::fonts::id(size::LABEL, Weight::W400),
                t.color(Color::TextMuted),
                w - 34.0,
            );
            let rows = g.rows.len().max(1) as f32;
            let ih = rows * line + 24.0 + 2.0;
            let (irect, _) = c.allocate_exact_size(Vec2::new(w, ih), Sense::hover());
            c.painter().rect_filled(
                irect,
                CornerRadius::same(geom::RADIUS_CARD as u8),
                t.color(Color::Surface2),
            );
            c.painter().rect_stroke(
                irect,
                CornerRadius::same(geom::RADIUS_CARD as u8),
                Stroke::new(1.0, t.color(Color::Border)),
                StrokeKind::Inside,
            );
            c.painter().galley(
                Pos2::new(
                    irect.min.x + 17.0,
                    irect.min.y + 13.0 + ((rows * line - g.size().y) / 2.0).floor().max(0.0),
                ),
                g,
                t.color(Color::TextMuted),
            );
            fidelity::mark(&c, format!("QLabel[{}]", &INFO[..40]), irect)
                .sample(
                    "fill",
                    Pos2::new(irect.min.x + 4.0, irect.center().y),
                    Color::Surface2,
                )
                .edge(Color::Border)
                .text(INFO, size::LABEL, Weight::W400)
                .done();
            space(&mut c, 12.0);
            for (i, (number, title)) in SECTIONS.iter().enumerate() {
                let name = if i == 0 {
                    "accordionSection".to_owned()
                } else {
                    format!("accordionSection#{}", i + 1)
                };
                let mut open = state.customize_open[i];
                // The header needs the icon cache while the body needs the whole state.
                let mut icons = std::mem::take(&mut state.icons);
                accordion::section(
                    &mut c,
                    &t,
                    &mut icons,
                    number,
                    title,
                    &mut open,
                    &name,
                    |ui| match i {
                        0 => typography(ui, state),
                        1 => colors(ui, state),
                        2 => icon_colors(ui, state),
                        3 => icons_section(ui, state),
                        4 => capture_card(ui, state),
                        _ => sounds(ui, state),
                    },
                );
                // Bodies may have replaced textures (Apply); keep the newer cache.
                if state.icons.is_empty() {
                    state.icons = icons;
                }
                state.customize_open[i] = open;
                space(&mut c, 12.0);
            }
            let bottom = c.cursor().min.y - 12.0 + 32.0;
            let content_h = bottom - body.min.y;
            if (content_h - last_h).abs() > 0.5 {
                ui.ctx().data_mut(|d| d.insert_temp(height_id, content_h));
                ui.ctx().request_repaint();
            }
            ui.allocate_rect(
                Rect::from_min_max(body.min, Pos2::new(scroll_rect.max.x - bar, bottom)),
                Sense::hover(),
            );
        });
    ui.allocate_rect(rect, Sense::hover());
}

fn typography(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    section::label_wrapped_fit(
        ui,
        &t,
        "Fonts: .ttf and .otf files are supported.",
        size::BODY,
        Weight::W400,
        Color::TextDim,
        w,
    );
    space(ui, 12.0);
    let mut families: Vec<String> = BUILTIN_FONT_FAMILIES
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut customs: Vec<String> = state
        .theme
        .custom_font_paths()
        .into_iter()
        .map(|(f, _)| f)
        .filter(|f| !families.contains(f))
        .collect();
    customs.sort_by_key(|s| s.to_lowercase());
    families.extend(customs);
    for (i, (role, title)) in [("display", "Heading font"), ("body", "Interface font")]
        .iter()
        .enumerate()
    {
        let top = ui.cursor().min;
        section::label_in(
            ui,
            &t,
            Rect::from_min_size(top, Vec2::new(140.0, 41.0)),
            title,
            size::LABEL,
            Weight::W600,
            Color::Text,
            None,
        );
        let import_w = (galley(ui, "Import font", size::LABEL, Weight::W600, Color32::WHITE)
            .size()
            .x
            + 34.0)
            .ceil();
        let combo_w = w - 140.0 - 12.0 - 12.0 - import_w;
        let current = state
            .theme
            .data
            .fonts
            .get(*role)
            .cloned()
            .filter(|f| families.contains(f))
            .unwrap_or_else(|| "Oswald".into());
        let sel = families.iter().position(|f| *f == current).unwrap_or(0);
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + 152.0, top.y + 1.0),
            Vec2::new(combo_w, 38.0),
        )));
        let mut combo = Combo::new(families.clone(), sel).with_width(combo_w);
        let name = if i == 0 {
            format!("WheelSafeComboBox[{}]", combo.current())
        } else {
            format!("WheelSafeComboBox[{}]#2", combo.current())
        };
        if combo.ui(&mut c, &t, &mut state.icons, &name).changed() {
            state
                .theme
                .data
                .fonts
                .insert((*role).to_owned(), families[combo.selected].clone());
            save_theme(state);
        }
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + w - import_w, top.y),
            Vec2::new(import_w, 41.0),
        )));
        let bname = if i == 0 {
            "QPushButton[Import font]".to_owned()
        } else {
            "QPushButton[Import font]#2".to_owned()
        };
        if buttons::outline(&mut b, &t, "Import font", &bname).clicked() {
            state.browse_requested = Some(BrowseKind::FontFile(role.to_string()));
        }
        ui.allocate_exact_size(Vec2::new(w, 41.0), Sense::hover());
        space(ui, 12.0);
    }
    // Preview: SURFACE_2, 1 px BORDER, padding 10, centred 17 px text, min 52.
    let top = ui.cursor().min;
    let prect = Rect::from_min_size(top, Vec2::new(w, 52.0));
    ui.painter()
        .rect_filled(prect, 0.0, t.color(Color::Surface2));
    ui.painter().rect_stroke(
        prect,
        0.0,
        Stroke::new(1.0, t.color(Color::Border)),
        StrokeKind::Inside,
    );
    let text = "Your privacy isn't currency.";
    let g = galley(ui, text, size::BODY_L, Weight::W400, t.color(Color::Text));
    paint_text(ui, g, prect, Align::Center, 0.0, t.color(Color::Text));
    fidelity::mark(ui, format!("QLabel[{text}]"), prect)
        .sample(
            "fill",
            Pos2::new(prect.min.x + 4.0, prect.center().y),
            Color::Surface2,
        )
        .text(text, size::BODY_L, Weight::W400)
        .done();
    ui.allocate_exact_size(Vec2::new(w, 52.0), Sense::hover());
    space(ui, 12.0);
    let top = ui.cursor().min;
    let rw = (galley(
        ui,
        "Reset typography",
        size::LABEL,
        Weight::W600,
        Color32::WHITE,
    )
    .size()
    .x + 34.0)
        .ceil();
    let mut r = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + w - rw, top.y),
        Vec2::new(rw, 41.0),
    )));
    if buttons::outline(
        &mut r,
        &t,
        "Reset typography",
        "QPushButton[Reset typography]",
    )
    .clicked()
    {
        state.theme.data.fonts = fthrust_core::theme::default_fonts();
        save_theme(state);
    }
    ui.allocate_exact_size(Vec2::new(w, 41.0), Sense::hover());
}

fn color_label(token: &str) -> &str {
    COLOR_LABELS
        .iter()
        .find(|(k, _)| *k == token)
        .map(|(_, l)| *l)
        .unwrap_or(token)
}

fn swatch_grid(
    ui: &mut Ui,
    state: &mut AppState,
    tokens: &[(String, String, String)],
    cols: usize,
    x0: f32,
) -> Option<(String, String)> {
    let t = state.tokens.clone();
    let mut changed = None;
    let rows = tokens.len().div_ceil(cols);
    let top = ui.cursor().min;
    for (i, (token, label, hex)) in tokens.iter().enumerate() {
        let (r, c) = (i / cols, i % cols);
        let pos = Pos2::new(
            x0 + c as f32 * (96.0 + 8.0),
            top.y + r as f32 * (52.0 + 8.0),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(pos, crate::widgets::color::SIZE)),
        );
        if let Some(h) = swatch(
            &mut child,
            &t,
            token,
            label,
            hex,
            &format!("colorSwatch[{token}]"),
        ) {
            changed = Some((token.clone(), h));
        }
    }
    let h = rows as f32 * 52.0 + (rows.saturating_sub(1)) as f32 * 8.0;
    ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    changed
}

fn colors(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    color_preview(ui, state, w);
    space(ui, 12.0);
    space(ui, 8.0);
    let mut change: Option<(String, String)> = None;
    for (group, tokens) in COLOR_GROUPS {
        let top = ui.cursor().min;
        let lrect = Rect::from_min_size(top, Vec2::new(90.0, 33.0));
        let g = galley(ui, group, size::BODY, Weight::W600, t.color(Color::Accent));
        ui.painter().galley(
            Pos2::new((lrect.max.x - g.size().x).round(), top.y + 4.0 + 3.0),
            g,
            t.color(Color::Accent),
        );
        fidelity::mark(ui, format!("QLabel[{group}]"), lrect)
            .text(*group, size::BODY, Weight::W600)
            .done();
        let items: Vec<(String, String, String)> = tokens
            .iter()
            .map(|tk| {
                (
                    tk.to_string(),
                    color_label(tk).to_owned(),
                    state.theme.color(tk).to_owned(),
                )
            })
            .collect();
        let mut grid = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + 90.0 + 16.0, top.y),
            Vec2::new(w - 106.0, f32::INFINITY),
        )));
        if let Some(c) = swatch_grid(&mut grid, state, &items, 6, top.x + 106.0) {
            change = Some(c);
        }
        let rows = tokens.len().div_ceil(6) as f32;
        let gh = rows * 52.0 + (rows - 1.0) * 8.0;
        ui.allocate_exact_size(Vec2::new(w, gh.max(33.0)), Sense::hover());
        space(ui, 12.0);
    }
    if let Some((token, hex)) = change {
        set_draft_color(state, &token, &hex);
    }
    let top = ui.cursor().min;
    let g = galley(
        ui,
        "Reset all colors",
        size::LABEL,
        Weight::W600,
        Color32::WHITE,
    );
    let rw = (g.size().x + 28.0 + 2.0).ceil();
    let mut r = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + w - rw, top.y),
        Vec2::new(rw, 35.0),
    )));
    if small_button(
        &mut r,
        &t,
        "Reset all colors",
        Vec2::new(14.0, 6.0),
        false,
        true,
        "QPushButton[Reset all colors]",
    ) {
        state.theme.data.colors = default_colors();
        save_theme(state);
    }
    ui.allocate_exact_size(Vec2::new(w, 35.0), Sense::hover());
}

/// `_ColorPreviewMockup`: a 220 px miniature of the app drawn from the
/// draft colours (BG frame r8; 26 px SHELL_BG top bar; 20 px SHELL_BG_2
/// status strip; cards, buttons, text samples and status chips).
fn color_preview(ui: &mut Ui, state: &mut AppState, w: f32) {
    let c =
        |tok: &str| crate::tokens::parse_hex(state.theme.color(tok)).unwrap_or(Color32::MAGENTA);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 220.0), Sense::hover());
    let r8 = CornerRadius::same(geom::RADIUS_CARD as u8);
    ui.painter().rect_filled(rect, r8, c("BG"));
    fidelity::mark(ui, "colorPreview", rect).done();
    let tb = Rect::from_min_size(rect.min, Vec2::new(w, 26.0));
    ui.painter().rect_filled(
        tb,
        CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        },
        c("SHELL_BG"),
    );
    crate::widgets::rule_h(ui, tb.min.x, tb.max.x, tb.max.y - 1.0, c("SHELL_DIVIDER"));
    // Qt lays these 7–10 px labels out with hinted integer glyph advances,
    // so their widths cannot be reproduced from the unhinted metrics; the
    // markers are named apart from Qt's and the mockup is checked visually.
    let label = |ui: &mut Ui,
                 text: &str,
                 px: f32,
                 weight: Weight,
                 color: Color32,
                 rect: Rect,
                 align: Align| {
        let g = galley(ui, text, px, weight, color);
        paint_text(ui, g, rect, align, 0.0, color);
        fidelity::mark(ui, format!("mockup[{text}]"), rect)
            .text(text, px, weight)
            .done();
    };
    let fthr_w = galley(ui, "FTHR", 10.0, Weight::W600, Color32::WHITE)
        .size()
        .x
        .ceil();
    label(
        ui,
        "FTHR",
        10.0,
        Weight::W600,
        c("TEXT"),
        Rect::from_min_size(
            Pos2::new(tb.min.x + 11.0, tb.min.y + 1.0),
            Vec2::new(fthr_w, 25.0),
        ),
        Align::Min,
    );
    let cap_w = galley(ui, "Capturing", 8.0, Weight::W600, Color32::WHITE)
        .size()
        .x
        .ceil()
        + 4.0;
    label(
        ui,
        "Capturing",
        8.0,
        Weight::W600,
        c("ACCENT"),
        Rect::from_min_size(
            Pos2::new(tb.max.x - 11.0 - cap_w, tb.min.y + 1.0),
            Vec2::new(cap_w, 25.0),
        ),
        Align::Center,
    );
    let sb = Rect::from_min_size(Pos2::new(rect.min.x, tb.max.y), Vec2::new(w, 20.0));
    ui.painter().rect_filled(sb, 0.0, c("SHELL_BG_2"));
    crate::widgets::rule_h(ui, sb.min.x, sb.max.x, sb.max.y - 1.0, c("SHELL_DIVIDER"));
    let ready_w = galley(ui, "Ready", 8.0, Weight::W600, Color32::WHITE)
        .size()
        .x
        .ceil();
    label(
        ui,
        "Ready",
        8.0,
        Weight::W600,
        c("SUCCESS"),
        Rect::from_min_size(
            Pos2::new(sb.min.x + 11.0, sb.min.y + 1.0),
            Vec2::new(ready_w, 20.0),
        ),
        Align::Min,
    );
    let strip_w = galley(ui, "Status strip", 7.0, Weight::W400, Color32::WHITE)
        .size()
        .x
        .ceil();
    label(
        ui,
        "Status strip",
        7.0,
        Weight::W400,
        c("TEXT_MUTED"),
        Rect::from_min_size(
            Pos2::new(sb.max.x - 11.0 - strip_w, sb.min.y + 1.0),
            Vec2::new(strip_w, 20.0),
        ),
        Align::Min,
    );
    // Content: margins (10, 8, 10, 8), spacing 8.
    let content = Rect::from_min_max(
        Pos2::new(rect.min.x + 11.0, sb.max.y + 8.0),
        Pos2::new(rect.max.x - 11.0, rect.max.y - 8.0),
    );
    let mut y = content.min.y;
    let mut x = content.min.x;
    for i in 0..3 {
        let card = Rect::from_min_size(Pos2::new(x, y), Vec2::new(80.0, 48.0));
        ui.painter().rect_filled(
            card,
            CornerRadius::same(geom::RADIUS_SM as u8),
            c("CARD_BG"),
        );
        ui.painter().rect_stroke(
            card,
            CornerRadius::same(geom::RADIUS_SM as u8),
            Stroke::new(1.0, c("CARD_BORDER")),
            StrokeKind::Inside,
        );
        ui.painter().rect_filled(
            Rect::from_min_size(card.min + Vec2::new(7.0, 5.0), Vec2::new(66.0, 22.0)),
            0.0,
            c("SURFACE_2"),
        );
        let title = format!("clip_{}.mp4", i + 1);
        label(
            ui,
            &title,
            7.0,
            Weight::W400,
            c("TEXT_DIM"),
            Rect::from_min_size(card.min + Vec2::new(7.0, 31.0), Vec2::new(66.0, 13.0)),
            Align::Min,
        );
        x += 88.0;
    }
    y += 48.0 + 7.0;
    let mut x = content.min.x;
    for (text, filled) in [("Save clip", true), ("Export", false)] {
        let tw = galley(ui, text, 8.0, Weight::W600, Color32::WHITE)
            .size()
            .x
            .ceil();
        let brect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(tw + 24.0, 22.0));
        if filled {
            ui.painter().rect_filled(brect, 0.0, c("ACCENT"));
        } else {
            ui.painter().rect_stroke(
                brect,
                0.0,
                Stroke::new(1.0, c("BORDER_HI")),
                StrokeKind::Inside,
            );
        }
        label(
            ui,
            text,
            8.0,
            Weight::W600,
            if filled { c("BG") } else { c("TEXT") },
            brect,
            Align::Center,
        );
        x += brect.width() + 8.0;
    }
    y += 22.0 + 8.0;
    for (text, tok) in [
        ("Primary text sample", "TEXT"),
        ("Secondary text sample", "TEXT_DIM"),
        ("Muted text sample", "TEXT_MUTED"),
        ("Ghost text sample", "TEXT_GHOST"),
    ] {
        label(
            ui,
            text,
            9.0,
            Weight::W400,
            c(tok),
            Rect::from_min_size(
                Pos2::new(content.min.x, y),
                Vec2::new(content.width(), 10.0),
            ),
            Align::Min,
        );
        y += 12.0;
    }
    y += 6.0;
    let mut x = content.min.x;
    for (text, tok) in [
        ("Error", "ERROR"),
        ("Warning", "WARNING"),
        ("Success", "SUCCESS"),
        ("Delete", "DELETE"),
    ] {
        let tw = galley(ui, text, 8.0, Weight::W600, Color32::WHITE)
            .size()
            .x
            .ceil();
        let chip = Rect::from_min_size(Pos2::new(x, y), Vec2::new(tw + 18.0, 18.0));
        ui.painter()
            .rect_stroke(chip, 0.0, Stroke::new(1.0, c(tok)), StrokeKind::Inside);
        label(ui, text, 8.0, Weight::W600, c(tok), chip, Align::Center);
        x += chip.width() + 8.0;
    }
}

fn icon_label(logical: &str) -> &str {
    CUSTOMIZABLE_ICONS
        .iter()
        .find(|(k, _)| *k == logical)
        .map(|(_, l)| *l)
        .unwrap_or(logical)
}

fn icon_for(logical: &str) -> Option<Icon> {
    Icon::ALL
        .iter()
        .copied()
        .find(|i| i.logical_name() == logical)
}

fn icon_box(
    ui: &mut Ui,
    t: &Tokens,
    icons: &mut IconCache,
    rect: Rect,
    icon: Option<Icon>,
    tint_override: Option<Color32>,
) {
    ui.painter()
        .rect_filled(rect, 0.0, t.color(Color::Surface2));
    ui.painter().rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, t.color(Color::Border)),
        StrokeKind::Inside,
    );
    if let Some(icon) = icon {
        let tex = if let Some(c) = tint_override {
            let mut tt = t.clone();
            tt.icon_tints.insert(icon.logical_name().to_owned(), c);
            icons.texture(ui.ctx(), icon, 24.0, &tt)
        } else {
            icons.texture(ui.ctx(), icon, 24.0, t)
        };
        ui.painter().image(
            tex.id,
            Rect::from_center_size(rect.center(), Vec2::splat(24.0)),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

fn icon_colors(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    // Global tint row.
    let top = ui.cursor().min;
    section::label_in(
        ui,
        &t,
        Rect::from_min_size(top, Vec2::new(120.0, 24.0)),
        "Global tint",
        size::BODY,
        Weight::W600,
        Color::Accent,
        None,
    );
    let global = state.theme.icon_tint("_global").to_owned();
    let mut sw = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + 132.0, top.y),
        Vec2::new(40.0, 24.0),
    )));
    if let Some(h) = mini_swatch(
        &mut sw,
        &t,
        Vec2::new(40.0, 24.0),
        &global,
        "globalTintSwatch",
    ) {
        state.theme.data.icon_tints.insert("_global".into(), h);
        save_theme(state);
    }
    let rw = (galley(ui, "Reset all", size::LABEL, Weight::W600, Color32::WHITE)
        .size()
        .x
        + 20.0
        + 2.0)
        .ceil();
    let mut r = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + w - rw, top.y),
        Vec2::new(rw, 31.0),
    )));
    if small_button(
        &mut r,
        &t,
        "Reset all",
        Vec2::new(10.0, 4.0),
        false,
        true,
        "QPushButton[Reset all]#2",
    ) {
        state.theme.data.icon_tints.clear();
        save_theme(state);
    }
    ui.allocate_exact_size(Vec2::new(w, 31.0), Sense::hover());
    space(ui, 4.0);
    space(ui, 4.0);
    for (logical, _) in CUSTOMIZABLE_ICONS {
        let top = ui.cursor().min;
        let row = Rect::from_min_size(top, Vec2::new(w, 48.0));
        let tint = state.theme.icon_tint(logical).to_owned();
        icon_box(
            ui,
            &t,
            &mut state.icons,
            Rect::from_min_size(Pos2::new(top.x, top.y + 8.0), Vec2::splat(32.0)),
            icon_for(logical),
            crate::tokens::parse_hex(&tint),
        );
        section::label_in(
            ui,
            &t,
            Rect::from_min_size(Pos2::new(top.x + 44.0, top.y + 4.0), Vec2::new(120.0, 40.0)),
            icon_label(logical),
            size::BODY,
            Weight::W400,
            Color::Text,
            Some(&format!("iconTintRow[{logical}]")),
        );
        let rw = (galley(ui, "Reset", size::LABEL, Weight::W600, Color32::WHITE)
            .size()
            .x
            + 20.0
            + 2.0)
            .ceil();
        let mut sw = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + w - rw - 12.0 - 32.0, top.y + 14.0),
            Vec2::new(32.0, 20.0),
        )));
        if let Some(h) = mini_swatch(
            &mut sw,
            &t,
            Vec2::new(32.0, 20.0),
            &tint,
            &format!("tintSwatch[{logical}]"),
        ) {
            state.theme.data.icon_tints.insert(logical.to_string(), h);
            save_theme(state);
        }
        let mut r = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + w - rw, top.y + 9.0),
            Vec2::new(rw, 31.0),
        )));
        if small_button(
            &mut r,
            &t,
            "Reset",
            Vec2::new(10.0, 4.0),
            false,
            true,
            &format!("tintReset[{logical}]"),
        ) {
            state.theme.data.icon_tints.remove(*logical);
            save_theme(state);
        }
        ui.allocate_exact_size(row.size(), Sense::hover());
        space(ui, 4.0);
    }
}

fn icons_section(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    for (logical, _) in CUSTOMIZABLE_ICONS {
        let top = ui.cursor().min;
        section::label_in(
            ui,
            &t,
            Rect::from_min_size(Pos2::new(top.x, top.y + 4.0), Vec2::new(120.0, 44.0)),
            icon_label(logical),
            size::BODY,
            Weight::W400,
            Color::Text,
            Some(&format!("iconRow[{logical}]")),
        );
        let mut x = top.x + 132.0;
        let dw = galley(ui, "Default:", size::LABEL, Weight::W400, Color32::WHITE)
            .size()
            .x
            .ceil();
        section::label_in(
            ui,
            &t,
            Rect::from_min_size(Pos2::new(x, top.y + 4.0), Vec2::new(dw, 44.0)),
            "Default:",
            size::LABEL,
            Weight::W400,
            Color::TextMuted,
            Some(&format!("iconDefault[{logical}]")),
        );
        x += dw + 12.0;
        icon_box(
            ui,
            &t,
            &mut state.icons,
            Rect::from_min_size(Pos2::new(x, top.y + 10.0), Vec2::splat(32.0)),
            icon_for(logical),
            None,
        );
        x += 32.0 + 12.0;
        let cw = galley(ui, "Current:", size::LABEL, Weight::W400, Color32::WHITE)
            .size()
            .x
            .ceil();
        section::label_in(
            ui,
            &t,
            Rect::from_min_size(Pos2::new(x, top.y + 4.0), Vec2::new(cw, 44.0)),
            "Current:",
            size::LABEL,
            Weight::W400,
            Color::TextMuted,
            Some(&format!("iconCurrent[{logical}]")),
        );
        x += cw + 12.0;
        icon_box(
            ui,
            &t,
            &mut state.icons,
            Rect::from_min_size(Pos2::new(x, top.y + 10.0), Vec2::splat(32.0)),
            icon_for(logical),
            None,
        );
        let rw = (galley(ui, "Reset", size::LABEL, Weight::W600, Color32::WHITE)
            .size()
            .x
            + 20.0
            + 2.0)
            .ceil();
        let iw = (galley(ui, "Import", size::LABEL, Weight::W600, Color32::WHITE)
            .size()
            .x
            + 24.0
            + 2.0)
            .ceil();
        let mut ib = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + w - rw - 12.0 - iw, top.y + 10.0),
            Vec2::new(iw, 31.0),
        )));
        if small_button(
            &mut ib,
            &t,
            "Import",
            Vec2::new(12.0, 4.0),
            true,
            false,
            &format!("iconImport[{logical}]"),
        ) {
            state.browse_requested = Some(BrowseKind::IconFile(logical.to_string()));
        }
        let mut rb = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + w - rw, top.y + 10.0),
            Vec2::new(rw, 31.0),
        )));
        if small_button(
            &mut rb,
            &t,
            "Reset",
            Vec2::new(10.0, 4.0),
            false,
            true,
            &format!("iconReset[{logical}]"),
        ) {
            state.theme.data.icons.remove(*logical);
            save_theme(state);
        }
        ui.allocate_exact_size(Vec2::new(w, 52.0), Sense::hover());
        space(ui, 4.0);
    }
}

fn capture_card(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    let items: Vec<(String, String, String)> = CC_TOKENS
        .iter()
        .map(|(k, l)| {
            (
                k.to_string(),
                l.to_string(),
                state.theme.capture_card_color(k).to_owned(),
            )
        })
        .collect();
    let top = ui.cursor().min;
    if let Some((token, hex)) = swatch_grid(ui, state, &items, 4, top.x) {
        state.theme.data.capture_card.insert(token, hex);
        save_theme(state);
    }
    space(ui, 12.0);
    let top = ui.cursor().min;
    let rw = (galley(
        ui,
        "Reset capture card",
        size::LABEL,
        Weight::W600,
        Color32::WHITE,
    )
    .size()
    .x + 28.0
        + 2.0)
        .ceil();
    let mut r = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(top.x + w - rw, top.y),
        Vec2::new(rw, 35.0),
    )));
    if small_button(
        &mut r,
        &t,
        "Reset capture card",
        Vec2::new(14.0, 6.0),
        false,
        true,
        "QPushButton[Reset capture card]",
    ) {
        state.theme.data.capture_card = default_capture_card_colors();
        save_theme(state);
    }
    ui.allocate_exact_size(Vec2::new(w, 35.0), Sense::hover());
}

fn sounds(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    section::label_wrapped_fit(
        ui,
        &t,
        "Any format the system can play is supported.",
        size::BODY,
        Weight::W400,
        Color::TextDim,
        w,
    );
    space(ui, 4.0);
    space(ui, 8.0);
    for (key, label) in CUSTOMIZABLE_SOUND_LABELS {
        let top = ui.cursor().min;
        section::label_in(
            ui,
            &t,
            Rect::from_min_size(Pos2::new(top.x, top.y + 4.0), Vec2::new(150.0, 44.0)),
            label,
            size::BODY,
            Weight::W400,
            Color::Text,
            Some(&format!("soundRow[{key}]")),
        );
        let file = state
            .theme
            .custom_sound_path(key)
            .filter(|p| p.exists())
            .map(|p| {
                let kb = std::fs::metadata(&p)
                    .map(|m| m.len() as f64 / 1024.0)
                    .unwrap_or(0.0);
                format!(
                    "{} ({kb:.0} KB)",
                    p.file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )
            })
            .unwrap_or_else(|| "(default)".into());
        section::label_in(
            ui,
            &t,
            Rect::from_min_size(
                Pos2::new(top.x + 162.0, top.y + 4.0),
                Vec2::new(150.0, 44.0),
            ),
            &file,
            size::LABEL,
            Weight::W400,
            Color::TextDim,
            Some(&format!("soundFile[{key}]")),
        );
        let vx = top.x + 324.0;
        let vw = galley(ui, "Volume", size::MICRO, Weight::W600, Color32::WHITE)
            .size()
            .x
            .ceil();
        section::label_in(
            ui,
            &t,
            Rect::from_min_size(Pos2::new(vx, top.y + 4.0), Vec2::new(vw, 44.0)),
            "Volume",
            size::MICRO,
            Weight::W600,
            Color::TextMuted,
            Some(&format!("soundVolumeLabel[{key}]")),
        );
        let setting_key = sound_setting_key(key);
        let mut v = state.settings.get_i64(&setting_key).unwrap_or(100) as f32;
        let mut sl = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(vx + vw + 12.0, top.y + 13.0),
            Vec2::new(120.0, 26.0),
        )));
        if slider::settings_slider(
            &mut sl,
            &t,
            &mut v,
            0.0..=100.0,
            &format!("soundVolume[{key}]"),
        )
        .changed()
        {
            state.settings.set(&setting_key, json!(v as i64));
            let _ = state.settings.save();
        }
        let text = format!("{}%", v as i64);
        let g = galley(
            ui,
            &text,
            size::LABEL,
            Weight::W400,
            t.color(Color::TextDim),
        );
        paint_text(
            ui,
            g,
            Rect::from_min_size(
                Pos2::new(vx + vw + 12.0 + 120.0 + 12.0, top.y + 4.0),
                Vec2::new(34.0, 44.0),
            ),
            Align::Max,
            0.0,
            t.color(Color::TextDim),
        );
        let rw = (galley(ui, "Reset", size::LABEL, Weight::W600, Color32::WHITE)
            .size()
            .x
            + 20.0
            + 2.0)
            .ceil();
        let iw = (galley(ui, "Import", size::LABEL, Weight::W600, Color32::WHITE)
            .size()
            .x
            + 24.0
            + 2.0)
            .ceil();
        let pw = (galley(ui, "preview", size::LABEL, Weight::W600, Color32::WHITE)
            .size()
            .x
            + 20.0
            + 2.0
            + 18.0)
            .ceil();
        let mut px = top.x + w - rw - 12.0 - iw - 12.0 - pw;
        let mut pb = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(px, top.y + 10.0),
            Vec2::new(pw, 31.0),
        )));
        let opts = Opts {
            icon: Some((Icon::Play, Vec2::splat(14.0))),
            text_color: Some(Color::Accent),
            border_color: Some(Color::Border),
            pad: Some(Vec2::new(10.0, 4.0)),
            ..Opts::default()
        };
        if buttons::button_with(
            &mut pb,
            &t,
            Some(&mut state.icons),
            Kind::Outline,
            "preview",
            &format!("soundPreview[{key}]"),
            &opts,
        )
        .clicked()
        {
            state.sound_preview_requested = Some(key.to_string());
        }
        px += pw + 12.0;
        let mut ib = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(px, top.y + 10.0),
            Vec2::new(iw, 31.0),
        )));
        if small_button(
            &mut ib,
            &t,
            "Import",
            Vec2::new(12.0, 4.0),
            true,
            false,
            &format!("soundImport[{key}]"),
        ) {
            state.browse_requested = Some(BrowseKind::SoundFile(key.to_string()));
        }
        let mut rb = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            Pos2::new(top.x + w - rw, top.y + 10.0),
            Vec2::new(rw, 31.0),
        )));
        if small_button(
            &mut rb,
            &t,
            "Reset",
            Vec2::new(10.0, 4.0),
            false,
            true,
            &format!("soundReset[{key}]"),
        ) {
            state.theme.data.sounds.remove(*key);
            state.settings.set(&setting_key, json!(100));
            let _ = state.settings.save();
            save_theme(state);
        }
        ui.allocate_exact_size(Vec2::new(w, 52.0), Sense::hover());
        space(ui, 4.0);
    }
}

/// `sound_volume_<key>` with the reference's key aliases.
pub fn sound_setting_key(theme_key: &str) -> String {
    match theme_key {
        "clip_captured" => "sound_volume_clip".into(),
        "screenshot_captured" => "sound_volume_screenshot".into(),
        other => format!("sound_volume_{other}"),
    }
}
