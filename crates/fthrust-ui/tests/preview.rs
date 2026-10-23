//! The editor's live crop / stretch / colour preview (notes section 16).

use std::sync::Arc;

use egui::mutex::RwLock;
use egui::{Pos2, Rect, Vec2};
use egui_wgpu::RenderState;
use egui_wgpu::wgpu;
use fthrust_core::editor::snapshot::Effects;
use fthrust_ui::editor::preview::{self, PreviewRenderer, Uniforms};
use fthrust_ui::editor::{Editor, Lifecycle};
use fthrust_ui::state::AppState;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

fn fx() -> Effects {
    Effects {
        exposure: 50,
        contrast: -50,
        saturation: 20,
        temperature: -30,
        sharpness: 40,
        vignette: 100,
    }
}

#[test]
fn uniform_mapping_matches_reference() {
    let u = Uniforms::from_edit(Some((192, 108, 960, 540)), (1920, 1080), &fx(), 1.25, true);
    assert!(close(u.exposure, 0.15));
    assert!(close(u.contrast, 0.7));
    assert!(close(u.saturation, 1.2));
    assert!(close(u.temperature, -0.09));
    assert!(close(u.sharpness, 0.6));
    assert!(close(u.vignette, 100.0 * 1.8 / 255.0));
    assert_eq!(u.crop, [0.1, 0.1, 0.5, 0.5]);
    assert!(u.crop_active);
    assert!(u.effects_active());
    assert!(close(u.stretch, 1.25));
    assert_eq!(
        preview::crop_label((960, 540)),
        "CROP  \u{b7}  960 \u{d7} 540"
    );
    let plain = Uniforms::from_edit(None, (1920, 1080), &Effects::default(), 1.0, true);
    assert!(!plain.effects_active());
    assert!(!plain.crop_active);
    assert_eq!(plain.crop, [0.0, 0.0, 1.0, 1.0]);
    // Colour preview off: identity colour, crop and stretch kept.
    let off = Uniforms::from_edit(Some((192, 108, 960, 540)), (1920, 1080), &fx(), 1.25, false);
    assert!(!off.effects_active());
    assert!(off.crop_active);
    assert!(close(off.stretch, 1.25));
}

#[test]
fn display_rect_follows_stretch() {
    let surface = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 500.0));
    let r = preview::display_rect(surface, (1920, 1080), 1.0);
    assert!(
        (r.width() - 888.888_9).abs() < 0.01 && (r.height() - 500.0).abs() < 0.01,
        "{r:?}"
    );
    assert!((r.center().x - 500.0).abs() < 0.01);
    let r = preview::display_rect(surface, (1920, 1080), 1.5);
    assert!((r.width() - 1000.0).abs() < 0.01, "{r:?}");
    assert!(
        (r.height() - 1000.0 / (1920.0 * 1.5 / 1080.0)).abs() < 0.01,
        "{r:?}"
    );
    assert!((r.center().y - 250.0).abs() < 0.01);
}

#[test]
fn crop_overlay_geometry() {
    let display = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(1000.0, 562.5));
    let o = preview::crop_overlay(display, (1920, 1080), (480, 10, 960, 540));
    let s = 1000.0 / 1920.0;
    let crop = Rect::from_min_size(
        Pos2::new(480.0 * s, 10.0 * s),
        Vec2::new(960.0 * s, 540.0 * s),
    );
    let near =
        |a: Rect, b: Rect| (a.min - b.min).length() < 0.01 && (a.max - b.max).length() < 0.01;
    assert!(near(o.crop, crop), "{:?}", o.crop);
    // Four veils around the crop, inside the picture.
    assert!(near(
        o.veils[0],
        Rect::from_min_max(display.min, Pos2::new(display.max.x, crop.min.y))
    ));
    assert!(near(
        o.veils[1],
        Rect::from_min_max(Pos2::new(display.min.x, crop.max.y), display.max)
    ));
    assert!(near(
        o.veils[2],
        Rect::from_min_max(
            Pos2::new(display.min.x, crop.min.y),
            Pos2::new(crop.min.x, crop.max.y)
        )
    ));
    assert!(near(
        o.veils[3],
        Rect::from_min_max(
            Pos2::new(crop.max.x, crop.min.y),
            Pos2::new(display.max.x, crop.max.y)
        )
    ));
    // The pill sits 28 px above the crop, but never above the picture top + 6.
    assert!((o.pill.min.y - 6.0).abs() < 0.01, "{:?}", o.pill);
    assert!((o.pill.height() - 22.0).abs() < 0.01);
    assert!((o.pill.min.x - (crop.min.x + 6.0)).abs() < 0.01);
    assert_eq!(o.label, "CROP  \u{b7}  960 \u{d7} 540");
    // Lower down, the pill is 28 px above the crop's top.
    let o = preview::crop_overlay(display, (1920, 1080), (480, 400, 960, 540));
    assert!(
        (o.pill.min.y - (400.0 * s - 28.0)).abs() < 0.01,
        "{:?}",
        o.pill
    );
}

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
        label: Some("preview-test"),
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

#[test]
fn shader_output_matches_cpu_formula() {
    let Some(rs) = headless() else {
        return;
    };
    const W: u32 = 8;
    const H: u32 = 6;
    let pixels: Vec<[u8; 4]> = (0..W * H)
        .map(|i| {
            let (x, y) = (i % W, i / W);
            [
                (x * 31 + 7) as u8,
                (y * 40 + 20) as u8,
                ((x * y * 13) % 256) as u8,
                255,
            ]
        })
        .collect();
    let frame = fthrust_media::playback::video::Frame {
        pts_ms: 0,
        width: W,
        height: H,
        rgba: pixels.concat(),
    };
    let mut tex = fthrust_ui::video_texture::VideoTexture::new();
    tex.update(&rs, &frame);
    let u = Uniforms::from_edit(Some((1, 1, 6, 4)), (W, H), &fx(), 1.0, true);
    let renderer = PreviewRenderer::new(&rs.device, wgpu::TextureFormat::Rgba8Unorm);
    let got = renderer.render_readback(
        &rs.device,
        &rs.queue,
        tex.view_unorm().unwrap(),
        (W, H),
        &u,
        rs.target_format,
    );
    let want = preview::cpu_reference(&pixels, W, H, &u);
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        for c in 0..3 {
            assert!(
                (i32::from(g[c]) - i32::from(w[c])).abs() <= 2,
                "pixel {i} channel {c}: gpu {g:?} cpu {w:?}"
            );
        }
    }
    // A pixel outside the crop is graded but not vignetted.
    assert_ne!(want[0], pixels[0]);
}

fn editor_state(settings: serde_json::Value) -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), settings);
    let mut e = Editor::for_tests(&tmp.path().join("a.mp4"), 10_000, Lifecycle::Paused);
    e.crop = Some((10, 20, 300, 200));
    e.effects = fx();
    e.stretch = 1.3;
    s.editor = Some(e);
    (tmp, s)
}

#[test]
fn gates_disable_preview() {
    let (_t, s) = editor_state(serde_json::json!({}));
    let u = preview::uniforms_for(&s).expect("live preview on by default");
    assert!(u.effects_active() && u.crop_active && close(u.stretch, 1.3));
    let (_t, s) = editor_state(serde_json::json!({"clip_editor_live_preview": false}));
    assert!(preview::uniforms_for(&s).is_none(), "original video");
    let (_t, s) = editor_state(serde_json::json!({"editor_color_preview_enabled": false}));
    let u = preview::uniforms_for(&s).unwrap();
    assert!(!u.effects_active(), "no colour");
    assert!(
        u.crop_active && close(u.stretch, 1.3),
        "crop and stretch still shown"
    );
}
