//! `fthrust` binary entry point: the main window (spec 5.1).
#![forbid(unsafe_code)]

use fthrust_core::FthrPaths;
use fthrust_core::window_geometry::{Rect, Screen};

fn main() {
    fthrust_media::capabilities::handle_probe_argv();
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--version") {
        println!("fthrust {}", fthrust_ui::VERSION);
        return;
    }
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let paths = match FthrPaths::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("fthrust: {e}");
            std::process::exit(2);
        }
    };
    let state = match fthrust_ui::state::AppState::load(paths) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fthrust: {e}");
            std::process::exit(2);
        }
    };
    // Without a monitor list before the window exists, restore against a
    // generous virtual screen; the compositor clamps the final placement.
    let screens = [Screen {
        name: String::new(),
        available: Rect {
            x: 0,
            y: 0,
            width: 16384,
            height: 16384,
        },
        primary: true,
    }];
    let placement = fthrust_ui::shell::window::restore_geometry(&state.settings, &screens);
    let viewport = egui::ViewportBuilder::default()
        .with_title("FTHR Clips")
        .with_app_id("fthrust")
        .with_decorations(false)
        .with_inner_size(egui::vec2(
            placement.rect.width as f32,
            placement.rect.height as f32,
        ))
        .with_min_inner_size(egui::vec2(
            fthrust_core::window_geometry::MIN_WIDTH as f32,
            fthrust_core::window_geometry::MIN_HEIGHT as f32,
        ))
        .with_maximized(placement.maximized);
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "FTHR Clips",
        options,
        Box::new(move |cc| Ok(Box::new(fthrust_ui::App::new(cc, state)))),
    ) {
        eprintln!("fthrust: {e}");
        std::process::exit(1);
    }
}
