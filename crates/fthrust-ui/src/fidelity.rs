//! Per-widget fidelity markers. Every custom widget registers the rect it
//! drew, the colour it expects at fixed sample points and the text run it
//! laid out, under the Qt object name it reproduces. The test harness
//! collects the markers after a frame and compares them with the Qt
//! acceptance JSON in `docs/reference/qt-screens/` (spec 6.4, decision 10).

use std::sync::Arc;

use egui::{Context, Id, Pos2, Rect, Ui};
use serde::{Deserialize, Serialize};

use crate::fonts::Weight;
use crate::tokens::Color;

/// One sampled point.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// `fill`, `edge`, `top`, `accent`, `text`.
    pub role: &'static str,
    /// Absolute position in logical pixels.
    pub at: Pos2,
    /// The token the pixel must equal.
    pub expect: Color,
}

/// A laid-out text run.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    /// The visible string.
    pub content: String,
    /// Pixel size.
    pub px: f32,
    /// Weight.
    pub weight: Weight,
}

/// A registered widget.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    /// Qt object name (`settingsTabBtn[General]`).
    pub name: String,
    /// Bounding box in logical pixels.
    pub rect: Rect,
    /// Colour samples.
    pub samples: Vec<Sample>,
    /// Text run, when the widget shows text.
    pub text: Option<TextRun>,
    /// Icon asset and logical size, when the widget shows an icon.
    pub icon: Option<(String, f32)>,
}

#[derive(Clone, Default)]
struct Registry(Arc<std::sync::Mutex<Vec<Marker>>>);

fn registry(ctx: &Context) -> Registry {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Registry>(Id::unique("fthrust-fidelity"))
            .clone()
    })
}

/// Builder returned by [`mark`].
pub struct MarkerBuilder<'a> {
    ctx: &'a Context,
    marker: Marker,
}

impl MarkerBuilder<'_> {
    /// Expects `color` at the rect centre.
    pub fn fill(mut self, color: Color) -> Self {
        let at = self.marker.rect.center();
        self.marker.samples.push(Sample {
            role: "fill",
            at,
            expect: color,
        });
        self
    }

    /// Expects `color` at an absolute position.
    pub fn sample(mut self, role: &'static str, at: Pos2, color: Color) -> Self {
        self.marker.samples.push(Sample {
            role,
            at,
            expect: color,
        });
        self
    }

    /// Expects `color` on the left edge, mid-height (a 1 px border).
    pub fn edge(mut self, color: Color) -> Self {
        let r = self.marker.rect;
        self.marker.samples.push(Sample {
            role: "edge",
            at: Pos2::new(r.min.x, r.center().y),
            expect: color,
        });
        self
    }

    /// Expects `color` on the top edge, mid-width.
    pub fn top(mut self, color: Color) -> Self {
        let r = self.marker.rect;
        self.marker.samples.push(Sample {
            role: "top",
            at: Pos2::new(r.center().x, r.min.y),
            expect: color,
        });
        self
    }

    /// Records the text run.
    pub fn text(mut self, content: impl Into<String>, px: f32, weight: Weight) -> Self {
        self.marker.text = Some(TextRun {
            content: content.into(),
            px,
            weight,
        });
        self
    }

    /// Records the icon asset and size.
    pub fn icon(mut self, asset: impl Into<String>, size_px: f32) -> Self {
        self.marker.icon = Some((asset.into(), size_px));
        self
    }

    /// Stores the marker.
    pub fn done(self) {
        registry(self.ctx).0.lock().unwrap().push(self.marker);
    }
}

/// Starts a marker for `name` covering `rect`.
pub fn mark<'a>(ui: &'a Ui, name: impl Into<String>, rect: Rect) -> MarkerBuilder<'a> {
    MarkerBuilder {
        ctx: ui.ctx(),
        marker: Marker {
            name: name.into(),
            rect,
            samples: Vec::new(),
            text: None,
            icon: None,
        },
    }
}

/// Takes every marker registered since the last call.
pub fn take(ctx: &Context) -> Vec<Marker> {
    std::mem::take(&mut *registry(ctx).0.lock().unwrap())
}

// ---------------------------------------------------------------------------
// Qt acceptance JSON (written by tools/render-qt-reference.py).

/// One sample in the Qt JSON.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct QtSample {
    /// Role.
    pub role: String,
    /// Position.
    pub at: [i32; 2],
    /// Hex colour.
    pub rgb: String,
    /// Palette token, when the pixel was a palette colour.
    #[serde(default)]
    pub token: Option<String>,
}

/// Text metrics in the Qt JSON.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct QtText {
    /// Content.
    pub content: String,
    /// Pixel size.
    pub px: f32,
    /// Qt weight.
    pub weight: u32,
    /// Font family.
    #[serde(default)]
    pub family: String,
    /// Ascent.
    pub ascent: f32,
    /// Descent.
    pub descent: f32,
    /// Line height.
    pub height: f32,
    /// Horizontal advance of the first line.
    pub advance: f32,
}

/// One widget in the Qt JSON.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct QtWidget {
    /// Object name.
    pub name: String,
    /// Qt class.
    #[serde(default)]
    pub class: String,
    /// `[x, y, w, h]`.
    pub rect: [i32; 4],
    /// Enabled flag.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Checked flag for checkable buttons.
    #[serde(default)]
    pub checked: Option<bool>,
    /// Samples.
    #[serde(default)]
    pub samples: Vec<QtSample>,
    /// Text.
    #[serde(default)]
    pub text: Option<QtText>,
}

fn default_true() -> bool {
    true
}

/// A screen's acceptance data.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct QtScreen {
    /// Screen name.
    pub screen: String,
    /// `[w, h]`.
    pub size: [u32; 2],
    /// Widgets.
    pub widgets: Vec<QtWidget>,
}

impl QtScreen {
    /// Loads `docs/reference/qt-screens/<name>.json`.
    pub fn load(name: &str) -> Result<QtScreen, String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reference/qt-screens")
            .join(format!("{name}.json"));
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The widget named `name`.
    pub fn widget(&self, name: &str) -> Option<&QtWidget> {
        self.widgets.iter().find(|w| w.name == name)
    }
}

/// The result of comparing rendered markers with a Qt screen.
#[derive(Debug, Default)]
pub struct Report {
    /// One line per violated rule.
    pub failures: Vec<String>,
    /// Markers compared.
    pub compared: usize,
    /// Structural similarity against the Qt PNG, when computed.
    pub ssim: Option<f64>,
}

/// Compares `markers` with the Qt widgets of the same names. Rules: each
/// box edge within 1 px; each egui sample's pixel equals the token; text
/// size and weight equal and egui row height within 1 px of Qt's
/// `ascent + descent`.
pub fn compare(
    markers: &[Marker],
    qt: &QtScreen,
    tokens: &crate::tokens::Tokens,
    pixel: impl Fn(Pos2) -> Option<egui::Color32>,
    row_height: impl Fn(f32, Weight) -> f32,
) -> Report {
    let mut report = Report::default();
    for m in markers {
        let Some(w) = qt.widget(&m.name) else {
            continue;
        };
        report.compared += 1;
        let [qx, qy, qw, qh] = w.rect.map(|v| v as f32);
        // The extractor records widgets clipped to the window; clip ours too.
        let screen = Rect::from_min_size(
            Pos2::ZERO,
            egui::Vec2::new(qt.size[0] as f32, qt.size[1] as f32),
        );
        let clipped = m.rect.intersect(screen);
        let got = [
            clipped.min.x,
            clipped.min.y,
            clipped.width(),
            clipped.height(),
        ];
        for (i, (g, q)) in got.iter().zip([qx, qy, qw, qh]).enumerate() {
            if (g - q).abs() > 1.0 {
                let edge = ["x", "y", "w", "h"][i];
                report
                    .failures
                    .push(format!("{}: {edge} {g} vs Qt {q}", m.name));
            }
        }
        for s in &m.samples {
            if !screen.contains(s.at) {
                continue; // clipped away in the Qt capture as well
            }
            let want = tokens.color(s.expect);
            match pixel(s.at) {
                Some(got) if got.r() == want.r() && got.g() == want.g() && got.b() == want.b() => {}
                Some(got) => report.failures.push(format!(
                    "{}: {} at ({}, {}) is {} not {} ({})",
                    m.name,
                    s.role,
                    s.at.x,
                    s.at.y,
                    crate::tokens::to_hex(got),
                    s.expect.key(),
                    crate::tokens::to_hex(want)
                )),
                None => report.failures.push(format!(
                    "{}: {} at ({}, {}) off-image",
                    m.name, s.role, s.at.x, s.at.y
                )),
            }
        }
        if let (Some(t), Some(q)) = (&m.text, &w.text) {
            if t.px != q.px {
                report
                    .failures
                    .push(format!("{}: text {}px vs Qt {}px", m.name, t.px, q.px));
            }
            // QFontInfo reports the base font; a `:checked` QSS rule can paint a
            // different weight that the JSON cannot see.
            if w.checked != Some(true) && t.weight != Weight::from_qt(q.weight) {
                report.failures.push(format!(
                    "{}: weight {:?} vs Qt {}",
                    m.name, t.weight, q.weight
                ));
            }
            let h = row_height(t.px, t.weight);
            if (h - q.height).abs() > 1.0 {
                report
                    .failures
                    .push(format!("{}: row height {h} vs Qt {}", m.name, q.height));
            }
        }
    }
    report
}

/// Mean SSIM (8×8 windows, grey) between two RGBA images of equal size.
pub fn ssim(a: &image::RgbaImage, b: &image::RgbaImage) -> Option<f64> {
    if a.dimensions() != b.dimensions() {
        return None;
    }
    let (w, h) = a.dimensions();
    let grey = |img: &image::RgbaImage, x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64
    };
    let (c1, c2) = ((0.01f64 * 255.0).powi(2), (0.03f64 * 255.0).powi(2));
    let mut total = 0.0;
    let mut n = 0;
    let win = 8;
    let mut y = 0;
    while y + win <= h {
        let mut x = 0;
        while x + win <= w {
            let (mut ma, mut mb) = (0.0, 0.0);
            for j in 0..win {
                for i in 0..win {
                    ma += grey(a, x + i, y + j);
                    mb += grey(b, x + i, y + j);
                }
            }
            let cnt = (win * win) as f64;
            ma /= cnt;
            mb /= cnt;
            let (mut va, mut vb, mut cov) = (0.0, 0.0, 0.0);
            for j in 0..win {
                for i in 0..win {
                    let da = grey(a, x + i, y + j) - ma;
                    let db = grey(b, x + i, y + j) - mb;
                    va += da * da;
                    vb += db * db;
                    cov += da * db;
                }
            }
            va /= cnt - 1.0;
            vb /= cnt - 1.0;
            cov /= cnt - 1.0;
            total += ((2.0 * ma * mb + c1) * (2.0 * cov + c2))
                / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            n += 1;
            x += win;
        }
        y += win;
    }
    (n > 0).then(|| total / n as f64)
}
