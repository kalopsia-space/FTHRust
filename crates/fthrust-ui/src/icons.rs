//! Interface icons: the legacy logical names (`clip.png`) map to the bundled
//! SVGs as in the reference's `ui/icons.py`; every icon is rasterised at 2×
//! the requested logical size and tinted with `CompositionMode_SourceIn`
//! (the colour replaces every pixel, alpha is kept). Custom theme files are
//! used untinted unless the theme names a tint for that icon.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::load::SizedTexture;
use egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions, Vec2};

use crate::tokens::Tokens;

/// Every icon the shell draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Icon {
    Clip,
    Sound,
    Visuals,
    General,
    Updates,
    Personalize,
    Performance,
    Shortcuts,
    Home,
    Refresh,
    Play,
    Pause,
    Close,
    Minimize,
    Maximize,
    Shutdown,
    Dropdown,
    Upload,
    Link,
    Delete,
    Camera,
    Increase,
    Decrease,
    StepBack,
    StepForward,
    Kofi,
    Logo,
    Favicon,
}

impl Icon {
    /// All icons.
    pub const ALL: [Icon; 28] = [
        Icon::Clip,
        Icon::Sound,
        Icon::Visuals,
        Icon::General,
        Icon::Updates,
        Icon::Personalize,
        Icon::Performance,
        Icon::Shortcuts,
        Icon::Home,
        Icon::Refresh,
        Icon::Play,
        Icon::Pause,
        Icon::Close,
        Icon::Minimize,
        Icon::Maximize,
        Icon::Shutdown,
        Icon::Dropdown,
        Icon::Upload,
        Icon::Link,
        Icon::Delete,
        Icon::Camera,
        Icon::Increase,
        Icon::Decrease,
        Icon::StepBack,
        Icon::StepForward,
        Icon::Kofi,
        Icon::Logo,
        Icon::Favicon,
    ];

    /// The legacy theme key (`clip.png`), as `theme.json` names it.
    pub fn logical_name(self) -> &'static str {
        match self {
            Icon::Clip => "clip.png",
            Icon::Sound => "sound.png",
            Icon::Visuals => "visuals.png",
            Icon::General => "settings(general).png",
            Icon::Updates => "updates.png",
            Icon::Personalize => "personalize.png",
            Icon::Performance => "performance.png",
            Icon::Shortcuts => "shortcuts.png",
            Icon::Home => "home.png",
            Icon::Refresh => "refresh.png",
            Icon::Play => "play.png",
            Icon::Pause => "pause.png",
            Icon::Close => "close.png",
            Icon::Minimize => "minimize.png",
            Icon::Maximize => "maximize.png",
            Icon::Shutdown => "shutdown.png",
            Icon::Dropdown => "dropdown.png",
            Icon::Upload => "upload.png",
            Icon::Link => "link.png",
            Icon::Delete => "delete.png",
            Icon::Camera => "camera.png",
            Icon::Increase => "increase.png",
            Icon::Decrease => "decrease.png",
            Icon::StepBack => "step-back.png",
            Icon::StepForward => "step-forward.png",
            Icon::Kofi => "kofi_symbol.png",
            Icon::Logo => "fthr_logo.png",
            Icon::Favicon => "favicon.ico",
        }
    }

    /// The bundled file, relative to the assets directory (`ICON_ASSETS`).
    pub fn asset(self) -> &'static str {
        match self {
            Icon::Clip => "icons/clips.svg",
            Icon::Sound => "icons/sound.svg",
            Icon::Visuals => "icons/visuals.svg",
            Icon::General => "icons/settings.svg",
            Icon::Updates => "icons/update.svg",
            Icon::Personalize => "icons/customise.svg",
            Icon::Performance => "icons/performance.svg",
            Icon::Shortcuts => "icons/shortcuts.svg",
            Icon::Home => "icons/home.svg",
            Icon::Refresh => "icons/refresh.svg",
            Icon::Play => "icons/play.svg",
            Icon::Pause => "icons/pause.svg",
            Icon::Close => "icons/close.svg",
            Icon::Minimize => "icons/minimise.svg",
            Icon::Maximize => "icons/maximise.svg",
            Icon::Shutdown => "icons/shutdown.svg",
            Icon::Dropdown => "icons/dropdown.svg",
            Icon::Upload => "icons/upload.svg",
            Icon::Link => "icons/copylink.svg",
            Icon::Delete => "icons/delete.svg",
            Icon::Camera => "icons/camera.svg",
            Icon::Increase => "icons/plus.svg",
            Icon::Decrease => "icons/minimise.svg",
            Icon::StepBack => "icons/chevronleft.svg",
            Icon::StepForward => "icons/chevronright.svg",
            Icon::Kofi => "icons/kofi_symbol.png",
            Icon::Logo => "fthr_logo.png",
            Icon::Favicon => "favicon.ico",
        }
    }

    /// Absolute path of the bundled file.
    pub fn bundled_path(self, assets: &Path) -> PathBuf {
        assets.join(self.asset())
    }

    /// Icons that are never tinted by the global tint (brand marks).
    fn is_brand(self) -> bool {
        matches!(self, Icon::Kofi | Icon::Logo | Icon::Favicon)
    }
}

/// Icon sizes the shell uses.
pub fn sizes() -> [f32; 5] {
    [14.0, 16.0, 18.0, 28.0, 36.0]
}

/// Rasterises an SVG or bitmap at `2 * size_px` square (aspect kept, centred
/// for SVGs) and applies the SourceIn tint when given.
pub fn rasterise(path: &Path, size_px: u32, tint: Option<Color32>) -> Result<ColorImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let pixels = (size_px * 2).max(1);
    let is_svg = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    let mut image = if is_svg {
        render_svg(&bytes, pixels)?
    } else {
        let decoded = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
        let scaled = decoded
            .resize(pixels, pixels, image::imageops::FilterType::Lanczos3)
            .to_rgba8();
        ColorImage::from_rgba_unmultiplied(
            [scaled.width() as usize, scaled.height() as usize],
            scaled.as_raw(),
        )
    };
    if let Some(t) = tint {
        for p in &mut image.pixels {
            let a = p.a();
            *p = Color32::from_rgba_unmultiplied(t.r(), t.g(), t.b(), a);
        }
    }
    Ok(image)
}

fn render_svg(bytes: &[u8], pixels: u32) -> Result<ColorImage, String> {
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(|e| e.to_string())?;
    let natural = tree.size();
    let scale = (pixels as f32 / natural.width()).min(pixels as f32 / natural.height());
    let fitted_w = natural.width() * scale;
    let fitted_h = natural.height() * scale;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(pixels, pixels).ok_or("empty pixmap")?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(
        (pixels as f32 - fitted_w) / 2.0,
        (pixels as f32 - fitted_h) / 2.0,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(ColorImage::from_rgba_premultiplied(
        [pixels as usize, pixels as usize],
        pixmap.data(),
    ))
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    icon: Icon,
    size: u32,
    tint: Option<Color32>,
    source: PathBuf,
}

/// Rasterised icon textures, keyed by icon, size, tint and source file.
#[derive(Default)]
pub struct IconCache {
    textures: HashMap<Key, TextureHandle>,
    last_tint: HashMap<Icon, Option<Color32>>,
}

impl IconCache {
    /// The texture for `icon` at the logical `size_px` under `tokens`.
    pub fn texture(
        &mut self,
        ctx: &Context,
        icon: Icon,
        size_px: f32,
        tokens: &Tokens,
    ) -> SizedTexture {
        let logical = icon.logical_name();
        let custom = tokens.icon_overrides.get(logical).cloned();
        let tint = match (&custom, tokens.icon_tints.get(logical)) {
            (_, Some(t)) if icon != Icon::Favicon => Some(*t),
            (Some(_), None) => None,
            (None, None) if icon.is_brand() => None,
            (None, None) => Some(tokens.icon_tint_global),
            _ => None,
        };
        let source = custom.unwrap_or_else(|| icon.bundled_path(&crate::assets_dir()));
        let size = size_px.round().max(1.0) as u32;
        let key = Key {
            icon,
            size,
            tint,
            source,
        };
        self.last_tint.insert(icon, tint);
        let handle = self.textures.entry(key.clone()).or_insert_with(|| {
            let image = rasterise(&key.source, key.size, key.tint).unwrap_or_else(|e| {
                tracing::warn!("icon {logical}: {e}");
                ColorImage::filled([1, 1], Color32::TRANSPARENT)
            });
            ctx.load_texture(
                format!("icon:{logical}:{size}:{tint:?}"),
                image,
                TextureOptions::LINEAR,
            )
        });
        SizedTexture::new(handle.id(), Vec2::splat(size as f32 * 2.0))
    }

    /// An `egui::Image` sized to `size_px` logical pixels.
    pub fn image(
        &mut self,
        ctx: &Context,
        icon: Icon,
        size_px: f32,
        tokens: &Tokens,
    ) -> egui::Image<'static> {
        let tex = self.texture(ctx, icon, size_px, tokens);
        egui::Image::from_texture(tex).fit_to_exact_size(Vec2::splat(size_px))
    }

    /// The tint last used for `icon` (tests).
    pub fn last_tint(&self, icon: Icon) -> Option<Color32> {
        self.last_tint.get(&icon).copied().flatten()
    }

    /// Drops every texture (Apply theme).
    pub fn clear(&mut self) {
        self.textures.clear();
        self.last_tint.clear();
    }
}
