//! Plan 5 Task 8: the services runtime, instance activation and the
//! hotkey client.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use fthrust_ui::services::{Event, Request, Services};
use fthrust_ui::state::{AppState, StatusStyle};

fn state() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.tokens.reduce_motion = true;
    (tmp, s)
}

#[test]
fn tick_drains_events_and_sends_requests() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    services.inject(Event::Status {
        text: "CAPTURING".into(),
        style: StatusStyle::Active,
    });
    services.tick(&mut s);
    assert_eq!(s.status.text, "CAPTURING");
    assert_eq!(s.status.style, StatusStyle::Active);
    s.scan_clips_requested = true;
    s.engine_restart_requested = true;
    s.error_action_requested = Some("restart".into());
    services.tick(&mut s);
    assert!(
        !s.scan_clips_requested
            && !s.engine_restart_requested
            && s.error_action_requested.is_none(),
        "flags are cleared"
    );
    let sent = services.drain_requests_for_tests();
    assert_eq!(
        sent,
        vec![
            Request::Scan,
            Request::RestartEngine,
            Request::ErrorAction("restart".into())
        ]
    );
    // A scan result lands in the library.
    let d = tempfile::tempdir().unwrap();
    let clip = d.path().join("a.mp4");
    std::fs::write(&clip, b"x").unwrap();
    let records = vec![fthrust_core::library_index::LibraryRecord {
        path: clip.clone(),
        identity: clip.clone(),
        root_ids: vec![0],
        imported: false,
        kind: fthrust_core::library_index::MediaKind::Video,
        size: 1,
        mtime_ns: 1,
    }];
    services.inject(Event::Scan(fthrust_ui::library::scan::ScanEvent::Done {
        generation: 1,
        kind: fthrust_ui::library::scan::ScanKind::Foreground,
        records,
        watched: vec![],
        elapsed_ms: 1,
    }));
    services.tick(&mut s);
    assert_eq!(s.library.records.len(), 1);
    assert!(s.library.scanned && s.library_dirty);
    services.shutdown();
}

#[test]
fn secondary_instance_activates_primary() {
    let d = tempfile::tempdir().unwrap();
    let runtime = d.path().join("rt");
    std::fs::create_dir_all(&runtime).unwrap();
    let lock = d.path().join("fthr.lock");
    let ctx = egui::Context::default();
    let primary =
        fthrust_ui::services::instance::become_primary(&lock, &runtime, &ctx).expect("primary");
    assert!(primary.is_primary());
    // A second launch sends `activate` and is told it is secondary.
    let second =
        fthrust_ui::services::instance::become_primary(&lock, &runtime, &ctx).expect("secondary");
    assert!(!second.is_primary());
    let deadline = Instant::now() + Duration::from_millis(350);
    let mut activated = false;
    while Instant::now() < deadline {
        if primary.activation_requested() {
            activated = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(activated, "the primary saw the activation request");
    drop(second);
    drop(primary);
    // A missing primary: the request fails within the timeout.
    let gone = d.path().join("gone");
    std::fs::create_dir_all(&gone).unwrap();
    let started = Instant::now();
    assert_eq!(
        fthrust_ui::services::instance::request_activation(&gone),
        fthrust_ui::services::instance::Activation::NoInstance
    );
    assert!(started.elapsed() < Duration::from_millis(400));
}

/// Final review of the audit fixes: an instance that is quitting keeps the
/// lock until its shutdown is done; a launch meanwhile is told so and takes
/// the lock once it goes, instead of reporting a window that never shows.
#[test]
fn a_launch_waits_for_an_instance_that_is_quitting() {
    use fthrust_ui::services::instance::{Activation, become_primary_within, request_activation};
    let d = tempfile::tempdir().unwrap();
    let runtime = d.path().join("rt");
    std::fs::create_dir_all(&runtime).unwrap();
    let lock = d.path().join("fthr.lock");
    let ctx = egui::Context::default();
    let primary = become_primary_within(&lock, &runtime, &ctx, Duration::ZERO).expect("primary");
    assert_eq!(request_activation(&runtime), Activation::Shown);
    assert!(primary.activation_requested());
    primary.set_exiting();
    assert_eq!(request_activation(&runtime), Activation::Exiting);
    assert!(
        !primary.activation_requested(),
        "a quitting instance shows nothing"
    );
    // Too short a wait: an error, not a secondary that "activated" it.
    assert!(become_primary_within(&lock, &runtime, &ctx, Duration::from_millis(200)).is_err());
    // The lock goes while a launch waits: that launch becomes the instance.
    let (lock2, runtime2, ctx2) = (lock.clone(), runtime.clone(), ctx.clone());
    let waiting = std::thread::spawn(move || {
        become_primary_within(&lock2, &runtime2, &ctx2, Duration::from_secs(10))
    });
    std::thread::sleep(Duration::from_millis(300));
    drop(primary);
    let next = waiting.join().unwrap().expect("the waiting launch starts");
    assert!(next.is_primary());
    assert_eq!(request_activation(&runtime), Activation::Shown);
}

#[test]
fn hotkey_client_writes_action_and_fails_without_socket() {
    let d = tempfile::tempdir().unwrap();
    let runtime = d.path().join("rt");
    std::fs::create_dir_all(&runtime).unwrap();
    let sock = runtime.join("hotkey.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let handle = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut conn, &mut buf).unwrap();
        buf
    });
    assert_eq!(
        fthrust_ui::services::hotkey_client::send(&runtime, "save_clip"),
        0
    );
    assert_eq!(handle.join().unwrap(), "save_clip");
    let empty = d.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(
        fthrust_ui::services::hotkey_client::send(&empty, "save_clip"),
        1
    );
    let _: PathBuf = sock;
}

#[test]
fn diagnostics_session_records_events_and_exports_from_the_live_session() {
    let (_tmp, mut s) = state();
    let ctx = egui::Context::default();
    let mut services = Services::start_for_tests(&ctx);
    s.diagnostics = fthrust_ui::services::diagnostics::start(&s.paths);
    assert!(s.diagnostics.is_some(), "a writable profile gets a session");
    fthrust_ui::services::diagnostics::emit(
        &s,
        "library",
        "scan_started",
        serde_json::json!({"generation": 1}),
    );
    let events = s.paths.diagnostics_dir();
    let mut found = false;
    for entry in walk(&events) {
        if entry.file_name().unwrap() == "events.jsonl" {
            let text = std::fs::read_to_string(&entry).unwrap();
            found |= text.contains("\"scan_started\"");
        }
    }
    assert!(found, "events.jsonl has the event");
    let zip = fthrust_ui::dialogs::diagnostic_report::export(&mut s).unwrap();
    assert!(zip.exists());
    services.tick(&mut s);
    services.shutdown();
}

#[test]
fn a_diagnostics_session_that_cannot_start_leaves_the_app_without_one() {
    let (_tmp, mut s) = state();
    // A file where the diagnostics directory belongs, as when the disk is
    // full or read-only: nothing can be created there. Starting must neither
    // panic nor fall back to a session in the temp directory.
    std::fs::create_dir_all(s.paths.root()).unwrap();
    std::fs::write(s.paths.diagnostics_dir(), "in the way").unwrap();
    assert!(fthrust_ui::services::diagnostics::start(&s.paths).is_none());
    s.diagnostics = None;
    fthrust_ui::services::diagnostics::emit(&s, "library", "scan_started", serde_json::json!({}));
    // The report dialog says what went wrong instead of panicking.
    assert!(fthrust_ui::dialogs::diagnostic_report::export(&mut s).is_err());
}

#[test]
fn a_report_made_without_a_live_session_leaves_no_crash_marker() {
    // Diagnostics could not start at launch (a full disk), so there is no
    // live session; the disk has room again when the user asks for a report.
    let (_tmp, mut s) = state();
    assert!(s.diagnostics.is_none());
    let zip = fthrust_ui::dialogs::diagnostic_report::export(&mut s).unwrap();
    assert!(zip.exists());
    assert!(
        !s.paths
            .diagnostics_dir()
            .join("active-session.json")
            .exists(),
        "the report's own session is closed: no marker for the next launch to call a crash"
    );
    let next =
        fthrust_core::diagnostics::session::DiagnosticsSession::start(&s.paths, "next").unwrap();
    assert!(
        next.previous_unclean().is_none(),
        "the next launch is not told the previous one did not shut down cleanly"
    );
}

#[test]
fn startup_notices_are_not_swallowed_by_the_error_notification_setting() {
    use fthrust_ui::shell::error_bar::Level;
    let tmp = tempfile::tempdir().unwrap();
    // A damaged theme.json is something the stores report at startup.
    let paths = fthrust_core::FthrPaths::new(tmp.path());
    std::fs::create_dir_all(paths.theme_dir()).unwrap();
    std::fs::write(paths.theme_file(), "{not json").unwrap();
    let mut s = AppState::for_tests(
        tmp.path(),
        serde_json::json!({"error_notifications_enabled": false}),
    );
    assert!(
        !s.error_bar.enabled,
        "the user turned error notifications off"
    );
    assert!(
        s.theme
            .startup_issues()
            .iter()
            .any(|m| m.contains("theme.json was damaged"))
    );
    fthrust_ui::services::diagnostics::report_startup_issues(&mut s);
    let shown = s.error_bar.current().expect("the notice is shown anyway");
    assert_eq!(shown.title, "STARTUP PROBLEM");
    assert!(
        shown.detail.contains("theme.json was damaged"),
        "{}",
        shown.detail
    );
    // Ordinary errors still follow the setting.
    assert!(
        !s.error_bar
            .push_error("X", "y", Level::Error, Vec::new(), None, false)
    );
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// Audit 2026-10-31: frequent background events (health, save progress,
/// window lists) wake the services without repainting the whole UI.
#[test]
fn quiet_events_do_not_request_a_repaint() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let ctx = egui::Context::default();
    let repaints = Arc::new(AtomicUsize::new(0));
    let seen = repaints.clone();
    ctx.set_request_repaint_callback(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let outbox = fthrust_ui::services::Outbox::new(tx, ctx.clone());
    outbox.send_quiet(Event::Sound("clip".into()));
    assert_eq!(repaints.load(Ordering::SeqCst), 0);
    assert!(rx.try_recv().is_ok(), "the event is delivered");
    outbox.send(Event::Sound("clip".into()));
    assert!(repaints.load(Ordering::SeqCst) > 0);
}
