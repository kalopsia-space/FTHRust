//! A decoded frame as an egui texture: one RGBA wgpu texture rewritten in
//! place while the size holds, recreated (and re-registered with egui's
//! renderer) when it changes, and unregistered when dropped.

use std::sync::Arc;

use egui::mutex::RwLock;
use egui_wgpu::RenderState;
use egui_wgpu::wgpu;
use fthrust_media::playback::video::Frame;

/// The editor's video surface.
#[derive(Default)]
pub struct VideoTexture {
    texture: Option<wgpu::Texture>,
    /// A non-sRGB view: the preview shader grades gamma-encoded values,
    /// as the reference's shader does.
    view_unorm: Option<wgpu::TextureView>,
    id: Option<egui::TextureId>,
    size: Option<(u32, u32)>,
    /// The renderer `id` is registered with, so that dropping the texture can
    /// unregister it: egui's renderer keeps a native texture (a bind group on
    /// its view) until told otherwise.
    renderer: Option<Arc<RwLock<egui_wgpu::Renderer>>>,
}

impl VideoTexture {
    /// No texture yet.
    pub fn new() -> VideoTexture {
        VideoTexture::default()
    }

    /// The egui texture id while a frame is loaded.
    pub fn id(&self) -> Option<egui::TextureId> {
        self.id
    }

    /// The loaded frame's size.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.size
    }

    /// The frame as gamma-encoded `Rgba8Unorm` (the preview shader's input).
    pub fn view_unorm(&self) -> Option<&wgpu::TextureView> {
        self.view_unorm.as_ref()
    }

    /// The wgpu texture (for readback tests).
    pub fn texture(&self) -> Option<&wgpu::Texture> {
        self.texture.as_ref()
    }

    /// Uploads `frame`, recreating the texture when its size changed.
    pub fn update(&mut self, rs: &RenderState, frame: &Frame) {
        if frame.width == 0 || frame.height == 0 {
            return;
        }
        let expected = (frame.width * frame.height * 4) as usize;
        if frame.rgba.len() < expected {
            tracing::warn!(
                len = frame.rgba.len(),
                expected,
                "video frame shorter than its size; skipped"
            );
            return;
        }
        // A texture belongs to the device it was made on: a render state that
        // is not the one it is registered with starts it over.
        let same_renderer = self
            .renderer
            .as_ref()
            .is_some_and(|r| Arc::ptr_eq(r, &rs.renderer));
        if self.size != Some((frame.width, frame.height)) || !same_renderer {
            self.clear();
            let texture = rs.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("fthrust-video"),
                size: wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // The frame's bytes are gamma-encoded, and egui samples them
                // as such (`Rgba8Unorm`) and blends in gamma space; an sRGB
                // view would linearise the frame on sampling and apply gamma
                // twice (dark midtones). A plain `Rgba8Unorm` texture is read
                // the same way and needs no extra view format, which the GL
                // fallback lacks: it panicked here when the editor opened.
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let id = rs.renderer.write().register_native_texture(
                &rs.device,
                &view,
                wgpu::FilterMode::Linear,
            );
            self.view_unorm = Some(view);
            self.texture = Some(texture);
            self.id = Some(id);
            self.size = Some((frame.width, frame.height));
            self.renderer = Some(rs.renderer.clone());
        }
        let Some(texture) = &self.texture else {
            return;
        };
        rs.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.rgba[..expected],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(frame.width * 4),
                rows_per_image: Some(frame.height),
            },
            wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Frees the texture and unregisters it from the renderer it was
    /// registered with.
    pub fn clear(&mut self) {
        if let (Some(id), Some(renderer)) = (self.id.take(), self.renderer.take()) {
            renderer.write().free_texture(&id);
        }
        self.texture = None;
        self.view_unorm = None;
        self.size = None;
    }
}

impl Drop for VideoTexture {
    /// Closing or parking the editor drops its texture: without this the
    /// renderer kept the frame's GPU memory (8 MB at 1080p, 33 MB at 4K) for
    /// every editor session.
    fn drop(&mut self) {
        self.clear();
    }
}
