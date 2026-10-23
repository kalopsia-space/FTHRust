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

use super::surface::OverlaySurface;
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
fn load(path: &Path) -> Option<ColorImage> {
    let img = image::open(path).ok()?;
    let img = if img.width().max(img.height()) > 2048 {
        img.resize(2048, 2048, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

struct Shown {
    surface: OverlaySurface,
    textures: Vec<Option<TextureHandle>>,
}

fn random_roll() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(Instant::now().elapsed().as_nanos());
    (h.finish() % 1_000_003) as u32
}

fn run(rx: Receiver<Cmd>, assets: PathBuf) {
    let mut settings: Option<GarySettings> = None;
    let mut state = GaryState::default();
    // [picture, cat]
    let mut images: [Option<ColorImage>; 2] = [None, None];
    let cat = load(&assets.join("gatze.png"));
    let mut image_path: Option<PathBuf> = None;
    let mut shown: Option<Shown> = None;
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
                    shown = None;
                }
                settings = Some(*s);
            }
            Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
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
            let ctx = egui::Context::default();
            match OverlaySurface::new_placed(
                s.output.as_deref(),
                Placement::Fullscreen,
                "fthrust-gary",
                ctx,
            ) {
                Ok(surface) => {
                    shown = Some(Shown {
                        surface,
                        textures: vec![None, None],
                    })
                }
                Err(e) => {
                    // No layer-shell: Gary stays off (spec).
                    tracing::warn!(error = %e, "Gary overlay unavailable");
                    settings = None;
                    continue;
                }
            }
        }
        let Some(sh) = shown.as_mut() else { continue };
        if !sh.surface.pump() {
            shown = None;
            continue;
        }
        let which = usize::from(state.cat() && images[1].is_some());
        let Some(img) = images[which].clone() else {
            continue;
        };
        let slot = &mut sh.textures[which];
        let t = started.elapsed().as_secs_f64();
        let result = sh.surface.render(t, |ctx, painter, rect| {
            let tex = slot.get_or_insert_with(|| {
                ctx.load_texture("gary", img.clone(), egui::TextureOptions::LINEAR)
            });
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
        }
    }
}
