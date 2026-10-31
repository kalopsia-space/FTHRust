//! Gary mode (`ui/gary_overlay.py`): a click-through picture over the
//! whole output that grows and fades in while the microphone is loud. A
//! thread ticks every 33 ms (`_update_gary_mode`), reading the mic level;
//! the layer-shell surface exists only while the picture shows. One
//! appearance in ten shows the cat instead.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use egui::{Color32, ColorImage, Rect, TextureHandle, Vec2};
use fthrust_core::gary::{intensity_for_level, step_intensity};
use fthrust_wayland::layer_shell::Placement;

use super::surface::{OverlayGpu, OverlaySurface};
use crate::services::mic_level::SharedMeter;

/// The tick (`QTimer` 33 ms).
pub const TICK: Duration = Duration::from_millis(33);

/// Intensity and the per-appearance cat roll.
#[derive(Clone, Copy, Debug, Default)]
pub struct GaryState {
    intensity: f64,
    cat: bool,
}

impl GaryState {
    /// One tick: off hides at once; otherwise step towards the level's
    /// intensity, rolling for the cat when the picture appears.
    pub fn tick(
        &mut self,
        level: f32,
        enabled: bool,
        (low, high): (i64, i64),
        roll: &mut dyn FnMut() -> u32,
    ) -> f64 {
        if !enabled {
            self.intensity = 0.0;
            return 0.0;
        }
        let target = intensity_for_level(f64::from(level), low, high);
        let next = step_intensity(self.intensity, target);
        if self.intensity <= 0.0 && next > 0.0 {
            self.cat = roll().is_multiple_of(10);
        }
        self.intensity = next;
        next
    }

    /// This appearance shows the cat.
    pub fn cat(&self) -> bool {
        self.cat
    }
}

/// The picture: `0.55 + 0.45 × intensity` of a 58 % × 76 % box, aspect
/// kept, centred on the screen.
pub fn picture_rect(screen: Rect, image: Vec2, intensity: f64) -> Rect {
    let scale = 0.55 + 0.45 * intensity.clamp(0.0, 1.0) as f32;
    let max_w = (screen.width() * 0.58 * scale).trunc();
    let max_h = (screen.height() * 0.76 * scale).trunc();
    let fit = (max_w / image.x.max(1.0)).min(max_h / image.y.max(1.0));
    Rect::from_center_size(screen.center(), image * fit)
}

/// What the Gary thread shows.
#[derive(Clone)]
pub struct GarySettings {
    /// `gary_mode_enabled` and audio capture on.
    pub enabled: bool,
    /// Clamped thresholds.
    pub thresholds: (i64, i64),
    /// The picture (`None`: the bundled `gary.png`).
    pub image: Option<PathBuf>,
    /// The output of the main window.
    pub output: Option<String>,
    /// The mic level.
    pub meter: Option<SharedMeter>,
}

enum Cmd {
    Settings(Box<GarySettings>),
    Shutdown,
}

/// The Gary thread (UI side).
pub struct GaryOverlay {
    tx: Sender<Cmd>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl GaryOverlay {
    /// Starts the thread; `assets` holds `gary.png` and `gatze.png`.
    pub fn start(assets: PathBuf) -> GaryOverlay {
        let (tx, rx) = channel();
        let thread = std::thread::Builder::new()
            .name("fthr-gary".into())
            .spawn(move || run(rx, assets))
            .ok();
        GaryOverlay { tx, thread }
    }

    /// Applies new settings.
    pub fn set(&self, settings: GarySettings) {
        let _ = self.tx.send(Cmd::Settings(Box::new(settings)));
    }
}

impl Drop for GaryOverlay {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Decodes a picture no larger than 2048 px on its long side.
fn load(path: &Path) -> Option<std::sync::Arc<image::RgbaImage>> {
    let img = image::open(path).ok()?;
    let img = if img.width().max(img.height()) > 2048 {
        img.resize(2048, 2048, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    Some(std::sync::Arc::new(img.to_rgba8()))
}

/// `img` no larger than it can ever appear on a `screen`-sized surface
/// (the largest [`picture_rect`]), as egui wants it.
pub fn fitted(img: &image::RgbaImage, screen: Vec2) -> ColorImage {
    let full = Vec2::new(img.width() as f32, img.height() as f32);
    let shown = picture_rect(Rect::from_min_size(egui::Pos2::ZERO, screen), full, 1.0).size();
    let scaled;
    let img = if shown.x < full.x && shown.y >= 1.0 && shown.x >= 1.0 {
        scaled = image::imageops::resize(
            img,
            shown.x.round() as u32,
            shown.y.round() as u32,
            image::imageops::FilterType::Triangle,
        );
        &scaled
    } else {
        img
    };
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

fn random_roll() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(Instant::now().elapsed().as_nanos());
    (h.finish() % 1_000_003) as u32
}

fn fresh_ctx() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(1.0);
    ctx
}

fn run(rx: Receiver<Cmd>, assets: PathBuf) {
    let mut settings: Option<GarySettings> = None;
    let mut state = GaryState::default();
    // [picture, cat]
    let mut images: [Option<std::sync::Arc<image::RgbaImage>>; 2] = [None, None];
    let cat = load(&assets.join("gatze.png"));
    let mut image_path: Option<PathBuf> = None;
    // One egui Context and one GPU for the thread's lifetime (each
    // appearance used to build a Vulkan device and a renderer); the
    // textures are made once per picture and output size (the picture was
    // deep-copied, up to 16.8 MB, on every 33 ms tick).
    let mut ctx = fresh_ctx();
    let mut gpu = OverlayGpu::new();
    let mut textures: [Option<TextureHandle>; 2] = [None, None];
    let mut textures_for: Option<(u32, u32)> = None;
    let mut shown: Option<OverlaySurface> = None;
    let started = Instant::now();
    loop {
        match rx.recv_timeout(TICK) {
            Ok(Cmd::Settings(s)) => {
                let wanted = s.image.clone();
                if images[0].is_none() || wanted != image_path {
                    // A custom picture that fails falls back to the bundled one.
                    images[0] = wanted
                        .as_deref()
                        .and_then(load)
                        .or_else(|| load(&assets.join("gary.png")));
                    images[1] = cat.clone();
                    image_path = wanted;
                    textures = [None, None];
                    shown = None;
                }
                settings = Some(*s);
            }
            Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if gpu.device_lost() {
            shown = None;
            textures = [None, None];
            gpu.reset();
            ctx = fresh_ctx();
        }
        let Some(s) = &settings else { continue };
        let level = s
            .meter
            .as_ref()
            .map(|m| {
                m.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .read(Instant::now())
                    .0
            })
            .unwrap_or(0.0);
        let enabled = s.enabled && s.meter.is_some() && images[0].is_some();
        let intensity = state.tick(level, enabled, s.thresholds, &mut random_roll);
        if intensity <= 0.0 {
            shown = None;
            continue;
        }
        if shown.is_none() {
            match OverlaySurface::new_placed(
                &mut gpu,
                s.output.as_deref(),
                Placement::Fullscreen,
                "fthrust-gary",
            ) {
                Ok(surface) => shown = Some(surface),
                Err(e) => {
                    // No layer-shell: Gary stays off (spec).
                    tracing::warn!(error = %e, "Gary overlay unavailable");
                    settings = None;
                    continue;
                }
            }
        }
        let Some(surface) = shown.as_mut() else {
            continue;
        };
        if !surface.pump(&gpu) {
            shown = None;
            continue;
        }
        if textures_for != Some(surface.size()) {
            textures = [None, None];
            textures_for = Some(surface.size());
        }
        let which = usize::from(state.cat() && images[1].is_some());
        let Some(img) = images[which].as_ref() else {
            continue;
        };
        let screen = Vec2::new(surface.size().0 as f32, surface.size().1 as f32);
        let tex = textures[which]
            .get_or_insert_with(|| {
                ctx.load_texture("gary", fitted(img, screen), egui::TextureOptions::LINEAR)
            })
            .clone();
        let t = started.elapsed().as_secs_f64();
        let result = surface.render(&mut gpu, &ctx, t, |_, painter, rect| {
            let r = picture_rect(rect, tex.size_vec2(), intensity);
            painter.image(
                tex.id(),
                r,
                Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                Color32::WHITE.gamma_multiply(intensity as f32),
            );
        });
        if let Err(e) = result {
            tracing::warn!(error = %e, "Gary frame failed");
            shown = None;
            textures = [None, None];
            gpu.reset();
            ctx = fresh_ctx();
        }
    }
}
