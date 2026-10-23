//! A decoded frame as an egui texture: one RGBA wgpu texture rewritten in
//! place while the size holds, recreated (and re-registered with egui's
//! renderer) when it changes.

use egui_wgpu::RenderState;
use egui_wgpu::wgpu;
use fthrust_media::playback::video::Frame;

/// The editor's video surface.
#[derive(Default)]
pub struct VideoTexture {
    texture: Option<wgpu::Texture>,
    id: Option<egui::TextureId>,
    size: Option<(u32, u32)>,
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
        if self.size != Some((frame.width, frame.height)) {
            self.clear(rs);
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
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            self.texture = Some(texture);
            self.id = Some(id);
            self.size = Some((frame.width, frame.height));
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

    /// Frees the texture and unregisters it.
    pub fn clear(&mut self, rs: &RenderState) {
        if let Some(id) = self.id.take() {
            rs.renderer.write().free_texture(&id);
        }
        self.texture = None;
        self.size = None;
    }
}
