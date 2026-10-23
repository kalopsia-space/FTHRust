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
    if args.get(1).map(String::as_str) == Some("--hotkey") {
        // The compositor bind client: deliver the action to the running app.
        let Some(action) = args.get(2) else {
            eprintln!("fthrust --hotkey <action>");
            std::process::exit(2);
        };
        let code = match fthrust_core::runtime_dir() {
            Ok(dir) => fthrust_ui::services::hotkey_client::send(&dir, action),
            Err(e) => {
                eprintln!("fthrust: {e}");
                1
            }
        };
        std::process::exit(code);
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
    let mut state = match fthrust_ui::state::AppState::load(paths) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fthrust: {e}");
            std::process::exit(2);
        }
    };
    // One instance: a second launch activates the first and exits.
    let runtime_dir = fthrust_core::runtime_dir().ok();
    let instance_ctx = egui::Context::default();
    let instance = runtime_dir.as_ref().and_then(|dir| {
        fthrust_ui::services::instance::become_primary(
            &state.paths.root().join("fthr.lock"),
            dir,
            &instance_ctx,
        )
        .ok()
    });
    if instance.as_ref().is_some_and(|i| !i.is_primary()) {
        eprintln!("FTHR Clips is already running; activated the existing window");
        return;
    }
    if std::path::Path::new("/dev/shm/FTHR_SharedMemory_v4").exists() {
        tracing::warn!(
            "the Python FTHR Clips engine appears to be running; running both is unsupported"
        );
    }
    state.diagnostics = Some(fthrust_ui::services::diagnostics::start(&state.paths));
    state.thumb_pool = Some(fthrust_ui::library::thumbs::ThumbPool::start(
        state.paths.thumbnails_dir(),
    ));
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
    if let Err(e) = fthrust_ui::runner::run(state, instance, viewport) {
        eprintln!("fthrust: {e}");
        std::process::exit(1);
    }
}
