use std::fs;
use std::io::Read;
use std::path::Path;

use fthrust_core::FthrPaths;
use fthrust_core::diagnostics::logging::{Logger, init_crash_log};
use fthrust_core::diagnostics::redact::{redact, sanitize_url_for_log};
use fthrust_core::diagnostics::session::{DiagnosticsSession, README_TEXT};
use serde_json::Value;

fn paths() -> (tempfile::TempDir, FthrPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());
    (tmp, p)
}

#[test]
fn redaction_cases_match_fixture() {
    let cases: Vec<(String, String)> =
        serde_json::from_str(include_str!("fixtures/diagnostics-redaction.json")).unwrap();
    for (input, expected) in cases {
        assert_eq!(redact(&input), expected, "input {input:?}");
    }
    assert_eq!(
        sanitize_url_for_log("https://u:p@host/a/b?token=1"),
        "https://<redacted>:<redacted>@host/a/b?<redacted>"
    );
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn log_line_format_and_banner() {
    let (_tmp, p) = paths();
    let logger = Logger::open(&p, "2.0.0-alpha.1").unwrap();
    tracing::subscriber::with_default(logger.subscriber(), || {
        tracing::info!(target: "fthrust_core::demo", "hello token=abc123 user=bob");
        tracing::warn!(target: "fthrust_core::demo", "slow");
    });
    drop(logger);
    let out = lines(&p.app_log());
    let banner = regex::Regex::new(
        r"^===== FTHR Clips 2\.0\.0-alpha\.1 started \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} \(linux\) =====$",
    )
    .unwrap();
    assert!(banner.is_match(&out[0]), "{}", out[0]);
    let line = regex::Regex::new(
        r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} INFO     \[[^\]]+\] fthrust_core::demo: hello token=<redacted> user=bob$",
    )
    .unwrap();
    assert!(line.is_match(&out[1]), "{}", out[1]);
    assert!(
        out[2].contains(" WARN     [") && out[2].ends_with("fthrust_core::demo: slow"),
        "{}",
        out[2]
    );
}

#[test]
fn log_rotates_at_2mib_keeping_two() {
    let (_tmp, p) = paths();
    let logger = Logger::open_with_limits(&p, "2.0.0", 1_000, 2).unwrap();
    tracing::subscriber::with_default(logger.subscriber(), || {
        for i in 0..200 {
            tracing::info!(target: "t", "line number {i} padded with some text to fill the file");
        }
    });
    drop(logger);
    let dir = p.logs_dir();
    assert!(dir.join("fthr.log").exists());
    assert!(dir.join("fthr.log.1").exists());
    assert!(dir.join("fthr.log.2").exists());
    assert!(
        !dir.join("fthr.log.3").exists(),
        "only two backups are kept"
    );
    assert!(fs::metadata(dir.join("fthr.log")).unwrap().len() <= 1_000 + 120);
    assert!(fs::metadata(dir.join("fthr.log.1")).unwrap().len() <= 1_000 + 120);
    let newest = lines(&dir.join("fthr.log"));
    assert!(newest.last().unwrap().contains("line number 199"));
    assert_eq!(Logger::DEFAULT_MAX_BYTES, 2 * 1024 * 1024);
    assert_eq!(Logger::DEFAULT_BACKUPS, 2);
}

#[test]
fn crash_log_rotates_over_1mib_and_records_panics() {
    let (_tmp, p) = paths();
    fs::create_dir_all(p.logs_dir()).unwrap();
    fs::write(p.crash_log(), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    init_crash_log(&p).unwrap();
    let old = p.logs_dir().join("fthr-crash.log.old");
    assert_eq!(fs::metadata(&old).unwrap().len(), 1024 * 1024 + 1);
    let text = fs::read_to_string(p.crash_log()).unwrap();
    let banner = regex::Regex::new(&format!(
        r"^===== started \d{{4}}-\d{{2}}-\d{{2}} \d{{2}}:\d{{2}}:\d{{2}} pid {} =====\n$",
        std::process::id()
    ))
    .unwrap();
    assert!(banner.is_match(&text), "{text:?}");

    let worker = std::thread::Builder::new()
        .name("boom".into())
        .spawn(|| panic!("deliberate test panic"))
        .unwrap();
    assert!(worker.join().is_err());
    let text = fs::read_to_string(p.crash_log()).unwrap();
    assert!(text.contains("panic: deliberate test panic"), "{text}");
    assert!(text.contains("thread boom"), "{text}");
    assert!(
        text.contains("tests/diagnostics.rs"),
        "panic location is recorded: {text}"
    );
}

#[test]
fn session_dir_name_and_marker() {
    let (_tmp, p) = paths();
    let session = DiagnosticsSession::start(&p, "2.0.0-alpha.1").unwrap();
    let name = session
        .dir()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let re = regex::Regex::new(r"^session-\d{8}T\d{6}Z-[0-9a-f]{8}$").unwrap();
    assert!(re.is_match(&name), "{name}");
    assert_eq!(session.dir().parent().unwrap(), p.diagnostics_dir());
    assert_eq!(session.engine_log_path(), session.dir().join("engine.log"));
    let marker: Value = serde_json::from_str(
        &fs::read_to_string(p.diagnostics_dir().join("active-session.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(marker["session_id"], session.session_id());
    assert_eq!(marker["pid"], std::process::id());
    assert!(marker["started_utc"].as_str().unwrap().ends_with("+00:00"));
    assert_eq!(session.session_id().len(), 36);
    assert!(name.ends_with(&session.session_id()[..8]));
    assert!(session.previous_unclean().is_none());
    assert_eq!(marker.as_object().unwrap().len(), 3);

    let second = DiagnosticsSession::start(&p, "2.0.0-alpha.1").unwrap();
    let prev = second
        .previous_unclean()
        .expect("first session never closed");
    assert_eq!(prev["session_id"], session.session_id());
    let first_events = lines(&session.dir().join("events.jsonl"));
    let started: Value = serde_json::from_str(&first_events[0]).unwrap();
    assert_eq!(started["event"], "session_started");
    assert_eq!(started["subsystem"], "application");
    assert_eq!(started["fields"]["app_version"], "2.0.0-alpha.1");
    let second_events = lines(&second.dir().join("events.jsonl"));
    assert!(
        second_events
            .iter()
            .any(|l| l.contains("previous_session_unclean"))
    );
}

#[test]
fn events_jsonl_appends_and_caps() {
    let (_tmp, p) = paths();
    let session = DiagnosticsSession::start_with_limits(&p, "2.0.0", 600, 1024 * 1024, 5).unwrap();
    for i in 0..20 {
        session.event(
            "capture",
            "poll",
            serde_json::json!({ "n": i, "token": "abcdef" }),
        );
    }
    let out = lines(&session.dir().join("events.jsonl"));
    let total: usize = out.iter().map(|l| l.len() + 1).sum();
    assert!(total <= 600, "events file stays within the cap: {total}");
    assert!(out.len() >= 2 && out.len() < 21);
    let rec: Value = serde_json::from_str(&out[1]).unwrap();
    assert_eq!(rec["schema_version"], 1);
    assert_eq!(rec["session_id"], session.session_id());
    assert_eq!(rec["subsystem"], "capture");
    assert_eq!(rec["event"], "poll");
    assert_eq!(rec["fields"]["n"], 0);
    assert_eq!(
        rec["fields"]["token"], "<redacted>",
        "event fields are redacted"
    );
    assert!(rec["timestamp_utc"].as_str().unwrap().ends_with("+00:00"));
    assert!(rec["monotonic_ms"].is_u64());
    assert!(session.dropped_events() > 0);
}

#[test]
fn keeps_five_sessions() {
    let (_tmp, p) = paths();
    let root = p.diagnostics_dir();
    fs::create_dir_all(&root).unwrap();
    for i in 0..7u64 {
        let dir = root.join(format!("session-2026010{i}T000000Z-{i:08x}"));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("events.jsonl"), "{}\n").unwrap();
        let when = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(1_700_000_000 + i * 60);
        fs::File::open(&dir).unwrap().set_modified(when).unwrap();
    }
    fs::write(root.join("unrelated.txt"), "keep").unwrap();
    let session = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    let mut names: Vec<String> = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("session-"))
        .collect();
    names.sort();
    assert_eq!(names.len(), 5, "{names:?}");
    assert!(
        names.contains(
            &session
                .dir()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        )
    );
    assert!(
        !names.iter().any(|n| n.starts_with("session-20260100")),
        "oldest removed"
    );
    assert!(!names.iter().any(|n| n.starts_with("session-20260101")));
    assert!(!names.iter().any(|n| n.starts_with("session-20260102")));
    assert!(root.join("unrelated.txt").exists());
}

#[test]
fn export_zip_contains_ui_log_tail_and_readme() {
    let (_tmp, p) = paths();
    fs::create_dir_all(p.logs_dir()).unwrap();
    let mut app_log = String::new();
    for i in 0..100 {
        app_log.push_str(&format!(
            "line {i:03} password=hunter2 filler filler filler\n"
        ));
    }
    fs::write(p.app_log(), &app_log).unwrap();
    let session =
        DiagnosticsSession::start_with_limits(&p, "2.0.0", 4 * 1024 * 1024, 512, 5).unwrap();
    session.event("capture", "poll", serde_json::json!({ "n": 1 }));
    fs::write(session.engine_log_path(), "engine says hi\n").unwrap();
    session
        .write_summary(&serde_json::json!({ "health": { "state": "healthy" } }))
        .unwrap();
    let dest = p.root().join("report");
    let written = session.export_zip(&dest, &p.app_log()).unwrap();
    assert_eq!(written.extension().unwrap(), "zip");
    let mut zip = zip::ZipArchive::new(fs::File::open(&written).unwrap()).unwrap();
    let mut names: Vec<String> = (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "README.txt",
            "diagnostic-summary.json",
            "engine.log",
            "events.jsonl",
            "ui.log"
        ]
    );
    let mut read = |name: &str| {
        let mut s = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    };
    let ui = read("ui.log");
    assert!(
        ui.starts_with("[earlier diagnostic log content omitted]\n"),
        "{ui}"
    );
    assert!(ui.len() <= 512);
    assert!(
        ui.contains("line 099 password=<redacted>"),
        "tail keeps the newest lines, redacted: {ui}"
    );
    assert_eq!(read("README.txt"), README_TEXT);
    assert_eq!(read("engine.log"), "engine says hi\n");
    assert!(read("events.jsonl").contains("\"event\":\"poll\""));
    let summary: Value = serde_json::from_str(&read("diagnostic-summary.json")).unwrap();
    assert_eq!(summary["schema_version"], 1);
    assert_eq!(summary["session_id"], session.session_id());
    assert_eq!(summary["app_version"], "2.0.0");
    assert_eq!(summary["health"]["state"], "healthy");
    assert_eq!(summary["privacy"]["automatic_upload"], false);
    assert_eq!(
        summary["export"]["files"],
        serde_json::json!([
            "diagnostic-summary.json",
            "events.jsonl",
            "ui.log",
            "engine.log",
            "README.txt"
        ])
    );
}
