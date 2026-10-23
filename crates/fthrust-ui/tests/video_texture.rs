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
