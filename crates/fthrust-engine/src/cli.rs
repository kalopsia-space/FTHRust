//! The engine's command line. Any `--flag` the engine does not know is a
//! usage error wherever it appears, so a stray flag can never be read as
//! a value (the reference once read `--list-playback-devices` as the fps).

use std::path::PathBuf;

/// What the process should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliMode {
    /// Serve the IPC socket at this path.
    Serve {
        /// Socket path (`--socket PATH`).
        socket: PathBuf,
    },
    /// Print the playback sinks as JSON and exit.
    ListPlaybackDevices,
    /// Print the microphones as JSON and exit.
    ListMicrophones,
    /// Print the version and exit.
    Version,
    /// Print this message to stderr and exit 2.
    UsageError(String),
}

/// The usage line printed on a usage error.
pub const USAGE: &str = "usage: fthrust-engine --socket PATH | --list-playback-devices | --list-microphones | --version";

/// Classifies the arguments after the program name.
pub fn classify(args: &[String]) -> CliMode {
    let listing = |mode: CliMode, flag: &str| {
        if args.len() == 1 {
            mode
        } else {
            CliMode::UsageError(format!("{flag} must be the only argument"))
        }
    };
    let mut socket: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--list-playback-devices" => return listing(CliMode::ListPlaybackDevices, arg),
            "--list-microphones" => return listing(CliMode::ListMicrophones, arg),
            "--version" => return listing(CliMode::Version, arg),
            "--socket" => match args.get(i + 1) {
                Some(path) if !path.starts_with("--") => {
                    socket = Some(PathBuf::from(path));
                    i += 2;
                    continue;
                }
                _ => return CliMode::UsageError("--socket needs a path".into()),
            },
            other if other.starts_with("--") => {
                return CliMode::UsageError(format!("unknown flag {other}"));
            }
            other => return CliMode::UsageError(format!("unexpected argument {other:?}")),
        }
    }
    match socket {
        Some(socket) => CliMode::Serve { socket },
        None => CliMode::UsageError("--socket is required".into()),
    }
}
