//! The FTHRust application shell: tokens, theme, fonts, icons, widgets,
//! the frameless window, settings pages and dialogs, verified against the
//! Qt reference screenshots.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod dialogs;
pub mod fidelity;
pub mod fonts;
pub mod icons;
pub mod motion;
pub mod pages;
pub mod popups;
pub mod settings;
pub mod shell;
pub mod state;
pub mod theme;
pub mod tokens;
pub mod widgets;

/// The eframe application.
pub struct App {
    /// Shared state.
    pub state: state::AppState,
    screen_name: String,
}

impl App {
    /// Builds the app: fonts and style are installed on `ctx`.
    pub fn new(cc: &eframe::CreationContext<'_>, state: state::AppState) -> App {
        fonts::install(&cc.egui_ctx, &state.tokens);
        tokens::install_style(&cc.egui_ctx, &state.tokens);
        App {
            state,
            screen_name: String::new(),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        shell::ui(ui, &mut self.state);
    }

    fn on_exit(&mut self) {
        // winit on Wayland has no window position; keep size and state.
        let rect = self.state.window.last_inner;
        if rect.width > 0 && rect.height > 0 {
            shell::window::persist_geometry(
                &mut self.state.settings,
                rect,
                self.state.window.maximized,
                &self.screen_name,
            );
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
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

/// Where the bundled assets live, relative to the repository (development)
/// or next to the executable (packaged).
pub fn assets_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("FTHRUST_ASSETS") {
        return std::path::PathBuf::from(dir);
    }
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    if dev.is_dir() {
        return dev.canonicalize().unwrap_or(dev);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("assets")))
        .unwrap_or_else(|| std::path::PathBuf::from("assets"))
}
