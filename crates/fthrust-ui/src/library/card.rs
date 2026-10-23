//! The clip card (`ClipThumbnail`): a 16:9 thumbnail with badges and the
//! hover play icon above an 88 px body with the title (inline rename),
//! metadata, share and delete buttons.

use std::path::PathBuf;

use egui::epaint::{Mesh, RectShape, Vertex, WHITE_UV};
use egui::{Align, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::layout::{CARD_BODY_H, CARD_MAX_W, CARD_MIN_W};
use crate::fidelity;
use crate::fonts::{self, Weight};
use crate::icons::{Icon, IconCache};
use crate::motion;
use crate::tokens::{Color, Tokens, geom, size};
use crate::widgets::{galley, paint_text};

/// What a card shows.
#[derive(Clone, Debug, PartialEq)]
pub struct CardData {
    /// The file.
    pub path: PathBuf,
    /// Title (the stem).
    pub title: String,
    /// `1080p · 60 FPS · 12 Mbps`, `Reading clip details…` or `Screenshot`.
    pub metadata: String,
    /// Duration for the badge (`None` → `--:--`).
    pub duration_s: Option<f64>,
    /// Video (else screenshot).
    pub is_video: bool,
    /// From an imported folder.
    pub imported: bool,
    /// Uploaded (badge).
    pub uploaded: bool,
    /// Link to copy instead of uploading.
    pub upload_link: Option<String>,
    /// The uploader is on.
    pub upload_enabled: bool,
    /// Accessible (else `Finalizing`).
    pub ready: bool,
    /// Transient feedback text.
    pub feedback: Option<String>,
    /// The thumbnail texture and its pixel size.
    pub thumbnail: Option<(egui::TextureId, Vec2)>,
}

/// What the user did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CardResponse {
    /// The thumbnail was clicked (open).
    pub opened: bool,
    /// Inline rename committed with this stem.
    pub rename_committed: Option<String>,
    /// Delete pressed.
    pub delete_requested: bool,
    /// Share / copy-link pressed.
    pub share_clicked: bool,
    /// Pointer over the card.
    pub hovered: bool,
    /// Right-clicked (context menu).
    pub context_menu: bool,
}

/// `m:ss` for the duration badge, `--:--` when unknown.
pub fn duration_label(duration_s: Option<f64>) -> String {
    match duration_s {
        Some(d) if d > 0.0 => {
            let s = (d + 1e-6) as u64;
            format!("{}:{:02}", s / 60, s % 60)
        }
        Some(_) => "0:00".into(),
        None => "--:--".into(),
    }
}

/// `QFontMetrics::elidedText` (right) for a font.
fn elide(ui: &Ui, text: &str, px: f32, weight: Weight, max_w: f32) -> String {
    if galley(ui, text, px, weight, Color32::WHITE).size().x <= max_w {
        return text.to_owned();
    }
    let mut s: String = text.to_owned();
    while !s.is_empty() {
        s.pop();
        let candidate = format!("{s}…");
        if galley(ui, &candidate, px, weight, Color32::WHITE).size().x <= max_w {
            return candidate;
        }
    }
    "…".into()
}

/// Width of a styled QLabel badge: advance + padding + Qt's 6 px.
fn badge_size(ui: &Ui, text: &str, px: f32, weight: Weight, pad: Vec2, border: f32) -> Vec2 {
    let g = galley(ui, text, px, weight, Color32::WHITE);
    Vec2::new(
        g.size().x.round() + pad.x * 2.0 + 6.0 + border * 2.0,
        (fonts::qt_height(px, weight) + pad.y * 2.0 + border * 2.0).round(),
    )
}

#[allow(clippy::too_many_arguments)]
fn badge(
    ui: &mut Ui,
    t: &Tokens,
    rect: Rect,
    text: &str,
    px: f32,
    weight: Weight,
    bg: Color32,
    fg: Color,
    border: Option<Color>,
    name: &str,
) {
    let radius = CornerRadius::same(geom::RADIUS_SM as u8);
    ui.painter().rect_filled(rect, radius, bg);
    if let Some(b) = border {
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, t.color(b)),
            StrokeKind::Inside,
        );
    }
    let g = galley(ui, text, px, weight, t.color(fg));
    paint_text(ui, g, rect, Align::Center, 0.0, t.color(fg));
    let mut m = fidelity::mark(ui, name, rect).text(text, px, weight);
    if let Some(b) = border {
        m = m.edge(b).top(b);
    }
    m.done();
}

#[allow(clippy::too_many_arguments)]
fn icon_button(
    ui: &mut Ui,
    t: &Tokens,
    icons: &mut IconCache,
    rect: Rect,
    icon: Icon,
    bg: Color,
    hover_bg: Color,
    border: Color,
    hover_border: Color,
    tint: Color,
    hover_tint: Color,
    enabled: bool,
    name: &str,
    card_name: &str,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(rect, ui.scope_id().with((card_name, name)), sense);
    let blend = motion::hover(
        ui.ctx(),
        resp.id.with("hover"),
        enabled && resp.hovered(),
        t.reduce_motion,
    );
    let radius = CornerRadius::same(4);
    let (fill, stroke, icon_tint) = if enabled {
        (
            crate::widgets::lerp(t.color(bg), t.color(hover_bg), blend),
            crate::widgets::lerp(t.color(border), t.color(hover_border), blend),
            crate::widgets::lerp(t.color(tint), t.color(hover_tint), blend),
        )
    } else {
        (
            t.color(Color::Surface3),
            t.color(Color::Border),
            t.color(Color::TextMuted),
        )
    };
    ui.painter().rect_filled(rect, radius, fill);
    ui.painter()
        .rect_stroke(rect, radius, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let mut tt = t.clone();
    tt.icon_tints
        .insert(icon.logical_name().to_owned(), icon_tint);
    let tex = icons.texture(ui.ctx(), icon, 28.0, &tt);
    ui.painter().image(
        tex.id,
        Rect::from_center_size(rect.center(), Vec2::splat(28.0)),
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
    fidelity::mark(ui, name, rect)
        .edge(border)
        .top(border)
        .done();
    resp
}

/// Draws one card of `width` at the cursor; `editing` holds the rename
/// draft while the title is being edited.
pub fn card(
    ui: &mut Ui,
    t: &Tokens,
    icons: &mut IconCache,
    width: f32,
    d: &CardData,
    editing: &mut Option<String>,
    name: &str,
) -> CardResponse {
    let width = width.clamp(CARD_MIN_W, CARD_MAX_W).round();
    let thumb_h = (width * 9.0 / 16.0).round();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, thumb_h + CARD_BODY_H), Sense::hover());
    let mut out = CardResponse::default();
    let card_bg = t.color(Color::CardBg);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(geom::RADIUS_CARD as u8), card_bg);
    fidelity::mark(ui, name, rect).done();
    // Thumbnail.
    let thumb = Rect::from_min_size(rect.min, Vec2::new(width, thumb_h));
    let top_round = CornerRadius {
        nw: geom::RADIUS_CARD as u8,
        ne: geom::RADIUS_CARD as u8,
        sw: 0,
        se: 0,
    };
    let thumb_resp = ui.interact(thumb, ui.scope_id().with((name, "thumb")), Sense::click());
    if let Some((tex, size_px)) = d.thumbnail {
        // KeepAspectRatioByExpanding + centred crop as uv.
        let scale = (width / size_px.x).max(thumb_h / size_px.y);
        let shown = size_px * scale;
        let u = ((shown.x - width) / 2.0 / shown.x).clamp(0.0, 0.5);
        let v = ((shown.y - thumb_h) / 2.0 / shown.y).clamp(0.0, 0.5);
        let uv = Rect::from_min_max(Pos2::new(u, v), Pos2::new(1.0 - u, 1.0 - v));
        ui.painter()
            .add(RectShape::filled(thumb, top_round, Color32::WHITE).with_texture(tex, uv));
        // Fade to CARD_BG over the bottom 36 %.
        let fade_top = thumb.min.y + thumb_h * 0.64;
        let mut mesh = Mesh::default();
        let clear = Color32::from_rgba_unmultiplied(card_bg.r(), card_bg.g(), card_bg.b(), 0);
        mesh.vertices.extend([
            Vertex {
                pos: Pos2::new(thumb.min.x, fade_top),
                uv: WHITE_UV,
                color: clear,
            },
            Vertex {
                pos: Pos2::new(thumb.max.x, fade_top),
                uv: WHITE_UV,
                color: clear,
            },
            Vertex {
                pos: Pos2::new(thumb.max.x, thumb.max.y),
                uv: WHITE_UV,
                color: card_bg,
            },
            Vertex {
                pos: Pos2::new(thumb.min.x, thumb.max.y),
                uv: WHITE_UV,
                color: card_bg,
            },
        ]);
        mesh.indices.extend([0, 1, 2, 0, 2, 3]);
        ui.painter().add(mesh);
    }
    fidelity::mark(ui, "cardThumb", thumb).done();
    let busy = !d.ready;
    out.hovered = ui.rect_contains_pointer(rect);
    if d.is_video && thumb_resp.hovered() && !busy {
        let tex = icons.texture(ui.ctx(), Icon::Play, 36.0, t);
        ui.painter().image(
            tex.id,
            Rect::from_center_size(thumb.center(), Vec2::splat(36.0)),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    if thumb_resp.clicked() && !busy {
        out.opened = true;
    }
    if thumb_resp.secondary_clicked() && !busy {
        out.context_menu = true;
    }
    // Duration badge (videos): bottom-right, 10 from the right, 8 from the bottom.
    if d.is_video {
        let text = duration_label(d.duration_s);
        let sz = badge_size(
            ui,
            &text,
            size::LABEL,
            Weight::W400,
            Vec2::new(9.0, 2.0),
            0.0,
        );
        let brect = Rect::from_min_size(
            Pos2::new(thumb.max.x - sz.x - 10.0, thumb.max.y - sz.y - 8.0),
            sz,
        );
        let s1 = t.color(Color::Surface1);
        badge(
            ui,
            t,
            brect,
            &text,
            size::LABEL,
            Weight::W400,
            Color32::from_rgba_unmultiplied(s1.r(), s1.g(), s1.b(), 230),
            Color::Text,
            None,
            "cardDurationBadge",
        );
    }
    // Imported / Uploaded badges stack bottom-left (8 px inset, 4 apart).
    let mut badge_y = thumb.max.y - 8.0;
    if d.imported {
        let sz = badge_size(
            ui,
            "Imported",
            size::MICRO,
            Weight::W400,
            Vec2::new(7.0, 2.0),
            0.0,
        );
        badge_y -= sz.y;
        badge(
            ui,
            t,
            Rect::from_min_size(Pos2::new(thumb.min.x + 8.0, badge_y), sz),
            "Imported",
            size::MICRO,
            Weight::W400,
            t.color(Color::Accent),
            Color::Bg,
            None,
            "cardImportedBadge",
        );
        badge_y -= 4.0;
    }
    if d.uploaded {
        let sz = badge_size(
            ui,
            "Uploaded",
            size::MICRO,
            Weight::W400,
            Vec2::new(7.0, 2.0),
            0.0,
        );
        badge_y -= sz.y;
        badge(
            ui,
            t,
            Rect::from_min_size(Pos2::new(thumb.min.x + 8.0, badge_y), sz),
            "Uploaded",
            size::MICRO,
            Weight::W400,
            t.color(Color::Accent),
            Color::Bg,
            None,
            "cardUploadedBadge",
        );
    }
    // Finalizing badge or action feedback, top-left at 8,8.
    if busy {
        let sz = badge_size(
            ui,
            "Finalizing",
            size::MICRO,
            Weight::W400,
            Vec2::new(7.0, 3.0),
            0.0,
        );
        badge(
            ui,
            t,
            Rect::from_min_size(thumb.min + Vec2::splat(8.0), sz),
            "Finalizing",
            size::MICRO,
            Weight::W400,
            t.color(Color::Warning),
            Color::Bg,
            None,
            "cardFinalizingBadge",
        );
    } else if let Some(fb) = &d.feedback {
        let sz = badge_size(ui, fb, size::LABEL, Weight::W400, Vec2::new(8.0, 4.0), 1.0);
        badge(
            ui,
            t,
            Rect::from_min_size(thumb.min + Vec2::splat(8.0), sz),
            fb,
            size::LABEL,
            Weight::W400,
            t.color(Color::Surface3),
            Color::Text,
            Some(Color::AccentDim),
            "cardActionFeedback",
        );
    }
    // Body: margins 10, spacing 8; title 32 tall, 3 px gap, metadata fills.
    let body = Rect::from_min_size(
        Pos2::new(rect.min.x, thumb.max.y),
        Vec2::new(width, CARD_BODY_H),
    );
    fidelity::mark(ui, "cardBody", body)
        .fill(Color::CardBg)
        .edge(Color::CardBg)
        .top(Color::CardBg)
        .done();
    let info_w = width - 10.0 - 8.0 - 48.0 - 8.0 - 48.0 - 10.0;
    let title_rect = Rect::from_min_size(
        Pos2::new(body.min.x + 10.0, body.min.y + 10.0),
        Vec2::new(info_w, 32.0),
    );
    let meta_rect = Rect::from_min_size(
        Pos2::new(body.min.x + 10.0, title_rect.max.y + 3.0),
        Vec2::new(info_w, CARD_BODY_H - 10.0 - 32.0 - 3.0 - 10.0),
    );
    let title_resp = ui.interact(
        title_rect,
        ui.scope_id().with((name, "title")),
        Sense::click(),
    );
    let f2 = out.hovered && ui.ctx().input(|i| i.key_pressed(egui::Key::F2));
    let focus_id = ui.scope_id().with((name, "rename-focus"));
    if editing.is_none() && !busy && (title_resp.double_clicked() || f2) {
        *editing = Some(d.title.clone());
        ui.ctx().data_mut(|d| d.insert_temp(focus_id, true));
    }
    if let Some(draft) = editing.as_mut() {
        let edit_id = ui.scope_id().with((name, "rename"));
        let inner = title_rect.shrink2(Vec2::new(0.0, 0.0));
        ui.painter().rect_filled(
            inner,
            CornerRadius::same(geom::RADIUS_SM as u8),
            t.color(Color::Surface1),
        );
        ui.painter().rect_stroke(
            inner,
            CornerRadius::same(geom::RADIUS_SM as u8),
            Stroke::new(1.0, t.color(Color::Accent)),
            StrokeKind::Inside,
        );
        let text_rect = Rect::from_min_max(
            inner.min + Vec2::new(4.0, 1.0),
            inner.max - Vec2::new(4.0, 1.0),
        );
        let edit = egui::TextEdit::singleline(draft)
            .id(edit_id)
            .font(fonts::id(size::BODY_L, Weight::W400))
            .text_color(t.color(Color::Text))
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_width(text_rect.width());
        // Read the keys before the text edit consumes them.
        let (enter, escape) = ui.ctx().input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        let resp = ui.put(text_rect, edit).accessible_name("Clip filename");
        // Focus once when the edit starts; afterwards losing it ends the
        // edit like Qt's editingFinished.
        if ui
            .ctx()
            .data_mut(|d| d.remove_temp::<bool>(focus_id))
            .is_some()
        {
            ui.ctx().memory_mut(|m| m.request_focus(edit_id));
        }
        fidelity::mark(ui, "cardRename", inner)
            .fill(Color::Surface1)
            .done();
        if enter || (resp.lost_focus() && !escape) {
            out.rename_committed = Some(draft.clone());
            *editing = None;
        } else if escape {
            *editing = None;
        }
    } else {
        let title_color = if busy { Color::TextMuted } else { Color::Text };
        let elided = elide(ui, &d.title, size::BODY, Weight::W600, info_w);
        let g = galley(ui, &elided, size::BODY, Weight::W600, t.color(title_color));
        paint_text(ui, g, title_rect, Align::Min, 0.0, t.color(title_color));
        fidelity::mark(ui, "cardTitle", title_rect)
            .top(Color::CardBg)
            .text(&d.title, size::BODY, Weight::W600)
            .done();
    }
    let elided_meta = elide(ui, &d.metadata, size::MICRO, Weight::W400, info_w);
    let g = galley(
        ui,
        &elided_meta,
        size::MICRO,
        Weight::W400,
        t.color(Color::TextDim),
    );
    paint_text(ui, g, meta_rect, Align::Min, 0.0, t.color(Color::TextDim));
    fidelity::mark(ui, "cardMetadata", meta_rect)
        .fill(Color::CardBg)
        .edge(Color::CardBg)
        .top(Color::CardBg)
        .text(&d.metadata, size::MICRO, Weight::W400)
        .done();
    // Share (upload / copy link) and delete, 48×52 centred in the body.
    let btn_y = body.min.y + (CARD_BODY_H - 52.0) / 2.0;
    let share_rect = Rect::from_min_size(
        Pos2::new(title_rect.max.x + 8.0, btn_y),
        Vec2::new(48.0, 52.0),
    );
    let delete_rect = Rect::from_min_size(
        Pos2::new(share_rect.max.x + 8.0, btn_y),
        Vec2::new(48.0, 52.0),
    );
    let has_link = d.upload_link.is_some();
    let share_enabled = d.ready && (has_link || (d.is_video && d.upload_enabled));
    let show_share = d.is_video || has_link;
    if show_share {
        let icon = if has_link { Icon::Link } else { Icon::Upload };
        let r = icon_button(
            ui,
            t,
            icons,
            share_rect,
            icon,
            Color::Accent,
            Color::AccentSoft,
            Color::Accent,
            Color::Accent,
            Color::Bg,
            Color::Accent,
            share_enabled,
            "cardShareBtn",
            name,
        );
        if r.clicked() {
            out.share_clicked = true;
        }
    }
    let r = icon_button(
        ui,
        t,
        icons,
        delete_rect,
        Icon::Delete,
        Color::Delete,
        Color::ErrorSoft,
        Color::Delete,
        Color::Error,
        Color::Text,
        Color::Text,
        d.ready,
        "cardDeleteBtn",
        name,
    );
    if r.clicked() {
        out.delete_requested = true;
    }
    out
}
