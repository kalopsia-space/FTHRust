//! Live preview of crop, stretch and colour (notes section 16): the video
//! keeps its full frame, stretched horizontally, with the area outside the
//! crop veiled and the crop outlined and labelled. Colour effects run
//! through a WGSL port of the reference's `color.frag`, only while any
//! effect is non-zero. `clip_editor_live_preview` off shows the original
//! video; `editor_color_preview_enabled` off drops the colour only.

use egui::{Pos2, Rect, Vec2};
use egui_wgpu::wgpu;
use fthrust_core::editor::snapshot::Effects;

use crate::state::AppState;

/// The shader's uniforms (`set_edit_state` mapping).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Uniforms {
    /// `v * 0.003`.
    pub exposure: f32,
    /// `1 + v * 0.006`.
    pub contrast: f32,
    /// `1 + v * 0.01`.
    pub saturation: f32,
    /// `v * 0.003`.
    pub temperature: f32,
    /// `v * 0.015`.
    pub sharpness: f32,
    /// `v * 1.8 / 255`.
    pub vignette: f32,
    /// Crop `(x, y, w, h)` as fractions of the source.
    pub crop: [f32; 4],
    /// A crop is set.
    pub crop_active: bool,
    /// Horizontal stretch.
    pub stretch: f32,
    /// The vignette colour (BG, gamma-encoded 0..1).
    pub background: [f32; 3],
}

impl Uniforms {
    /// The preview state for an edit; `color_enabled` false keeps crop and
    /// stretch but no colour.
    pub fn from_edit(
        crop: Option<(u32, u32, u32, u32)>,
        src: (u32, u32),
        effects: &Effects,
        stretch: f64,
        color_enabled: bool,
    ) -> Uniforms {
        let fx = if color_enabled {
            *effects
        } else {
            Effects::default()
        };
        let (w, h) = (src.0.max(1) as f32, src.1.max(1) as f32);
        let crop_frac = crop.map_or([0.0, 0.0, 1.0, 1.0], |(x, y, cw, ch)| {
            [x as f32 / w, y as f32 / h, cw as f32 / w, ch as f32 / h]
        });
        Uniforms {
            exposure: fx.exposure as f32 * 0.003,
            contrast: 1.0 + fx.contrast as f32 * 0.006,
            saturation: 1.0 + fx.saturation as f32 * 0.01,
            temperature: fx.temperature as f32 * 0.003,
            sharpness: fx.sharpness as f32 * 0.015,
            vignette: fx.vignette as f32 * 1.8 / 255.0,
            crop: crop_frac,
            crop_active: crop.is_some(),
            stretch: stretch as f32,
            background: [7.0 / 255.0, 10.0 / 255.0, 9.0 / 255.0],
        }
    }

    /// `effectsActive`: the shader runs only then.
    pub fn effects_active(&self) -> bool {
        self.exposure != 0.0
            || self.contrast != 1.0
            || self.saturation != 1.0
            || self.temperature != 0.0
            || self.sharpness != 0.0
            || self.vignette != 0.0
    }

    /// The uniform buffer: five `vec4<f32>`.
    fn to_bytes(self, size: (u32, u32), srgb_target: bool) -> Vec<u8> {
        let v: [f32; 20] = [
            self.exposure,
            self.contrast,
            self.saturation,
            self.temperature,
            self.sharpness,
            self.vignette,
            if self.crop_active { 1.0 } else { 0.0 },
            if srgb_target { 1.0 } else { 0.0 },
            self.crop[0],
            self.crop[1],
            self.crop[2],
            self.crop[3],
            self.background[0],
            self.background[1],
            self.background[2],
            0.0,
            size.0 as f32,
            size.1 as f32,
            0.0,
            0.0,
        ];
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }
}

/// `CROP  ·  {w} × {h}`.
pub fn crop_label((w, h): (u32, u32)) -> String {
    format!("CROP  \u{b7}  {w} \u{d7} {h}")
}

/// The gates and the open edit; `None` shows the original video.
pub fn uniforms_for(state: &AppState) -> Option<Uniforms> {
    let e = state.editor.as_ref()?;
    if !state
        .settings
        .get_bool("clip_editor_live_preview")
        .unwrap_or(true)
    {
        return None;
    }
    let color = state
        .settings
        .get_bool("editor_color_preview_enabled")
        .unwrap_or(true);
    let mut u = Uniforms::from_edit(e.crop, e.source_size(), &e.effects, e.stretch, color);
    let bg = state.tokens.color(crate::tokens::Color::Bg);
    u.background = [
        f32::from(bg.r()) / 255.0,
        f32::from(bg.g()) / 255.0,
        f32::from(bg.b()) / 255.0,
    ];
    Some(u)
}

/// The picture's rect: `(src_w × stretch) / src_h` fitted and centred.
pub fn display_rect(surface: Rect, src: (u32, u32), stretch: f64) -> Rect {
    let aspect = (src.0.max(1) as f64 * stretch.max(0.01)) / src.1.max(1) as f64;
    let (w, h) = (surface.width() as f64, surface.height() as f64);
    let size = if w / h > aspect {
        Vec2::new((h * aspect) as f32, h as f32)
    } else {
        Vec2::new(w as f32, (w / aspect) as f32)
    };
    Rect::from_center_size(surface.center(), size)
}

/// The crop overlay in screen space.
#[derive(Clone, Debug, PartialEq)]
pub struct CropOverlay {
    /// The crop rect.
    pub crop: Rect,
    /// Top, bottom, left, right veils.
    pub veils: [Rect; 4],
    /// The label pill (22 px high, 6 px from the crop corner, at most
    /// 28 px above it and never above the picture top + 6).
    pub pill: Rect,
    /// `CROP  ·  {w} × {h}`.
    pub label: String,
}

/// Pill width for the label at 12 px bold (measured by the caller when a
/// font is at hand; the geometry only needs its left, top and height).
const PILL_W: f32 = 150.0;

/// The overlay for `crop` (source pixels) over a picture at `display`.
pub fn crop_overlay(display: Rect, src: (u32, u32), crop: (u32, u32, u32, u32)) -> CropOverlay {
    let sx = display.width() / src.0.max(1) as f32;
    let sy = display.height() / src.1.max(1) as f32;
    let (x, y, w, h) = crop;
    let c = Rect::from_min_size(
        Pos2::new(display.min.x + x as f32 * sx, display.min.y + y as f32 * sy),
        Vec2::new(w as f32 * sx, h as f32 * sy),
    );
    let veils = [
        Rect::from_min_max(display.min, Pos2::new(display.max.x, c.min.y)),
        Rect::from_min_max(Pos2::new(display.min.x, c.max.y), display.max),
        Rect::from_min_max(
            Pos2::new(display.min.x, c.min.y),
            Pos2::new(c.min.x, c.max.y),
        ),
        Rect::from_min_max(
            Pos2::new(c.max.x, c.min.y),
            Pos2::new(display.max.x, c.max.y),
        ),
    ];
    let top = (display.min.y + 6.0).max(c.min.y - 28.0);
    CropOverlay {
        crop: c,
        veils,
        pill: Rect::from_min_size(Pos2::new(c.min.x + 6.0, top), Vec2::new(PILL_W, 22.0)),
        label: crop_label((w, h)),
    }
}

/// The shader's maths on the CPU (`color.frag`), for `pixels` (RGBA,
/// row-major, `w × h`) sampled at pixel centres with clamped edges.
pub fn cpu_reference(pixels: &[[u8; 4]], w: u32, h: u32, u: &Uniforms) -> Vec<[u8; 4]> {
    let at = |x: i64, y: i64| {
        let x = x.clamp(0, w as i64 - 1) as usize;
        let y = y.clamp(0, h as i64 - 1) as usize;
        pixels[y * w as usize + x]
    };
    let grade = |p: [u8; 4]| -> [f32; 3] {
        let c = [p[0], p[1], p[2]].map(|v| f32::from(v) / 255.0);
        let mut rgb = c.map(|v| (v - 0.5) * u.contrast + 0.5 + u.exposure);
        let luma = rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
        rgb = rgb.map(|v| luma + (v - luma) * u.saturation);
        rgb[0] *= 1.0 + u.temperature;
        rgb[2] *= 1.0 - u.temperature;
        rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() / 255.0)
    };
    let weights = [1.0f32, 4.0, 6.0, 4.0, 1.0];
    let (cw, ch) = (w as f32, h as f32);
    let crop = [
        u.crop[0] * cw,
        u.crop[1] * ch,
        u.crop[2] * cw,
        u.crop[3] * ch,
    ];
    let mut out = Vec::with_capacity(pixels.len());
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let mut rgb = grade(at(x, y));
            if u.sharpness > 0.0 {
                let mut blurred = [0.0f32; 3];
                for (j, wj) in weights.iter().enumerate() {
                    for (i, wi) in weights.iter().enumerate() {
                        let g = grade(at(x + i as i64 - 2, y + j as i64 - 2));
                        for c in 0..3 {
                            blurred[c] += g[c] * wi * wj;
                        }
                    }
                }
                for c in 0..3 {
                    rgb[c] = (rgb[c] * (1.0 + u.sharpness) - blurred[c] / 256.0 * u.sharpness)
                        .clamp(0.0, 1.0);
                }
            }
            if u.vignette > 0.0 {
                let p = (x as f32 + 0.5, y as f32 + 0.5);
                let inside = p.0 >= crop[0]
                    && p.0 <= crop[0] + crop[2]
                    && p.1 >= crop[1]
                    && p.1 <= crop[1] + crop[3];
                if inside {
                    let centre = (crop[0] + crop[2] / 2.0, crop[1] + crop[3] / 2.0);
                    let radius = crop[2].max(crop[3]) * 0.72;
                    let d = ((p.0 - centre.0).powi(2) + (p.1 - centre.1).powi(2)).sqrt() / radius;
                    let f = ((d - 0.35) / 0.65).clamp(0.0, 1.0) * u.vignette;
                    for (v, b) in rgb.iter_mut().zip(u.background) {
                        *v += (b - *v) * f;
                    }
                }
            }
            let px = rgb.map(|v| (v * 255.0).round() as u8);
            out.push([px[0], px[1], px[2], 255]);
        }
    }
    out
}

/// The colour pipeline.
pub struct PreviewRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
}

impl PreviewRenderer {
    /// Builds the pipeline for a `format` target.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> PreviewRenderer {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fthrust-preview"),
            source: wgpu::ShaderSource::Wgsl(include_str!("preview.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fthrust-preview-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fthrust-preview-pipeline-layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fthrust-preview"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fthrust-preview-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fthrust-preview-uniforms"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        PreviewRenderer {
            pipeline,
            layout,
            sampler,
            uniforms,
        }
    }

    fn bind_group(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fthrust-preview-bind-group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn write(
        &self,
        queue: &wgpu::Queue,
        u: &Uniforms,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) {
        queue.write_buffer(&self.uniforms, 0, &u.to_bytes(size, format.is_srgb()));
    }

    /// Renders `view` (a `size` frame) through the shader into a same-size
    /// texture and reads it back (tests).
    pub fn render_readback(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        size: (u32, u32),
        u: &Uniforms,
        format: wgpu::TextureFormat,
    ) -> Vec<[u8; 4]> {
        self.write(queue, u, size, format);
        let bind = self.bind_group(device, view);
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fthrust-preview-target"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let row = (size.0 * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fthrust-preview-readback"),
            size: u64::from(row * size.1),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fthrust-preview-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            extent,
        );
        queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let Ok(data) = slice.get_mapped_range() else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity((size.0 * size.1) as usize);
        for y in 0..size.1 {
            for x in 0..size.0 {
                let i = (y * row + x * 4) as usize;
                out.push([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            }
        }
        out
    }
}

/// The renderer and the frame's bind group, in egui-wgpu's callback
/// resources.
pub struct PreviewResources {
    renderer: PreviewRenderer,
    format: wgpu::TextureFormat,
    bind_group: Option<wgpu::BindGroup>,
}

/// Registers the pipeline once with egui's renderer.
pub fn install(rs: &egui_wgpu::RenderState) {
    let mut renderer = rs.renderer.write();
    if renderer
        .callback_resources
        .get::<PreviewResources>()
        .is_none()
    {
        renderer.callback_resources.insert(PreviewResources {
            renderer: PreviewRenderer::new(&rs.device, rs.target_format),
            format: rs.target_format,
            bind_group: None,
        });
    }
}

/// Draws the video through the shader inside egui's render pass.
pub struct PreviewCallback {
    /// The frame (gamma-encoded view).
    pub view: wgpu::TextureView,
    /// Its size.
    pub size: (u32, u32),
    /// The edit.
    pub uniforms: Uniforms,
}

impl egui_wgpu::CallbackTrait for PreviewCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(r) = resources.get_mut::<PreviewResources>() {
            r.renderer.write(queue, &self.uniforms, self.size, r.format);
            r.bind_group = Some(r.renderer.bind_group(device, &self.view));
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(r) = resources.get::<PreviewResources>()
            && let Some(bind) = &r.bind_group
        {
            pass.set_pipeline(&r.renderer.pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
