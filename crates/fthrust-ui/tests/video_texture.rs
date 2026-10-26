//! Plan 6a Task 5: decoded frames become egui textures through wgpu.

use std::sync::Arc;

use egui::mutex::RwLock;

use egui_wgpu::RenderState;
use egui_wgpu::wgpu;
use fthrust_media::playback::video::Frame;
use fthrust_ui::video_texture::VideoTexture;

fn headless() -> Option<RenderState> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }));
    let Ok(adapter) = adapter else {
        eprintln!("skipping: no wgpu adapter");
        return None;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("video-texture-test"),
        ..Default::default()
    }))
    .ok()?;
    let target_format = wgpu::TextureFormat::Rgba8Unorm;
    let renderer = egui_wgpu::Renderer::new(
        &device,
        target_format,
        egui_wgpu::RendererOptions::default(),
    );
    Some(RenderState {
        available_adapters: vec![],
        adapter,
        instance,
        device,
        queue,
        target_format,
        renderer: Arc::new(RwLock::new(renderer)),
    })
}

fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Frame {
    Frame {
        pts_ms: 0,
        width,
        height,
        rgba: rgba.repeat((width * height) as usize),
    }
}

/// The first pixel of the texture, read back through a staging buffer.
fn first_pixel(rs: &RenderState, texture: &wgpu::Texture) -> [u8; 4] {
    let buffer = rs.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = rs
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    rs.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    rs.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map result").expect("map");
    let view = buffer.slice(..).get_mapped_range().expect("mapped range");
    let px = [view[0], view[1], view[2], view[3]];
    drop(view);
    buffer.unmap();
    px
}

#[test]
fn video_texture_uploads_and_resizes() {
    let Some(rs) = headless() else {
        return;
    };
    let mut tex = VideoTexture::new();
    assert_eq!(tex.id(), None);
    assert_eq!(tex.size(), None);

    tex.update(&rs, &solid(320, 180, [255, 0, 0, 255]));
    let first = tex.id().expect("registered");
    assert_eq!(tex.size(), Some((320, 180)));
    assert_eq!(first_pixel(&rs, tex.texture().unwrap()), [255, 0, 0, 255]);

    // Same size: the texture is rewritten in place and keeps its id.
    tex.update(&rs, &solid(320, 180, [0, 255, 0, 255]));
    assert_eq!(tex.id(), Some(first));
    assert_eq!(first_pixel(&rs, tex.texture().unwrap()), [0, 255, 0, 255]);

    // New size: a new texture and a new id.
    tex.update(&rs, &solid(640, 360, [0, 0, 255, 255]));
    assert_eq!(tex.size(), Some((640, 360)));
    assert_ne!(tex.id(), Some(first));
    assert_eq!(first_pixel(&rs, tex.texture().unwrap()), [0, 0, 255, 255]);

    tex.clear(&rs);
    assert_eq!(tex.id(), None);
    assert_eq!(tex.size(), None);
}

/// Draws `tex` full-frame through egui into an `Rgba8Unorm` target (as the
/// app's swapchain is) and reads back the centre pixel.
fn drawn_centre(rs: &RenderState, tex: &VideoTexture) -> [u8; 4] {
    const SIZE: u32 = 64;
    let ctx = egui::Context::default();
    let id = tex.id().expect("registered");
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(SIZE as f32, SIZE as f32),
        )),
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| {
        ui.painter().image(
            id,
            ui.max_rect().expand(8.0),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    });
    let jobs = ctx.tessellate(std::mem::take(&mut out.shapes), 1.0);
    let target = rs.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: rs.target_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [SIZE, SIZE],
        pixels_per_point: 1.0,
    };
    let mut encoder = rs
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut renderer = rs.renderer.write();
        for (tid, deltas) in &out.textures_delta.set {
            for delta in deltas {
                renderer.update_texture(&rs.device, &rs.queue, *tid, delta);
            }
        }
        out.textures_delta.clear();
        renderer.update_buffers(&rs.device, &rs.queue, &mut encoder, &jobs, &screen);
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("draw"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
    }
    rs.queue.submit([encoder.finish()]);
    // Copy the centre texel to the corner of a 1×1 texture for readback.
    let probe = rs.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("probe"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: rs.target_format,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut encoder = rs
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: SIZE / 2,
                y: SIZE / 2,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: &probe,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    rs.queue.submit([encoder.finish()]);
    first_pixel(rs, &probe)
}

#[test]
fn video_frames_draw_at_their_own_brightness() {
    let Some(rs) = headless() else {
        return;
    };
    let mut tex = VideoTexture::new();
    for grey in [64u8, 128, 192] {
        tex.update(&rs, &solid(32, 32, [grey, grey, grey, 255]));
        let px = drawn_centre(&rs, &tex);
        assert!(
            (i32::from(px[0]) - i32::from(grey)).abs() <= 2,
            "grey {grey} drew as {px:?}: the frame went through gamma twice"
        );
    }
}
