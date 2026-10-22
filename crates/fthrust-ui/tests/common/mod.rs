//! Shared harness helpers: render a closure at a Qt screen size, collect
//! markers, compare with the acceptance JSON.
#![allow(dead_code)]

use egui::{Pos2, Vec2};
use egui_kittest::Harness;
use fthrust_ui::fidelity::{self, Marker, QtScreen, Report};
use fthrust_ui::fonts::{self, Weight};
use fthrust_ui::tokens::Tokens;

pub struct Rendered {
    pub image: image::RgbaImage,
    pub markers: Vec<Marker>,
    pub ppp: f32,
}

impl Rendered {
    pub fn pixel(&self, at: Pos2) -> Option<egui::Color32> {
        let x = (at.x * self.ppp).round() as i64;
        let y = (at.y * self.ppp).round() as i64;
        if x < 0 || y < 0 || x >= self.image.width() as i64 || y >= self.image.height() as i64 {
            return None;
        }
        let p = self.image.get_pixel(x as u32, y as u32);
        Some(egui::Color32::from_rgb(p[0], p[1], p[2]))
    }

    pub fn marker(&self, name: &str) -> Option<&Marker> {
        self.markers.iter().find(|m| m.name == name)
    }
}

/// Builds a harness with the app fonts installed and runs `app` for `steps` frames.
pub fn harness<'a>(
    size: Vec2,
    ppp: f32,
    tokens: &Tokens,
    app: impl FnMut(&mut egui::Ui) + 'a,
) -> Harness<'a> {
    harness_with_dt(size, ppp, tokens, 1.0 / 60.0, app)
}

/// [`harness`] with an explicit frame duration (timing tests).
pub fn harness_with_dt<'a>(
    size: Vec2,
    ppp: f32,
    tokens: &Tokens,
    step_dt: f32,
    app: impl FnMut(&mut egui::Ui) + 'a,
) -> Harness<'a> {
    let tokens = tokens.clone();
    let mut h = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(ppp)
        .with_step_dt(step_dt)
        .with_context_setup(move |ctx| {
            fonts::install(ctx, &tokens);
            fthrust_ui::tokens::install_style(ctx, &tokens);
        })
        .build_ui(app);
    h.run_steps(2);
    h
}

/// Renders `app` once (after it settled) and collects its markers.
pub fn render(size: Vec2, ppp: f32, tokens: &Tokens, app: impl FnMut(&mut egui::Ui)) -> Rendered {
    let mut h = harness(size, ppp, tokens, app);
    fidelity::take(&h.ctx);
    h.step();
    let markers = fidelity::take(&h.ctx);
    let image = h
        .render()
        .expect("GPU render (needs a Vulkan adapter; CI uses lavapipe)");
    Rendered {
        image,
        markers,
        ppp,
    }
}

pub fn row_height_fn(ctx: &egui::Context) -> impl Fn(f32, Weight) -> f32 + '_ {
    move |px, w| fonts::measure(ctx, "Hg", px, w).height
}

/// Compares a render with the Qt screen JSON and prints the report.
pub fn compare(rendered: &Rendered, screen: &str, tokens: &Tokens, ctx: &egui::Context) -> Report {
    let qt = QtScreen::load(screen).unwrap();
    let mut report = fidelity::compare(
        &rendered.markers,
        &qt,
        tokens,
        |p| rendered.pixel(p),
        row_height_fn(ctx),
    );
    let png = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../docs/reference/qt-screens/{screen}.png"));
    if let Ok(qt_img) = image::open(&png) {
        report.ssim = fidelity::ssim(&rendered.image, &qt_img.to_rgba8());
    }
    if let Ok(dir) = std::env::var("FTHRUST_FIDELITY_DUMP") {
        let _ = std::fs::create_dir_all(&dir);
        let _ = rendered.image.save(format!("{dir}/{screen}.egui.png"));
        let lines: Vec<String> = rendered
            .markers
            .iter()
            .map(|m| {
                format!(
                    "{} {:?} {:?}",
                    m.name,
                    m.rect,
                    m.text.as_ref().map(|t| &t.content)
                )
            })
            .collect();
        let _ = std::fs::write(format!("{dir}/{screen}.markers.txt"), lines.join("\n"));
    }
    eprintln!(
        "[fidelity {screen}] compared {} markers, {} failures, ssim {:?}",
        report.compared,
        report.failures.len(),
        report.ssim
    );
    for f in &report.failures {
        eprintln!("  - {f}");
    }
    report
}

#[macro_export]
macro_rules! assert_fidelity {
    ($rendered:expr, $screen:expr, $tokens:expr, $ctx:expr) => {{
        let report = $crate::common::compare(&$rendered, $screen, $tokens, $ctx);
        assert!(
            report.compared > 0,
            "no marker of {} matched a Qt widget",
            $screen
        );
        assert!(
            report.failures.is_empty(),
            "{} fidelity failures on {}",
            report.failures.len(),
            $screen
        );
    }};
}
