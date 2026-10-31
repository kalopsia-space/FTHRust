//! The pieces the app wires together at startup and shutdown: the marker that
//! reports an unclean exit, the log filter, and the files the support ZIP reads.

use std::fs;
use std::io::Read;

use fthrust_core::FthrPaths;
use fthrust_core::diagnostics::logging::{
    DEFAULT_LOG_FILTER, Logger, RotatingWriter, env_filter_from,
};
use fthrust_core::diagnostics::session::DiagnosticsSession;
use serde_json::{Value, json};

fn paths() -> (tempfile::TempDir, FthrPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());
    (tmp, p)
}

fn marker(p: &FthrPaths) -> std::path::PathBuf {
    p.diagnostics_dir().join("active-session.json")
}

fn read_zip(path: &std::path::Path, name: &str) -> String {
    let mut zip = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut s = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn a_clean_close_removes_the_marker_so_the_next_launch_is_not_unclean() {
    let (_t, p) = paths();
    let first = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    assert!(marker(&p).exists());
    first.close(true);
    assert!(!marker(&p).exists(), "the clean exit removes the marker");
    let second = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    assert!(
        second.previous_unclean().is_none(),
        "no crash is reported for a clean exit"
    );
}

#[test]
fn without_a_close_the_next_launch_reports_the_crash() {
    let (_t, p) = paths();
    let first = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    let second = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    assert_eq!(
        second.previous_unclean().unwrap()["session_id"],
        first.session_id()
    );
}

#[test]
fn an_abnormal_close_keeps_the_marker_and_says_so_in_the_summary() {
    let (_t, p) = paths();
    let s = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    s.close(false);
    assert!(marker(&p).exists());
    let summary: Value =
        serde_json::from_str(&fs::read_to_string(s.dir().join("diagnostic-summary.json")).unwrap())
            .unwrap();
    assert_eq!(summary["status"], "abnormal");
    assert_eq!(summary["session_end"]["clean"], false);
}

#[test]
fn a_clean_close_writes_the_summary_and_the_end_event() {
    let (_t, p) = paths();
    let s = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    s.close(true);
    s.close(true); // closing again changes nothing
    let summary: Value =
        serde_json::from_str(&fs::read_to_string(s.dir().join("diagnostic-summary.json")).unwrap())
            .unwrap();
    assert_eq!(summary["status"], "closed_cleanly");
    assert_eq!(summary["session_end"]["clean"], true);
    assert!(summary["session_end"]["elapsed_ms"].is_u64());
    let events = fs::read_to_string(s.events_path()).unwrap();
    let ended: Vec<&str> = events
        .lines()
        .filter(|l| l.contains("session_ended"))
        .collect();
    assert_eq!(ended.len(), 1, "{events}");
    let rec: Value = serde_json::from_str(ended[0]).unwrap();
    assert_eq!(rec["state"], "clean");
}

#[test]
fn closing_an_old_session_leaves_the_marker_of_the_running_one() {
    let (_t, p) = paths();
    let old = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    let running = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    old.close(true);
    let m: Value = serde_json::from_str(&fs::read_to_string(marker(&p)).unwrap()).unwrap();
    assert_eq!(m["session_id"], running.session_id());
}

#[test]
fn the_default_log_filter_is_info_for_our_crates_and_warn_for_the_rest() {
    let (_t, p) = paths();
    let logger = Logger::open(&p, "2.0.0").unwrap();
    tracing::subscriber::with_default(logger.subscriber_with_filter(env_filter_from(None)), || {
        tracing::info!(target: "fthrust_ui::services", "ours info");
        tracing::debug!(target: "fthrust_ui::services", "ours debug");
        tracing::info!(target: "fthrust", "binary info");
        tracing::info!(target: "wgpu_core::device", "theirs info");
        tracing::warn!(target: "wgpu_core::device", "theirs warn");
        tracing::info!(target: "fthrust_core::demo", count = 3, "with a field");
    });
    drop(logger);
    let log = fs::read_to_string(p.app_log()).unwrap();
    assert!(log.contains("ours info") && log.contains("binary info"));
    assert!(log.contains("theirs warn"));
    assert!(!log.contains("ours debug"), "{log}");
    assert!(!log.contains("theirs info"), "{log}");
    assert!(
        log.contains("with a field") && log.contains("count=3"),
        "{log}"
    );
    assert!(!log.contains('\u{1b}'), "no terminal escapes in the file");
    assert_eq!(DEFAULT_LOG_FILTER, "warn,fthrust=info");
}

#[test]
fn rust_log_overrides_the_default_unless_it_is_not_a_filter() {
    let (_t, p) = paths();
    let logger = Logger::open(&p, "2.0.0").unwrap();
    tracing::subscriber::with_default(
        logger.subscriber_with_filter(env_filter_from(Some("debug"))),
        || {
            tracing::debug!(target: "wgpu_core::device", "debug from anywhere");
        },
    );
    tracing::subscriber::with_default(
        logger.subscriber_with_filter(env_filter_from(Some("=== not a filter ==="))),
        || {
            tracing::debug!(target: "fthrust_ui", "dropped by the fallback");
            tracing::info!(target: "fthrust_ui", "kept by the fallback");
        },
    );
    drop(logger);
    let log = fs::read_to_string(p.app_log()).unwrap();
    assert!(log.contains("debug from anywhere"));
    assert!(log.contains("kept by the fallback") && !log.contains("dropped by the fallback"));
}

#[test]
fn the_support_zip_reads_the_files_the_app_writes() {
    let (_t, p) = paths();
    // What the app does: the application log through init_logging's Logger...
    let logger = Logger::open(&p, "2.0.0").unwrap();
    tracing::subscriber::with_default(logger.subscriber(), || {
        tracing::warn!(target: "fthrust_ui", "ui says hello");
    });
    drop(logger);
    // ...and the UI's engine driver writing the engine's stderr line by line.
    let mut engine = RotatingWriter::open(&p.engine_log(), 2 * 1024 * 1024, 1, false).unwrap();
    engine.write_line("engine says hello token=abc123").unwrap();
    drop(engine);

    let session = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    session.event("capture", "poll", json!({"n": 1}));
    let zip = session
        .export_zip(&p.root().join("report"), &p.app_log())
        .unwrap();
    let ui = read_zip(&zip, "ui.log");
    assert!(ui.contains("ui says hello"), "{ui}");
    assert!(ui.contains("===== FTHR Clips 2.0.0 started"), "{ui}");
    let engine = read_zip(&zip, "engine.log");
    assert!(
        engine.contains("engine says hello token=<redacted>"),
        "{engine}"
    );
}
