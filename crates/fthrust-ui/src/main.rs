//! `fthrust` binary entry point: the main window (spec 5.1).
#![forbid(unsafe_code)]

use fthrust_core::FthrPaths;
use fthrust_core::diagnostics::logging::{
    LogGuard, default_env_filter, init_crash_log, init_logging,
};
use fthrust_core::window_geometry::{Rect, Screen};
use fthrust_ui::state::AppState;

/// Starts the file log (`~/.fthr/logs/fthr.log`, mirrored to stderr; `RUST_LOG`
/// or else info for FTHRust's crates and warnings for the rest) and the crash
/// log (`fthr-crash.log`, every panic with its backtrace). Without a writable
/// log directory the app still logs to the terminal.
fn start_logging(paths: &FthrPaths) -> Option<LogGuard> {
    let guard = match init_logging(paths, fthrust_ui::VERSION) {
        Ok(guard) => Some(guard),
        Err(e) => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(default_env_filter())
                .with_writer(std::io::stderr)
                .try_init();
            tracing::warn!("no log file: {e}");
            None
        }
    };
    if let Err(e) = init_crash_log(paths) {
        tracing::warn!("no crash log: {e}");
    }
    guard
}

/// Removes what a crash left in the clips and recordings folders (hidden
/// `.partial` files of saves, recordings, exports and screenshots), on a
/// background thread: the walk can cover a whole drive.
fn sweep_stale_partials(state: &AppState) {
    let roots: Vec<_> = [
        state.settings.clips_directory(),
        state.settings.recording_directory(),
    ]
    .into_iter()
    .flatten()
    .collect();
    if let Err(e) = fthrust_core::clip_files::spawn_stale_partial_sweep(roots) {
        tracing::warn!("stale partial sweep not started: {e}");
    }
}

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
    let paths = match FthrPaths::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("fthrust: {e}");
            std::process::exit(2);
        }
    };
    // One instance: a second launch activates the first and exits. That
    // happens before the log is opened and the stores are loaded, so a second
    // launch adds nothing to the first one's log and touches none of its files.
    let runtime_dir = fthrust_core::runtime_dir().ok();
    let instance_ctx = egui::Context::default();
    let instance = runtime_dir.as_ref().and_then(|dir| {
        fthrust_ui::services::instance::become_primary(
            &paths.root().join("fthr.lock"),
            dir,
            &instance_ctx,
        )
        .ok()
    });
    if instance.as_ref().is_some_and(|i| !i.is_primary()) {
        eprintln!("FTHR Clips is already running; activated the existing window");
        return;
    }
    // While the process is still single-threaded (the log stamps local time).
    let _log = start_logging(&paths);
    paths.restrict_private_dirs();
    let mut state = match AppState::load(paths) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("cannot load the settings: {e}");
            eprintln!("fthrust: {e}");
            std::process::exit(2);
        }
    };
    if std::path::Path::new("/dev/shm/FTHR_SharedMemory_v4").exists() {
        tracing::warn!(
            "the Python FTHR Clips engine appears to be running; running both is unsupported"
        );
    }
    state.diagnostics = Some(fthrust_ui::services::diagnostics::start(&state.paths));
    sweep_stale_partials(&state);
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
        .with_title("FTHRust Clips")
        .with_app_id(fthrust_ui::services::APP_ID)
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
    let diagnostics = state.diagnostics.clone();
    let result = fthrust_ui::runner::run(state, instance, viewport);
    // The window loop has ended on its own terms: a clean exit removes the
    // marker that makes the next launch report this one as a crash.
    if let Some(session) = &diagnostics {
        session.close(result.is_ok());
    }
    if let Err(e) = result {
        tracing::error!("{e}");
        eprintln!("fthrust: {e}");
        std::process::exit(1);
    }
}
