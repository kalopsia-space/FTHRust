//! `fthrust-engine` binary entry point.
#![forbid(unsafe_code)]

use std::process::ExitCode;

use fthrust_engine::cli::{CliMode, USAGE, classify};
use fthrust_engine::devices::{enumerate_sinks, enumerate_sources, error_json, listing_json};
use fthrust_ipc::BackendKind;

fn main() -> ExitCode {
    fthrust_media::capabilities::handle_probe_argv();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match classify(&args) {
        CliMode::Version => {
            println!("fthrust-engine {}", fthrust_engine::VERSION);
            ExitCode::SUCCESS
        }
        CliMode::ListPlaybackDevices => listing(enumerate_sinks()),
        CliMode::ListMicrophones => listing(enumerate_sources()),
        CliMode::UsageError(msg) => {
            eprintln!("fthrust-engine: {msg}\n{USAGE}");
            ExitCode::from(2)
        }
        CliMode::Serve { socket } => {
            fthrust_engine::alloc::configure();
            init_logging();
            let default_override = std::env::var("FTHR_CAPTURE_BACKEND")
                .ok()
                .and_then(|v| BackendKind::parse_override(&v));
            let paths = fthrust_core::FthrPaths::from_env().ok();
            let replay_root = fthrust_engine::replay_dir::replay_root();
            fthrust_engine::replay_dir::remove_stale(
                &replay_root,
                fthrust_engine::replay_dir::pid_alive,
            );
            fthrust_engine::replay_dir::remove_own(&replay_root);
            let served = fthrust_engine::daemon::serve(
                socket,
                fthrust_engine::daemon::production_deps(paths),
                default_override,
            );
            fthrust_engine::replay_dir::remove_own(&replay_root);
            match served {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("FTHR_STARTUP_ERROR: ENGINE_START_FAILED: {e}");
                    ExitCode::from(1)
                }
            }
        }
    }
}

fn listing(result: Result<Vec<fthrust_engine::devices::Endpoint>, String>) -> ExitCode {
    match result {
        Ok(endpoints) => {
            println!("{}", listing_json(&endpoints));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}", error_json(&e));
            ExitCode::from(1)
        }
    }
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("FTHRUST_LOG")
        .unwrap_or_else(|_| "info".into());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
