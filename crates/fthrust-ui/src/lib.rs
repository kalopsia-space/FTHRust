//! The FTHRust application shell: tokens, theme, fonts, icons, widgets,
//! the frameless window, settings pages and dialogs, verified against the
//! Qt reference screenshots.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod dialogs;
pub mod editor;
pub mod fidelity;
pub mod fonts;
pub mod icons;
pub mod library;
pub mod motion;
pub mod overlay;
pub mod pages;
pub mod popups;
pub mod runner;
pub mod services;
pub mod settings;
pub mod shell;
pub mod state;
pub mod theme;
pub mod tokens;
pub mod ui_dump;
pub mod video_texture;
pub mod widgets;

/// The application: state, services and the per-frame UI.
pub struct App {
    /// Shared state.
    pub state: state::AppState,
    /// Background services (`None` in tests that build the app by hand).
    pub services: Option<services::Services>,
    /// The first-frame monitor clamp has run.
    clamped: bool,
    /// The bench's state dump (`FTHRUST_UI_DUMP`), if enabled.
    dump: Option<ui_dump::Writer>,
    /// The painter's wgpu state for video textures (set by the runner;
    /// `None` in tests and before the window exists).
    pub render_state: Option<egui_wgpu::RenderState>,
}

impl App {
    /// Builds the app: fonts and style are installed on `ctx`.
    pub fn new(ctx: &egui::Context, state: state::AppState) -> App {
        fonts::install(ctx, &state.tokens);
        tokens::install_style(ctx, &state.tokens);
        if let Some(pool) = &state.thumb_pool {
            pool.set_repaint(ctx.clone());
        }
        App {
            state,
            services: None,
            clamped: false,
            dump: ui_dump::from_env(),
            render_state: None,
        }
    }

    /// Runs the services without painting (loop wakes between frames).
    pub fn tick(&mut self) {
        if let Some(services) = self.services.as_mut() {
            services.tick(&mut self.state);
        }
        self.state.tick_editor(self.render_state.as_ref());
    }

    /// One frame.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if !self.clamped {
            // Review Focus 3: a geometry saved on a removed monitor is clamped
            // to the monitor the window actually opened on.
            let (monitor, inner) = ui.ctx().input(|i| {
                (
                    i.viewport().monitor_size,
                    i.viewport().inner_rect.map(|r| r.size()),
                )
            });
            if let (Some(m), Some(size)) = (monitor, inner) {
                let saved = fthrust_core::window_geometry::Rect {
                    x: 0,
                    y: 0,
                    width: size.x.round() as u32,
                    height: size.y.round() as u32,
                };
                let clamped = shell::window::clamp_to_monitor(saved, m);
                if (clamped.width, clamped.height) != (saved.width, saved.height) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                            clamped.width as f32,
                            clamped.height as f32,
                        )));
                }
                self.clamped = true;
            }
        }
        self.tick();
        shell::ui(ui, &mut self.state);
        if let Some(w) = self.dump.as_mut() {
            let ctx = ui.ctx();
            let markers = fidelity::take(ctx);
            let (ppp, time) = ctx.input(|i| (i.pixels_per_point(), i.time));
            let v = ui_dump::snapshot(&self.state, &markers, ppp, ctx.cumulative_pass_nr(), time);
            match w.write(&v) {
                // Throttled: paint once more after the gap so the file catches up.
                Ok(ui_dump::WriteOutcome::Deferred(after)) => ctx.request_repaint_after(after),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "ui dump failed"),
            }
        }
    }

    /// The reference's shutdown order, then the window geometry is saved.
    pub fn on_exit(&mut self) {
        editor::sessions::shutdown(&mut self.state);
        if let Some(mut services) = self.services.take() {
            services.prepare_exit(&mut self.state);
            services.shutdown();
        }
        services::diagnostics::emit(&self.state, "app", "exit", serde_json::json!({}));
        // winit on Wayland has no window position; keep size and state.
        let rect = self.state.window.last_inner;
        if rect.width > 0 && rect.height > 0 {
            shell::window::persist_geometry(
                &mut self.state.settings,
                (rect.width, rect.height),
                self.state.window.maximized,
            );
        }
    }

    /// The window clear colour (the theme background).
    pub fn clear_color(&self) -> [f32; 4] {
        let c = self.state.tokens.color(tokens::Color::Bg);
        [
            c.r() as f32 / 255.0,
            c.g() as f32 / 255.0,
            c.b() as f32 / 255.0,
            1.0,
        ]
    }
}

/// Crate version, printed by `fthrust --version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the bundled assets live: `$FTHRUST_ASSETS`, else next to the
/// executable (AppImage), else `<prefix>/share/fthrust/assets` (installed),
/// else the repository (development).
pub fn assets_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("FTHRUST_ASSETS") {
        return std::path::PathBuf::from(dir);
    }
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    assets_dir_for(std::env::current_exe().ok().as_deref(), &dev)
}

/// The executable compositor hotkey binds run. Inside an AppImage
/// `current_exe()` is under a mount point that changes every launch, so the
/// image file itself (`$APPIMAGE`, set by the AppImage runtime) is used.
pub fn hotkey_client_exe(
    appimage: Option<&std::ffi::OsStr>,
    current: Option<&std::path::Path>,
) -> String {
    match (appimage.filter(|a| !a.is_empty()), current) {
        (Some(image), _) => std::path::Path::new(image).display().to_string(),
        (None, Some(exe)) => exe.display().to_string(),
        (None, None) => "fthrust".into(),
    }
}

/// [`assets_dir`] without the environment: `<exe dir>/assets` (AppImage),
/// then `<exe dir>/../share/fthrust/assets` (`packaging/install.sh`), then
/// the repository tree, so a packaged or installed build never reads the
/// compile-time path.
pub fn assets_dir_for(exe: Option<&std::path::Path>, dev: &std::path::Path) -> std::path::PathBuf {
    let exe_dir = exe.and_then(|p| p.parent());
    if let Some(adjacent) = exe_dir.map(|d| d.join("assets"))
        && adjacent.is_dir()
    {
        return adjacent;
    }
    if let Some(installed) = exe_dir.map(|d| d.join("../share/fthrust/assets"))
        && installed.is_dir()
    {
        return installed.canonicalize().unwrap_or(installed);
    }
    if dev.is_dir() {
        return dev.canonicalize().unwrap_or_else(|_| dev.to_path_buf());
    }
    std::path::PathBuf::from("assets")
}
