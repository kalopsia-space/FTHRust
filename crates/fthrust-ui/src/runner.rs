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
    /// SIGTERM, SIGINT or SIGHUP: shut down as if the window were closed.
    Quit,
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
    // Fidelity markers are only read by the bench's state dump.
    crate::fidelity::set_recording(crate::ui_dump::requested());
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    let proxy: EventLoopProxy<Wake> = event_loop.create_proxy();
    quit_signals::install(proxy.clone());
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
    let ran = event_loop.run_app(&mut runner).map_err(|e| e.to_string());
    // The window is gone by now (see `exiting`), so the bounded shutdown
    // waits (exports, a recording, a save in flight) don't leave a frozen
    // window on screen.
    let shut_down = match runner.app.take() {
        Some(mut app) => {
            app.on_exit();
            true
        }
        None => false,
    };
    match ran {
        // winit ends the loop with an error when the compositor goes away
        // (a logout, the compositor's own exit); the app has shut down in
        // order all the same.
        Err(e) if shut_down => tracing::info!("the window loop ended: {e}"),
        r => r?,
    }
    match runner.error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// SIGTERM (a logout, `systemctl stop`), SIGINT and SIGHUP end the event
/// loop like closing the window, so the usual shutdown runs: pending
/// settings are written, a save in flight finishes, the session is closed
/// cleanly. They used to kill the app on the spot. A second signal exits at
/// once, in case the shutdown itself is stuck.
mod quit_signals {
    use winit::event_loop::EventLoopProxy;

    use super::Wake;

    /// Listens on a thread of its own (tokio's handlers are reset in child
    /// processes, so the engine and helpers keep the default actions).
    pub fn install(proxy: EventLoopProxy<Wake>) {
        let spawned = std::thread::Builder::new()
            .name("fthrust-quit-signal".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        tracing::warn!(error = %e, "quit signals not handled");
                        return;
                    }
                };
                rt.block_on(async move {
                    use tokio::signal::unix::{SignalKind, signal};
                    let (Ok(mut term), Ok(mut int)) = (
                        signal(SignalKind::terminate()),
                        signal(SignalKind::interrupt()),
                    ) else {
                        tracing::warn!("quit signals not handled");
                        return;
                    };
                    // Started with `nohup`: a hangup stays ignored.
                    let mut hup = if inherited_ignore(1) {
                        None
                    } else {
                        signal(SignalKind::hangup()).ok()
                    };
                    let mut asked = false;
                    loop {
                        tokio::select! {
                            Some(()) = term.recv() => {}
                            Some(()) = int.recv() => {}
                            Some(()) = async {
                                match hup.as_mut() {
                                    Some(h) => h.recv().await,
                                    None => std::future::pending().await,
                                }
                            } => {}
                            else => return,
                        }
                        if asked {
                            tracing::warn!("second quit signal; exiting now");
                            std::process::exit(1);
                        }
                        asked = true;
                        // At once: a probe killed by the same signal must
                        // not be cached as a dead encoder.
                        crate::services::set_quitting();
                        let _ = proxy.send_event(Wake::Quit);
                    }
                });
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "quit signal thread not started");
        }
    }

    /// Whether the process inherited "ignore" for signal `signo` (the
    /// `SigIgn` mask in `/proc/self/status`).
    pub fn inherited_ignore(signo: u32) -> bool {
        std::fs::read_to_string("/proc/self/status").is_ok_and(|s| ignored_in(&s, signo))
    }

    /// Whether a `/proc/<pid>/status` text lists `signo` as ignored.
    pub fn ignored_in(status: &str, signo: u32) -> bool {
        status
            .lines()
            .find_map(|l| l.strip_prefix("SigIgn:"))
            .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
            .is_some_and(|mask| (1..=64).contains(&signo) && mask & (1 << (signo - 1)) != 0)
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

    fn user_event(&mut self, el: &ActiveEventLoop, event: Wake) {
        match event {
            Wake::Tick => self.tick(),
            Wake::Quit => {
                tracing::info!("quit signal received");
                el.exit();
            }
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
                crate::services::set_quitting();
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
        // Close the window first; [`run`] shuts the app down once the event
        // loop has returned.
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
    fn an_inherited_hangup_ignore_is_seen() {
        let status = "Name:\tfthrust\nSigBlk:\t0000000000000000\nSigIgn:\t0000000000001001\n";
        assert!(quit_signals::ignored_in(status, 1), "SIGHUP");
        assert!(!quit_signals::ignored_in(status, 2), "SIGINT");
        assert!(quit_signals::ignored_in(status, 13), "SIGPIPE");
        assert!(!quit_signals::ignored_in("Name:\tx\n", 1));
    }

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
