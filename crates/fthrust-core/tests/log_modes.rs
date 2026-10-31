//! The log files the app creates are private to the user: they carry paths,
//! window titles and game names. One test in a process of its own, because
//! the crash log installs a process-wide panic hook.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use fthrust_core::FthrPaths;
use fthrust_core::diagnostics::logging::{Logger, RotatingWriter, init_crash_log};

fn mode(path: &Path) -> u32 {
    fs::metadata(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn new_log_files_are_created_0600() {
    // The umask most desktops run with: without the explicit mode these
    // files would be 0644.
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o022));
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());

    // fthr.log, and the new file and the backups a rotation makes.
    let logger = Logger::open_with_limits(&p, "2.0.0", 400, 2).unwrap();
    tracing::subscriber::with_default(logger.subscriber(), || {
        for i in 0..40 {
            tracing::info!(target: "t", "line number {i} padded with some text to fill the file");
        }
    });
    drop(logger);
    for name in ["fthr.log", "fthr.log.1", "fthr.log.2"] {
        assert_eq!(mode(&p.logs_dir().join(name)), 0o600, "{name}");
    }

    // engine.log: the UI's engine driver writes it through a RotatingWriter.
    let mut engine = RotatingWriter::open(&p.engine_log(), 200, 1, false).unwrap();
    for i in 0..10 {
        engine
            .write_line(&format!("engine line {i} with some padding"))
            .unwrap();
    }
    drop(engine);
    assert_eq!(mode(&p.engine_log()), 0o600);
    assert_eq!(mode(&p.logs_dir().join("engine.log.1")), 0o600);

    // fthr-crash.log, and the file a panic appends to.
    init_crash_log(&p).unwrap();
    assert_eq!(mode(&p.crash_log()), 0o600);
    let worker = std::thread::Builder::new()
        .name("boom".into())
        .spawn(|| panic!("deliberate test panic"))
        .unwrap();
    assert!(worker.join().is_err());
    assert!(
        fs::read_to_string(p.crash_log())
            .unwrap()
            .contains("deliberate test panic")
    );
    assert_eq!(mode(&p.crash_log()), 0o600);

    // A log that already exists keeps the mode it has (older profiles).
    let old = p.logs_dir().join("old.log");
    fs::write(&old, "old\n").unwrap();
    fs::set_permissions(&old, fs::Permissions::from_mode(0o644)).unwrap();
    drop(RotatingWriter::open(&old, 1024, 1, false).unwrap());
    assert_eq!(mode(&old), 0o644);
}
