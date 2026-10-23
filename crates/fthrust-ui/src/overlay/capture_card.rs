//! The capture card (`ui/capture_card.py`): content per kind, the
//! slide-in / hold / slide-out timeline with shimmer and progress, and the
//! painter that reproduces the Qt layout at a scale.

use egui::{
    Color32, Context, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, TextureHandle,
    TextureOptions, Vec2,
};

use crate::fonts::{self, Weight};
use crate::icons::{Icon, rasterise};
use crate::services::CardKind;
use crate::tokens::Tokens;

/// Full card width.
pub const FULL_W: f32 = 340.0;
/// Full card height.
pub const FULL_H: f32 = 116.0;
/// Compact card width.
pub const COMPACT_W: f32 = 320.0;
/// Compact card height.
pub const COMPACT_H: f32 = 76.0;
/// Left accent rail.
pub const BORDER: f32 = 3.0;
/// Slide-in duration.
pub const SLIDE_IN_S: f64 = 0.45;
/// Default hold.
pub const HOLD_S: f64 = 2.4;
/// Slide-out duration.
pub const SLIDE_OUT_S: f64 = 0.35;
/// Shimmer starts this long after the show.
pub const SHIMMER_DELAY_S: f64 = 0.15;
/// Shimmer sweep duration.
pub const SHIMMER_S: f64 = 0.55;
/// Progress starts this long after the slide-in.
pub const PROGRESS_DELAY_S: f64 = 0.03;

/// What a card says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardContent {
    /// Upper-case headline (capitalised when painted).
    pub headline: String,
    /// Compact detail line.
    pub detail: String,
    /// `(value, label)` columns of a full card.
    pub stats: Vec<(String, String)>,
    /// Compact (320×76) or full (340×116).
    pub compact: bool,
}

/// `show_*` texts.
pub fn content(kind: &CardKind) -> CardContent {
    let (headline, detail, stats, compact) = match kind {
        CardKind::Clip {
            seconds,
            fps,
            resolution,
        } => (
            "CLIP CAPTURED",
            String::new(),
            vec![
                (format!("{seconds}s"), "Duration".to_owned()),
                (format!("{fps} FPS"), "Framerate".to_owned()),
                (resolution.clone(), "Resolution".to_owned()),
            ],
            false,
        ),
        CardKind::Screenshot => ("SCREENSHOT SAVED", String::new(), vec![], true),
        CardKind::Error { detail } => ("CAPTURE FAILED", detail.clone(), vec![], true),
        CardKind::Upload { filename } => ("CLIP UPLOADED", filename.clone(), vec![], true),
        CardKind::UploadFailed { detail } => ("UPLOAD FAILED", detail.clone(), vec![], true),
        CardKind::RecordingSaved { filename } => (
            "MANUAL RECORDING SAVED",
            if filename.is_empty() {
                "Ready to trim and export.".to_owned()
            } else {
                format!("{filename}  ·  Ready to trim and export.")
            },
            vec![],
            true,
        ),
        CardKind::Prompt { text } => ("CAPTURE UPDATE", text.clone(), vec![], true),
        CardKind::Capturing { source } => (
            "NOW CAPTURING",
            if source.is_empty() {
                "Desktop".to_owned()
            } else {
                source.clone()
            },
            vec![],
            true,
        ),
        CardKind::Startup => ("", String::new(), vec![], true),
    };
    CardContent {
        headline: headline.to_owned(),
        detail,
        stats,
        compact,
    }
}

/// Whether a kind has a visual card (the startup cue is sound only).
pub fn visual(kind: &CardKind) -> bool {
    !matches!(kind, CardKind::Startup)
}

/// One animation frame.
#[derive(Clone, Debug, PartialEq)]
pub struct CardFrame {
    /// Horizontal offset from the resting position (positive = towards off-screen right).
    pub x_offset: f32,
    /// `1 → 0` during the hold.
    pub progress: f32,
    /// `-1` off, else `0 → 3`.
    pub shimmer: f32,
    /// The text.
    pub content: CardContent,
}

struct Shown {
    content: CardContent,
    started: f64,
    hold_s: f64,
}

/// The timeline.
#[derive(Default)]
pub struct CardModel {
    shown: Option<Shown>,
}

fn out_back(t: f64) -> f64 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

fn in_cubic(t: f64) -> f64 {
    t * t * t
}

impl CardModel {
    /// Starts (or restarts) the card at `now`.
    pub fn show(&mut self, kind: CardKind, hold_ms: Option<u32>, now: f64) {
        let hold_s = hold_ms
            .map(|m| f64::from(m.max(1)) / 1000.0)
            .unwrap_or(HOLD_S);
        self.shown = Some(Shown {
            content: content(&kind),
            started: now,
            hold_s,
        });
    }

    /// Hides immediately.
    pub fn hide(&mut self) {
        self.shown = None;
    }

    /// The base size of the current card (full when nothing is shown).
    pub fn size(&self) -> (f32, f32) {
        match &self.shown {
            Some(s) if s.content.compact => (COMPACT_W, COMPACT_H),
            _ => (FULL_W, FULL_H),
        }
    }

    /// A card is on its timeline.
    pub fn is_live(&self) -> bool {
        self.shown.is_some()
    }

    /// The frame at `now`, or `None` once the card is gone.
    pub fn frame(&mut self, now: f64) -> Option<CardFrame> {
        let (w, _) = self.size();
        let s = self.shown.as_ref()?;
        let e = now - s.started;
        let off = f64::from(w) + 10.0;
        let x = if e < 0.0 {
            off
        } else if e < SLIDE_IN_S {
            off * (1.0 - out_back(e / SLIDE_IN_S))
        } else if e < SLIDE_IN_S + s.hold_s {
            0.0
        } else if e <= SLIDE_IN_S + s.hold_s + SLIDE_OUT_S {
            off * in_cubic((e - SLIDE_IN_S - s.hold_s) / SLIDE_OUT_S)
        } else {
            self.shown = None;
            return None;
        };
        let shimmer = {
            let e2 = e - SHIMMER_DELAY_S;
            if (0.0..SHIMMER_S).contains(&e2) {
                (e2 / SHIMMER_S * 3.0) as f32
            } else {
                -1.0
            }
        };
        let progress = {
            let ep = e - SLIDE_IN_S - PROGRESS_DELAY_S;
            if ep < 0.0 {
                1.0
            } else {
                (1.0 - ep / s.hold_s).max(0.0) as f32
            }
        };
        Some(CardFrame {
            x_offset: x as f32,
            progress,
            shimmer,
            content: s.content.clone(),
        })
    }
}

/// Where the painter put things (card-local, unscaled; for tests).
#[derive(Clone, Debug, PartialEq)]
pub struct PaintLayout {
    /// Headline text rect.
    pub headline_rect: Rect,
    /// Camera icon rect.
    pub icon_rect: Rect,
    /// Divider y (full cards).
    pub divider_y: Option<f32>,
    /// Stats column x positions (full cards).
    pub stats_x: Vec<f32>,
}

/// Python `str.capitalize()`.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

fn elide(ctx: &Context, text: &str, font: &FontId, max_w: f32) -> String {
    let width = |s: &str| {
        ctx.fonts_mut(|f| {
            f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
    };
    if width(text) <= max_w {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        n -= 1;
        let candidate: String = chars[..n].iter().collect::<String>().trim_end().to_owned() + "…";
        if width(&candidate) <= max_w {
            return candidate;
        }
    }
    "…".into()
}

fn camera_texture(ctx: &Context, size_px: u32, tint: Color32) -> Option<TextureHandle> {
    let id = egui::Id::unique(("capture-card-camera", size_px, tint));
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return Some(t);
    }
    let path = Icon::Camera.bundled_path(&crate::assets_dir());
    let image = rasterise(&path, size_px, Some(tint)).ok()?;
    let handle = ctx.load_texture(
        format!("capture-card-camera-{size_px}"),
        image,
        TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
    Some(handle)
}

/// Paints `frame` into `rect` (the card's resting rect at `scale`),
/// shifted right by `x_offset·scale`.
pub fn paint(
    painter: &Painter,
    ctx: &Context,
    t: &Tokens,
    frame: &CardFrame,
    rect: Rect,
    scale: f32,
) -> PaintLayout {
    let c = &frame.content;
    let (w, h) = if c.compact {
        (COMPACT_W, COMPACT_H)
    } else {
        (FULL_W, FULL_H)
    };
    let cc = &t.capture_card;
    let origin = rect.min + Vec2::new(frame.x_offset * scale, 0.0);
    let at = |x: f32, y: f32| origin + Vec2::new(x * scale, y * scale);
    let r = |x: f32, y: f32, rw: f32, rh: f32| {
        Rect::from_min_size(at(x, y), Vec2::new(rw * scale, rh * scale))
    };
    painter.rect_filled(r(0.0, 0.0, w, h), 0.0, cc.bg);
    painter.rect_filled(r(0.0, 0.0, BORDER, h), 0.0, cc.accent);
    let inner = r(BORDER, 0.0, w - BORDER, h);
    if frame.shimmer >= 0.0 {
        let cx = BORDER + ((w - BORDER) * (frame.shimmer / 3.0 - 0.3)).floor();
        let clear = Color32::from_rgba_unmultiplied(cc.accent.r(), cc.accent.g(), cc.accent.b(), 0);
        let soft = Color32::from_rgba_unmultiplied(cc.accent.r(), cc.accent.g(), cc.accent.b(), 14);
        let mut mesh = Mesh::default();
        let top = inner.min.y;
        let bottom = inner.max.y;
        let xs = [at(cx - 70.0, 0.0).x, at(cx, 0.0).x, at(cx + 70.0, 0.0).x];
        let cols = [clear, soft, clear];
        for (x, col) in xs.iter().zip(cols.iter()) {
            mesh.colored_vertex(Pos2::new(*x, top), *col);
            mesh.colored_vertex(Pos2::new(*x, bottom), *col);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        mesh.add_triangle(2, 3, 4);
        mesh.add_triangle(3, 5, 4);
        painter.with_clip_rect(inner).add(Shape::mesh(mesh));
    }
    let (icon_x, icon_y, icon_size, hx, hy, hh, hsize) = if c.compact {
        (BORDER + 12.0, 13.0, 20.0, BORDER + 42.0, 8.0, 25.0, 11.0)
    } else {
        (BORDER + 17.0, 17.0, 26.0, BORDER + 52.0, 12.0, 28.0, 15.0)
    };
    let icon_rect = Rect::from_min_size(Pos2::new(icon_x, icon_y), Vec2::splat(icon_size));
    if let Some(tex) = camera_texture(ctx, (icon_size * scale).round().max(1.0) as u32, cc.text) {
        painter.image(
            tex.id(),
            r(icon_x, icon_y, icon_size, icon_size),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    let headline_rect = Rect::from_min_size(Pos2::new(hx, hy), Vec2::new(w - hx - 12.0, hh));
    let hfont = fonts::id(hsize * scale, Weight::W600);
    let headline = elide(
        ctx,
        &capitalize(&c.headline),
        &hfont,
        headline_rect.width() * scale,
    );
    let galley = ctx.fonts_mut(|f| f.layout_no_wrap(headline, hfont, cc.text));
    let hr = r(hx, hy, headline_rect.width(), hh);
    painter.galley(
        Pos2::new(hr.min.x, hr.center().y - galley.size().y / 2.0),
        galley,
        cc.text,
    );
    let mut divider_y = None;
    let mut stats_x = Vec::new();
    if c.compact {
        if !c.detail.is_empty() {
            let dfont = fonts::id(9.0 * scale, Weight::W400);
            let detail_color =
                Color32::from_rgba_unmultiplied(cc.text.r(), cc.text.g(), cc.text.b(), 180);
            let detail = elide(ctx, &c.detail, &dfont, (w - hx - 12.0) * scale);
            let g = ctx.fonts_mut(|f| f.layout_no_wrap(detail, dfont, detail_color));
            let dr = r(hx, 33.0, w - hx - 12.0, 27.0);
            painter.galley(
                Pos2::new(dr.min.x, dr.center().y - g.size().y / 2.0),
                g,
                detail_color,
            );
        }
    } else {
        divider_y = Some(47.0);
        painter.hline(
            at(BORDER + 10.0, 0.0).x..=at(w - 10.0, 0.0).x,
            at(0.0, 47.0).y + 0.5 * scale,
            Stroke::new(1.0 * scale, cc.divider),
        );
        if !c.stats.is_empty() {
            let top = 54.0;
            let n = c.stats.len() as f32;
            let usable = (w - BORDER - 20.0).floor();
            let col_w = (usable / n).floor();
            let base_x = BORDER + 10.0;
            let vfont = fonts::id(11.0 * scale, Weight::W600);
            let lfont = fonts::id(8.0 * scale, Weight::W400);
            for (i, (val, lbl)) in c.stats.iter().enumerate() {
                let cx = base_x + i as f32 * col_w;
                stats_x.push(cx);
                let value = elide(ctx, val, &vfont, (col_w - 6.0) * scale);
                let g = ctx.fonts_mut(|f| f.layout_no_wrap(value, vfont.clone(), cc.text));
                let vr = r(cx, top, col_w - 6.0, 22.0);
                painter.galley(
                    Pos2::new(vr.min.x, vr.center().y - g.size().y / 2.0),
                    g,
                    cc.text,
                );
                if !lbl.is_empty() {
                    let g = ctx.fonts_mut(|f| {
                        f.layout_no_wrap(lbl.to_uppercase(), lfont.clone(), cc.stats_dim)
                    });
                    let lr = r(cx, top + 22.0, col_w - 6.0, 16.0);
                    painter.galley(
                        Pos2::new(lr.min.x, lr.center().y - g.size().y / 2.0),
                        g,
                        cc.stats_dim,
                    );
                }
                if i + 1 < c.stats.len() {
                    let sep_x = cx + col_w - 2.0;
                    painter.vline(
                        at(sep_x, 0.0).x + 0.5 * scale,
                        at(0.0, top + 2.0).y..=at(0.0, top + 34.0).y,
                        Stroke::new(1.0 * scale, cc.divider),
                    );
                }
            }
        }
    }
    painter.rect_filled(r(0.0, h - 2.0, w, 2.0), 0.0, cc.progress_track);
    let fill_w = (w * frame.progress).floor();
    if fill_w > 0.0 {
        painter.rect_filled(r(0.0, h - 2.0, fill_w, 2.0), 0.0, cc.progress_fill);
    }
    PaintLayout {
        headline_rect,
        icon_rect,
        divider_y,
        stats_x,
    }
}
