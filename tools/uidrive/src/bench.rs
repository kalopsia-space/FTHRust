//! Where the bench lives (`BENCH`, `BENCH_SOCKET`, `BENCH_APP` from `bench.sh`).

use std::path::PathBuf;

/// The bench directories and socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bench {
    /// `$BENCH`.
    pub dir: PathBuf,
    /// The Wayland socket name inside `runtime_dir`.
    pub socket: String,
    /// The private `XDG_RUNTIME_DIR`.
    pub runtime_dir: PathBuf,
    /// The app's dump file.
    pub dump: PathBuf,
    /// Screenshots.
    pub shots: PathBuf,
    /// App and engine logs.
    pub logs: PathBuf,
    /// The app binary (`BENCH_APP`), for `hotkey`.
    pub app: Option<PathBuf>,
}

impl Bench {
    /// Reads the environment `bench.sh` exports.
    pub fn from_env() -> Result<Bench, String> {
        let dir = std::env::var_os("BENCH")
            .map(PathBuf::from)
            .ok_or("BENCH is not set; run under `bench.sh run` or `eval $(bench.sh up)`")?;
        let socket = std::env::var("BENCH_SOCKET").unwrap_or_else(|_| "wayland-0".to_owned());
        Ok(Bench {
            runtime_dir: dir.join("rt"),
            dump: dir.join("ui.json"),
            shots: dir.join("shots"),
            logs: dir.join("logs"),
            app: std::env::var_os("BENCH_APP").map(PathBuf::from),
            socket,
            dir,
        })
    }
}
