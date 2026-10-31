//! `init_logging` installs the process-wide subscriber, so this file holds a
//! single test and runs it in a process of its own.

use fthrust_core::FthrPaths;
use fthrust_core::diagnostics::logging::init_logging;

#[test]
fn init_logging_writes_fthr_log_with_the_default_filter() {
    // SAFETY: the only test in this binary, so nothing else reads the
    // environment while it changes.
    unsafe { std::env::remove_var("RUST_LOG") };
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());
    let guard = init_logging(&p, "2.0.0-test").expect("first init succeeds");
    tracing::info!(target: "fthrust_ui", "ours at info");
    tracing::debug!(target: "fthrust_ui", "ours at debug");
    tracing::info!(target: "wgpu_core", "theirs at info");
    tracing::warn!(target: "wgpu_core", "theirs at warn");
    // `log` records (wgpu, winit, zbus...) come through as well.
    log::warn!(target: "winit", "from the log crate");
    drop(guard);
    let log = std::fs::read_to_string(p.app_log()).unwrap();
    assert!(
        log.starts_with("===== FTHR Clips 2.0.0-test started "),
        "{log}"
    );
    assert!(log.contains("ours at info") && log.contains("theirs at warn"));
    assert!(log.contains("from the log crate"), "{log}");
    assert!(!log.contains("ours at debug") && !log.contains("theirs at info"));
    assert!(
        init_logging(&p, "2.0.0-test").is_err(),
        "a second global logger is refused"
    );
}
