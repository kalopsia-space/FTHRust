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

/// The GPU side of a thread's overlays, kept for the thread's lifetime:
/// one Vulkan device and one egui renderer for every surface it shows.
/// egui hands a Context's font atlas to a renderer once, so the old
/// renderer-per-card (with one Context per thread) drew the second and
/// later cards without any text ("Missing texture") or panicked on a
/// partial atlas update; it also built a Vulkan instance, adapter and
/// device for every card. After a GPU error, [`reset`](Self::reset) it and
/// start a fresh egui Context, since a new renderer has none of the old
/// one's textures.
pub struct OverlayGpu {
    instance: wgpu::Instance,
    device: Option<GpuDevice>,
}

struct GpuDevice {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    format: wgpu::TextureFormat,
}

impl Default for OverlayGpu {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayGpu {
    /// A Vulkan instance; the device comes with the first surface.
    pub fn new() -> Self {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::VULKAN;
        OverlayGpu {
            instance: wgpu::Instance::new(desc),
            device: None,
        }
    }

    /// Drops the device and renderer (after a GPU error).
    pub fn reset(&mut self) {
        self.device = None;
    }

    /// Whether a device is up (tests).
    pub fn has_device(&self) -> bool {
        self.device.is_some()
    }

    fn device_for(&mut self, surface: &wgpu::Surface<'_>) -> Result<&mut GpuDevice, OverlayError> {
        if let Some(d) = &self.device
            && !d.adapter.is_surface_supported(surface)
        {
            return Err(OverlayError::Gpu(
                "the overlay GPU cannot present to this output".into(),
            ));
        }
        if self.device.is_none() {
            let adapter =
                pollster::block_on(self.instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: Some(surface),
                    force_fallback_adapter: false,
                    apply_limit_buckets: false,
                }))
                .map_err(|e| OverlayError::Gpu(e.to_string()))?;
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
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
            let renderer =
                egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
            self.device = Some(GpuDevice {
                adapter,
                device,
                queue,
                renderer,
                format,
            });
        }
        self.device
            .as_mut()
            .ok_or_else(|| OverlayError::Gpu("no device".into()))
    }
}

/// One layer surface and its swapchain; drawn with a thread's
/// [`OverlayGpu`].
pub struct OverlaySurface {
    // Drop order: the GPU surface and its swapchain go before the
    // wl_surface they were created on.
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    layer: LayerSurface,
    size: (u32, u32),
}

impl OverlaySurface {
    /// Creates a `size` (physical px) overlay on `output` with `margins`
    /// (top, right, bottom, left).
    pub fn new(
        gpu: &mut OverlayGpu,
        output: Option<&str>,
        size: (u32, u32),
        margins: (i32, i32, i32, i32),
    ) -> Result<Self, OverlayError> {
        OverlaySurface::new_placed(
            gpu,
            output,
            fthrust_wayland::layer_shell::Placement::Corner { size, margins },
            "fthrust-capture-card",
        )
    }

    /// Creates an overlay with any placement and layer namespace.
    pub fn new_placed(
        gpu: &mut OverlayGpu,
        output: Option<&str>,
        placement: fthrust_wayland::layer_shell::Placement,
        namespace: &str,
    ) -> Result<Self, OverlayError> {
        let layer = LayerSurface::create_placed(output, placement, namespace)?;
        let size = layer.size();
        let handles = layer.handles();
        // The handles point at libwayland objects owned by `layer`; the
        // field order drops the GPU surface before the wl_surface.
        let surface = gpu
            .instance
            .create_surface(handles)
            .map_err(|e| OverlayError::Gpu(e.to_string()))?;
        let dev = gpu.device_for(&surface)?;
        let caps = surface.get_capabilities(&dev.adapter);
        if !caps.formats.contains(&dev.format) {
            return Err(OverlayError::Gpu(format!(
                "this output does not take {:?}",
                dev.format
            )));
        }
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
            format: dev.format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.0.max(1),
            height: size.1.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&dev.device, &config);
        Ok(OverlaySurface {
            surface,
            config,
            layer,
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
    pub fn pump(&mut self, gpu: &OverlayGpu) -> bool {
        let alive = self.layer.pump(Duration::ZERO);
        let new = self.layer.size();
        if new != self.size
            && new.0 > 0
            && new.1 > 0
            && let Some(dev) = &gpu.device
        {
            self.size = new;
            self.config.width = new.0;
            self.config.height = new.1;
            self.surface.configure(&dev.device, &self.config);
        }
        alive
    }

    /// Renders one frame with `ctx` (the thread's Context, fonts
    /// installed): `draw` paints into the full surface rect.
    pub fn render(
        &mut self,
        gpu: &mut OverlayGpu,
        ctx: &Context,
        time: f64,
        draw: impl FnOnce(&Context, &egui::Painter, Rect),
    ) -> Result<(), OverlayError> {
        let Some(dev) = gpu.device.as_mut() else {
            return Err(OverlayError::Gpu("no device".into()));
        };
        let rect = Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::new(self.size.0 as f32, self.size.1 as f32),
        );
        let raw = egui::RawInput {
            screen_rect: Some(rect),
            time: Some(time),
            ..Default::default()
        };
        let ctx = ctx.clone();
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
                dev.renderer
                    .update_texture(&dev.device, &dev.queue, *id, delta);
            }
        }
        let result = self.draw_frame(dev, &jobs, output.pixels_per_point);
        for id in &textures_delta.free {
            dev.renderer.free_texture(id);
        }
        textures_delta.clear();
        result
    }

    fn draw_frame(
        &mut self,
        dev: &mut GpuDevice,
        jobs: &[egui::ClippedPrimitive],
        pixels_per_point: f32,
    ) -> Result<(), OverlayError> {
        use wgpu::CurrentSurfaceTexture as Cur;
        let frame = match self.surface.get_current_texture() {
            Cur::Success(f) | Cur::Suboptimal(f) => f,
            Cur::Outdated | Cur::Lost => {
                self.surface.configure(&dev.device, &self.config);
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
        let mut encoder = dev
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fthrust-overlay"),
            });
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point,
        };
        let user_cmds =
            dev.renderer
                .update_buffers(&dev.device, &dev.queue, &mut encoder, jobs, &screen);
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
            dev.renderer.render(&mut pass, jobs, &screen);
        }
        dev.queue.submit(
            user_cmds
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        dev.queue.present(frame);
        Ok(())
    }
}
