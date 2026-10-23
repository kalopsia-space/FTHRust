//! A layer-shell surface drawn by egui over wgpu, for the capture card.

use std::time::Duration;

use egui::{Context, Rect};
use egui_wgpu::wgpu;
use fthrust_wayland::layer_shell::{LayerError, LayerSurface};
use fthrust_wayland::outputs::OutputInfo;

/// Reference: `min(1.0, max(0.65, height / 1080))`.
pub fn scale_for(height: u32) -> f32 {
    (height as f32 / 1080.0).clamp(0.65, 1.0)
}

/// The named output, else the highest-refresh one.
pub fn pick_output<'a>(outputs: &'a [OutputInfo], wanted: &str) -> Option<&'a OutputInfo> {
    let done: Vec<&OutputInfo> = outputs.iter().filter(|o| o.done).collect();
    if !wanted.is_empty()
        && wanted != "auto"
        && let Some(o) = done.iter().find(|o| o.name == wanted)
    {
        return Some(o);
    }
    done.into_iter().max_by_key(|o| o.refresh_mhz)
}

/// Why the overlay could not be created.
#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    /// Layer shell.
    #[error(transparent)]
    Layer(#[from] LayerError),
    /// GPU.
    #[error("gpu: {0}")]
    Gpu(String),
}

/// The surface plus its renderer.
pub struct OverlaySurface {
    // Drop order: the GPU surface and its swapchain go before the
    // wl_surface they were created on.
    renderer: egui_wgpu::Renderer,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    layer: LayerSurface,
    ctx: Context,
    size: (u32, u32),
}

impl OverlaySurface {
    /// Creates a `size` (physical px) overlay on `output` with `margins`
    /// (top, right, bottom, left); `ctx` must have the app fonts installed.
    pub fn new(
        output: Option<&str>,
        size: (u32, u32),
        margins: (i32, i32, i32, i32),
        ctx: Context,
    ) -> Result<Self, OverlayError> {
        let layer = LayerSurface::create(output, size, margins, "fthrust-capture-card")?;
        let size = layer.size();
        let handles = layer.handles();
        let mut desc = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(handles));
        desc.backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(desc);
        // The handles point at libwayland objects owned by `layer`; the
        // field order drops the GPU surface before the wl_surface.
        let surface = instance
            .create_surface(handles)
            .map_err(|e| OverlayError::Gpu(e.to_string()))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| OverlayError::Gpu(e.to_string()))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("fthrust-overlay"),
            ..Default::default()
        }))
        .map_err(|e| OverlayError::Gpu(e.to_string()))?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| OverlayError::Gpu("no surface format".into()))?;
        let alpha_mode = if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            caps.alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto)
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.0.max(1),
            height: size.1.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        let renderer =
            egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
        ctx.set_pixels_per_point(1.0);
        Ok(OverlaySurface {
            layer,
            surface,
            device,
            queue,
            config,
            renderer,
            ctx,
            size,
        })
    }

    /// The surface size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The output the surface sits on.
    pub fn output_name(&self) -> &str {
        self.layer.output_name()
    }

    /// Processes Wayland events; `false` once the compositor closed us.
    pub fn pump(&mut self) -> bool {
        let alive = self.layer.pump(Duration::ZERO);
        let new = self.layer.size();
        if new != self.size && new.0 > 0 && new.1 > 0 {
            self.size = new;
            self.config.width = new.0;
            self.config.height = new.1;
            self.surface.configure(&self.device, &self.config);
        }
        alive
    }

    /// Renders one frame: `draw` paints into the full surface rect.
    pub fn render(
        &mut self,
        time: f64,
        draw: impl FnOnce(&Context, &egui::Painter, Rect),
    ) -> Result<(), OverlayError> {
        let rect = Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::new(self.size.0 as f32, self.size.1 as f32),
        );
        let raw = egui::RawInput {
            screen_rect: Some(rect),
            time: Some(time),
            ..Default::default()
        };
        let ctx = self.ctx.clone();
        ctx.begin_pass(raw);
        {
            let painter = egui::Painter::new(ctx.clone(), egui::LayerId::background(), rect);
            draw(&ctx, &painter, rect);
        }
        let output = ctx.end_pass();
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let mut textures_delta = output.textures_delta;
        for (id, deltas) in &textures_delta.set {
            for delta in deltas {
                self.renderer
                    .update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        let result = self.draw_frame(&jobs, output.pixels_per_point);
        for id in &textures_delta.free {
            self.renderer.free_texture(id);
        }
        textures_delta.clear();
        result
    }

    fn draw_frame(
        &mut self,
        jobs: &[egui::ClippedPrimitive],
        pixels_per_point: f32,
    ) -> Result<(), OverlayError> {
        use wgpu::CurrentSurfaceTexture as Cur;
        let frame = match self.surface.get_current_texture() {
            Cur::Success(f) | Cur::Suboptimal(f) => f,
            Cur::Outdated | Cur::Lost => {
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    Cur::Success(f) | Cur::Suboptimal(f) => f,
                    other => return Err(OverlayError::Gpu(format!("surface texture: {other:?}"))),
                }
            }
            Cur::Timeout | Cur::Occluded => return Ok(()),
            Cur::Validation => return Err(OverlayError::Gpu("surface validation error".into())),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fthrust-overlay"),
            });
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point,
        };
        let user_cmds =
            self.renderer
                .update_buffers(&self.device, &self.queue, &mut encoder, jobs, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("fthrust-overlay"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            self.renderer.render(&mut pass, jobs, &screen);
        }
        self.queue.submit(
            user_cmds
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        self.queue.present(frame);
        Ok(())
    }
}
