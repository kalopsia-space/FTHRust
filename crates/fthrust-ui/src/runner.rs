//! The window event loop: winit + egui-winit + egui-wgpu. The services tick
//! on every loop wake (events from their threads, timers), not only when a
//! frame is painted, so hotkeys, saves and engine events keep flowing while
//! the window is hidden or the screen is static.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{ViewportBuilder, ViewportId, ViewportInfo};
use egui_wgpu::wgpu::Backends;
use egui_wgpu::winit::Painter;
use egui_wgpu::{RendererOptions, WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::App;
use crate::services::Services;
use crate::services::instance::Instance;
use crate::state::AppState;

/// Loop wake-ups from other threads.
#[derive(Debug)]
pub enum Wake {
    /// egui asked for a repaint after `delay`.
    Repaint(Duration),
    /// A service thread queued an event: tick now.
    Tick,
}

/// Services tick at least this often even with no events.
pub const HEARTBEAT: Duration = Duration::from_secs(1);

struct Runner {
    ctx: egui::Context,
    proxy: EventLoopProxy<Wake>,
    viewport: ViewportBuilder,
    pending: Option<(AppState, Option<Instance>)>,
    window: Option<Arc<Window>>,
    winit_state: Option<egui_winit::State>,
    painter: Option<Painter>,
    app: Option<App>,
    info: ViewportInfo,
    first_frame: bool,
    next_repaint: Option<Instant>,
    next_heartbeat: Instant,
    error: Option<String>,
}

/// The app's egui context. egui's own Ctrl+Q quit and Ctrl+plus/minus
/// zoom are off: Ctrl+Q is the editor's quick crop (egui closed the app
/// whenever the editor skipped it, e.g. while a clip loads), and the
/// reference has no keyboard zoom.
pub fn new_context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.options_mut(|o| {
        o.quit_shortcuts.clear();
        o.zoom_with_keyboard = false;
    });
    ctx
}

/// Runs the application until the window closes.
pub fn run(
    state: AppState,
    instance: Option<Instance>,
    viewport: ViewportBuilder,
) -> Result<(), String> {
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    let proxy: EventLoopProxy<Wake> = event_loop.create_proxy();
    let ctx = new_context();
    {
        let proxy = proxy.clone();
        ctx.set_request_repaint_callback(move |info| {
            let _ = proxy.send_event(Wake::Repaint(info.delay));
        });
    }
    let mut runner = Runner {
        ctx,
        proxy,
        viewport,
        pending: Some((state, instance)),
        window: None,
        winit_state: None,
        painter: None,
        app: None,
        info: ViewportInfo::default(),
        first_frame: true,
        next_repaint: None,
        next_heartbeat: Instant::now() + HEARTBEAT,
        error: None,
    };
    event_loop.run_app(&mut runner).map_err(|e| e.to_string())?;
    match runner.error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Vulkan only: egui-wgpu's default (`PRIMARY | GL`) also probes EGL, which
/// maps libgallium (~34 MB resident) for an adapter we never use.
/// `WGPU_BACKEND` still overrides.
fn preferred_backends() -> Backends {
    preferred_backends_from(std::env::var("WGPU_BACKEND").ok().as_deref())
}

fn preferred_backends_from(env: Option<&str>) -> Backends {
    match env.map(str::trim) {
        Some(v) if !v.is_empty() => Backends::from_comma_list(v),
        _ => Backends::VULKAN,
    }
}

fn wgpu_config(backends: Backends) -> WgpuConfiguration {
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = backends;
    WgpuConfiguration {
        wgpu_setup: WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}

/// The display handle is what lets the GL backend create a Wayland-compatible
/// context, which matters for the OpenGL fallback.
fn new_painter(ctx: &egui::Context, el: &ActiveEventLoop, backends: Backends) -> Painter {
    let mut config = wgpu_config(backends);
    if let WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup {
        setup.display_handle = Some(Box::new(el.owned_display_handle()));
    }
    pollster::block_on(Painter::new(
        ctx.clone(),
        config,
        false,
        RendererOptions::default(),
    ))
}

impl Runner {
    fn create_window(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
        let attrs = egui_winit::create_winit_window_attributes(&self.ctx, self.viewport.clone());
        let window = Arc::new(el.create_window(attrs).map_err(|e| e.to_string())?);
        egui_winit::apply_viewport_builder_to_window(&self.ctx, &window, &self.viewport);
        let first = preferred_backends();
        let mut painter = new_painter(&self.ctx, el, first);
        if let Err(e) =
            pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))
        {
            if first == Backends::GL {
                return Err(e.to_string());
            }
            tracing::info!(error = %e, "Vulkan setup failed; falling back to OpenGL");
            painter = new_painter(&self.ctx, el, Backends::GL);
            pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))
                .map_err(|e2| format!("vulkan: {e}; gl: {e2}"))?;
        }
        let mut winit_state = egui_winit::State::new(
            self.ctx.clone(),
            ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            painter.max_texture_side(),
        );
        if let Some(side) = painter.max_texture_side() {
            winit_state.set_max_texture_side(side);
        }
        egui_winit::update_viewport_info(&mut self.info, &self.ctx, &window, true);
        let (state, instance) = self.pending.take().ok_or("already started")?;
        let mut app = App::new(&self.ctx, state);
        app.render_state = painter.render_state();
        let waker = {
            let proxy = self.proxy.clone();
            Arc::new(move || {
                let _ = proxy.send_event(Wake::Tick);
            }) as Arc<dyn Fn() + Send + Sync>
        };
        let mut services = Services::start_with_waker(&app.state, &self.ctx, Some(waker));
        if let Some(i) = instance {
            services.attach_instance(i);
        }
        services.start_engine(&app.state);
        app.services = Some(services);
        self.app = Some(app);
        self.window = Some(window.clone());
        self.winit_state = Some(winit_state);
        self.painter = Some(painter);
        window.request_redraw();
        Ok(())
    }

    fn paint(&mut self, el: &ActiveEventLoop) {
        let (Some(window), Some(state), Some(painter), Some(app)) = (
            self.window.clone(),
            self.winit_state.as_mut(),
            self.painter.as_mut(),
            self.app.as_mut(),
        ) else {
            return;
        };
        egui_winit::update_viewport_info(&mut self.info, &self.ctx, &window, self.first_frame);
        self.first_frame = false;
        let mut raw = state.take_egui_input(&window);
        raw.viewports.insert(ViewportId::ROOT, self.info.clone());
        self.info.events.clear();
        let mut full = self.ctx.run_ui(raw, |ui| app.ui(ui));
        if std::mem::take(&mut app.state.clipboard_read_requested) {
            app.state.clipboard_paste = Some(state.clipboard_text().unwrap_or_default());
            window.request_redraw();
        }
        let clear = app.clear_color();
        state.handle_platform_output(&window, full.platform_output);
        let clipped = self.ctx.tessellate(full.shapes, full.pixels_per_point);
        painter.paint_and_update_textures(
            ViewportId::ROOT,
            full.pixels_per_point,
            clear,
            &clipped,
            &mut full.textures_delta,
            Vec::new(),
            &window,
        );
        if let Some(out) = full.viewport_output.remove(&ViewportId::ROOT) {
            let mut actions = Vec::new();
            egui_winit::process_viewport_commands(
                &self.ctx,
                &mut self.info,
                out.commands,
                &window,
                &mut actions,
            );
            if self.info.events.contains(&egui::ViewportEvent::Close) {
                tracing::info!("window closed by the app");
                el.exit();
                return;
            }
            if out.repaint_delay.is_zero() {
                window.request_redraw();
            } else if out.repaint_delay < Duration::from_secs(3600) {
                let at = Instant::now() + out.repaint_delay;
                self.next_repaint = Some(self.next_repaint.map_or(at, |n| n.min(at)));
            }
        }
    }

    fn tick(&mut self) {
        if let Some(app) = self.app.as_mut() {
            app.tick();
        }
    }

    fn schedule(&mut self, el: &ActiveEventLoop) {
        let mut deadline = self.next_heartbeat;
        if let Some(r) = self.next_repaint {
            deadline = deadline.min(r);
        }
        el.set_control_flow(ControlFlow::WaitUntil(deadline));
    }
}

impl ApplicationHandler<Wake> for Runner {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(e) = self.create_window(el)
        {
            self.error = Some(e);
            el.exit();
        }
    }

    fn new_events(&mut self, el: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. }) {
            let now = Instant::now();
            if now >= self.next_heartbeat {
                self.next_heartbeat = now + HEARTBEAT;
                self.tick();
            }
            if self.next_repaint.is_some_and(|r| now >= r) {
                self.next_repaint = None;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
        }
        self.schedule(el);
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: Wake) {
        match event {
            Wake::Tick => self.tick(),
            Wake::Repaint(delay) => {
                // Service threads wake us through egui's repaint request: tick
                // now, paint when the compositor lets us.
                self.tick();
                if delay.is_zero() {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                } else {
                    let at = Instant::now() + delay;
                    self.next_repaint = Some(self.next_repaint.map_or(at, |n| n.min(at)));
                }
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if let Some(state) = self.winit_state.as_mut() {
            let response = state.on_window_event(&window, &event);
            if response.repaint {
                window.request_redraw();
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                tracing::info!("window close requested by the compositor");
                el.exit();
            }
            WindowEvent::Resized(size) => {
                if let (Some(p), Some(w), Some(h)) = (
                    self.painter.as_mut(),
                    std::num::NonZeroU32::new(size.width),
                    std::num::NonZeroU32::new(size.height),
                ) {
                    p.on_window_resized(ViewportId::ROOT, w, h);
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.paint(el),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.schedule(el);
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        if let Some(mut app) = self.app.take() {
            app.on_exit();
        }
        if let Some(mut p) = self.painter.take() {
            p.destroy();
        }
        self.winit_state = None;
        self.window = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_wgpu::WgpuSetup;
    use egui_wgpu::wgpu::Backends;

    #[test]
    fn vulkan_only_unless_the_variable_says_otherwise() {
        assert_eq!(preferred_backends_from(None), Backends::VULKAN);
        assert_eq!(preferred_backends_from(Some("gl")), Backends::GL);
        assert_eq!(preferred_backends_from(Some("")), Backends::VULKAN);
        assert_eq!(preferred_backends_from(Some("  \t")), Backends::VULKAN);
        assert_eq!(
            preferred_backends_from(Some("vulkan,gl")),
            Backends::VULKAN | Backends::GL
        );
    }

    #[test]
    fn config_carries_the_requested_backends() {
        let WgpuSetup::CreateNew(setup) = wgpu_config(Backends::GL).wgpu_setup else {
            panic!("expected CreateNew");
        };
        assert_eq!(setup.instance_descriptor.backends, Backends::GL);
    }
}
