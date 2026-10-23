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
    assert!(!fthrust_ui::services::instance::request_activation(&gone));
    assert!(started.elapsed() < Duration::from_millis(400));
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
    s.diagnostics = Some(fthrust_ui::services::diagnostics::start(&s.paths));
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
