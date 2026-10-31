//! `events.jsonl` in a support ZIP stays JSON, one record per line, and stays
//! redacted.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};

use fthrust_core::FthrPaths;
use fthrust_core::diagnostics::session::DiagnosticsSession;
use serde_json::{Value, json};

fn paths() -> (tempfile::TempDir, FthrPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());
    (tmp, p)
}

struct Export {
    events: String,
    summary: Value,
}

fn export(session: &DiagnosticsSession, p: &FthrPaths) -> Export {
    let dest = p.root().join("report.zip");
    let written = session.export_zip(&dest, &p.app_log()).unwrap();
    let mut zip = zip::ZipArchive::new(fs::File::open(written).unwrap()).unwrap();
    let mut read = |name: &str| {
        let mut s = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    };
    Export {
        events: read("events.jsonl"),
        summary: serde_json::from_str(&read("diagnostic-summary.json")).unwrap(),
    }
}

fn append(session: &DiagnosticsSession, text: &str) {
    let mut f = OpenOptions::new()
        .append(true)
        .open(session.events_path())
        .unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

fn records(events: &str) -> Vec<Value> {
    events
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l:?}")))
        .collect()
}

#[test]
fn every_exported_line_is_a_json_record_with_the_secrets_gone() {
    let (_t, p) = paths();
    let home = std::env::var("HOME").unwrap();
    let session = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    session.event(
        "upload",
        "attempt",
        json!({
            "auth": "Bearer abc",
            "note": "retry with password=hunter2",
            "target": format!("{home}/Videos/a.mp4"),
        }),
    );
    // Written by an older build that did not know sftp_password.
    append(
        &session,
        &format!(
            "{}\n",
            json!({"schema_version": 1, "subsystem": "upload", "event": "old",
                "fields": {"sftp_password": "hunter2", "client_secret": "s", "ok": 1}})
        ),
    );
    let out = export(&session, &p);
    let recs = records(&out.events);
    assert!(recs.len() >= 3, "{}", out.events);
    assert!(!out.events.contains("hunter2") && !out.events.contains("Bearer abc"));
    assert!(!out.events.contains(&home), "{}", out.events);
    let attempt = recs.iter().find(|r| r["event"] == "attempt").unwrap();
    assert_eq!(attempt["fields"]["auth"], "<redacted>");
    assert_eq!(attempt["fields"]["note"], "retry with password=<redacted>");
    let old = recs.iter().find(|r| r["event"] == "old").unwrap();
    assert_eq!(old["fields"]["sftp_password"], "<redacted>");
    assert_eq!(old["fields"]["client_secret"], "<redacted>");
    assert_eq!(old["fields"]["ok"], 1);
    assert_eq!(out.summary["export"]["events_truncated"], false);
    assert_eq!(out.summary["export"]["events_invalid_lines"], 0);
}

#[test]
fn a_record_cut_short_by_a_crash_is_dropped_not_exported() {
    let (_t, p) = paths();
    let session = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    append(
        &session,
        "{\"schema_version\":1,\"event\":\"half\",\"fields\":{\"a\n",
    );
    session.event("capture", "after", json!({"n": 1}));
    let out = export(&session, &p);
    let recs = records(&out.events);
    assert!(recs.iter().any(|r| r["event"] == "after"));
    assert!(!out.events.contains("half"));
    assert_eq!(out.summary["export"]["events_invalid_lines"], 1);
}

#[test]
fn a_file_over_the_limit_exports_its_newest_whole_records_only() {
    let (_t, p) = paths();
    // The cap on what is written is low; the file is then grown past it, as
    // a long session of an older build could have.
    let session = DiagnosticsSession::start_with_limits(&p, "2.0.0", 700, 1024, 5).unwrap();
    for i in 0..40 {
        append(
            &session,
            &format!(
                "{}\n",
                json!({"schema_version": 1, "subsystem": "capture", "event": "poll", "fields": {"n": i}})
            ),
        );
    }
    let out = export(&session, &p);
    assert!(out.events.len() <= 700, "{} bytes", out.events.len());
    let recs = records(&out.events);
    assert!(recs.len() > 3, "{}", out.events);
    assert_eq!(
        recs.last().unwrap()["fields"]["n"],
        39,
        "the newest record is the last"
    );
    assert!(
        !out.events.contains("omitted"),
        "no non-JSON note inside events.jsonl"
    );
    assert_eq!(out.summary["export"]["events_truncated"], true);
}

#[test]
fn a_session_without_an_events_file_exports_an_empty_one() {
    let (_t, p) = paths();
    let session = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    fs::remove_file(session.events_path()).unwrap();
    let out = export(&session, &p);
    assert_eq!(out.events, "");
}
