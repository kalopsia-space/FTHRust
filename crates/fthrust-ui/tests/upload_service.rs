//! The upload queue, the uploader process and the feedback it gives
//! (notes-uploader-reference 1.3, 6).

#[path = "../../fthrust-media/tests/support/mod.rs"]
mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use fthrust_core::upload::history::{Entry, History};
use fthrust_ui::services::CardKind;
use fthrust_ui::services::upload::{ChildSlot, Timeouts, UploadService, run_request};
use fthrust_ui::state::{AppState, DialogKind};
use serde_json::{Value, json};

/// A fake uploader: a shell script with `body`.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

struct Rig {
    dir: tempfile::TempDir,
    state: AppState,
    svc: UploadService,
    clips: PathBuf,
}

fn rig(uploader: &str, extra: Value) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let clips = dir.path().join("clips");
    std::fs::create_dir_all(clips.join("Desktop")).unwrap();
    let mut state = AppState::for_tests(
        dir.path(),
        json!({"clips_directory": clips.to_string_lossy()}),
    );
    state.uploader_settings.set("upload_enabled", json!(true));
    state
        .uploader_settings
        .accept_catbox("2026-10-23T00:00:00+00:00");
    state
        .uploader_settings
        .set("upload_provider", json!("custom"));
    for (k, v) in extra.as_object().unwrap() {
        state.uploader_settings.set(k, v.clone());
    }
    state.uploader_settings.save(&state.paths).unwrap();
    let history = state.paths.upload_history_file();
    let exe = script(
        dir.path(),
        "uploader",
        &uploader.replace("$HISTORY", &history.to_string_lossy()),
    );
    let mut svc = UploadService::with_exe(Some(exe));
    svc.set_timeouts(Timeouts {
        upload: Duration::from_secs(10),
        test: Duration::from_secs(10),
        credentials: Duration::from_secs(10),
    });
    Rig {
        dir,
        state,
        svc,
        clips,
    }
}

impl Rig {
    fn clip(&self, name: &str) -> PathBuf {
        let p = self.clips.join("Desktop").join(name);
        std::fs::write(&p, vec![1u8; 1000]).unwrap();
        p
    }

    /// Ticks until nothing is queued or in flight.
    fn settle(&mut self) {
        let start = Instant::now();
        let mut now = 0.0;
        loop {
            self.svc.tick(&mut self.state, now);
            if !self.svc.busy() {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "upload never finished"
            );
            std::thread::sleep(Duration::from_millis(20));
            now += 0.02;
        }
    }

    fn requests(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join("requests.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn error_titles(&self) -> Vec<String> {
        self.state
            .error_bar
            .entries()
            .iter()
            .map(|e| e.title.clone())
            .collect()
    }
}

/// Logs each request and answers `$reply`, writing a history entry for
/// the clip like the real uploader.
fn uploader_ok(url: &str) -> String {
    format!(
        r#"read -r line; echo "$line" >> "$(dirname "$0")/requests.jsonl"
path=$(echo "$line" | sed 's/.*"path":"\([^"]*\)".*/\1/')
mkdir -p "$(dirname "$HISTORY")"
printf '{{"%s": {{"status": "ok", "provider": "custom", "url": "{url}", "raw_url": "{url}"}}}}' "$path" > "$HISTORY"
echo 'progress noise'
echo '{{"ok":true,"message":"{url}"}}'"#
    )
}

#[test]
fn runner_parses_last_line_and_failures() {
    let dir = tempfile::tempdir().unwrap();
    let slot = ChildSlot::default();
    let t = Duration::from_secs(5);
    let req = json!({"action": "test_connection"});
    let run = |body: &str, timeout: Duration| {
        let exe = script(dir.path(), &format!("u{}", body.len()), body);
        run_request(&exe, &req, timeout, &slot)
    };
    let r = run(
        "cat >/dev/null; echo junk; echo '{\"ok\":true,\"message\":\"fine\"}'; echo",
        t,
    );
    assert_eq!((r.ok, r.message.as_str()), (true, "fine"));
    let r = run("cat >/dev/null; echo 'boom happened' >&2; exit 2", t);
    assert_eq!((r.ok, r.message.as_str()), (false, "boom happened"));
    let r = run("cat >/dev/null; exit 3", t);
    assert_eq!(
        (r.ok, r.message.as_str()),
        (false, "Uploader exited with code 3")
    );
    let r = run("cat >/dev/null; echo '{oops'", t);
    assert_eq!(
        (r.ok, r.message.as_str()),
        (false, "Uploader returned an invalid response.")
    );
    let r = run("cat >/dev/null; echo '[1]'", t);
    assert_eq!(r.message, "Uploader returned an invalid response.");
    let started = Instant::now();
    let r = run("cat >/dev/null; sleep 30", Duration::from_millis(300));
    assert_eq!(
        (r.ok, r.message.as_str()),
        (false, "The uploader did not finish in time.")
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    // The request arrives whole on stdin.
    let exe = script(
        dir.path(),
        "echoer",
        "read -r line; printf '{\"ok\":true,\"message\":%s}\\n' \"$(echo \"$line\" | sed 's/.*\"action\":\\(\"[a-z_]*\"\\).*/\\1/')\"",
    );
    let r = run_request(&exe, &req, t, &slot);
    assert_eq!(r.message, "test_connection");
}

#[test]
fn success_updates_card_and_toasts() {
    let mut r = rig(&uploader_ok("https://cdn.example/a.mp4"), json!({}));
    let clip = r.clip("a.mp4");
    r.svc.enqueue(&mut r.state, clip.clone());
    assert_eq!(
        r.state.upload_status.get(&clip).map(String::as_str),
        Some("Uploading…")
    );
    assert!(r.svc.is_busy(&clip));
    r.settle();
    assert!(!r.state.upload_status.contains_key(&clip), "status cleared");
    assert_eq!(
        r.state.pending_cards,
        vec![CardKind::Upload {
            filename: "a.mp4 · ORIGINAL".into()
        }]
    );
    let status = r.svc.transient_status(1.0).unwrap();
    assert_eq!(status.text, "UPLOADED ORIGINAL");
    assert!(r.svc.transient_status(3.0).is_none(), "reverts after 2 s");
    assert!(
        r.state
            .upload_history
            .is_uploaded(&clip.canonicalize().unwrap())
    );
    let req = &r.requests()[0];
    assert_eq!(req["action"], json!("upload"));
    assert_eq!(req["path"], json!(clip.to_string_lossy()));
    let roots: Vec<String> = serde_json::from_value(req["allowed_roots"].clone()).unwrap();
    assert!(
        roots.contains(
            &r.clips
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        )
    );
    assert!(r.error_titles().is_empty());
}

#[test]
fn queue_dedups_and_waits_for_readiness() {
    let mut r = rig(&uploader_ok("https://cdn.example/b.mp4"), json!({}));
    let clip = r.clip("b.mp4");
    r.state.readiness.engine_committed(&clip, true);
    r.svc.enqueue(&mut r.state, clip.clone());
    r.svc.enqueue(&mut r.state, clip.clone());
    for i in 0..5 {
        r.svc.tick(&mut r.state, i as f64 * 0.1);
    }
    assert!(r.requests().is_empty(), "waits while finalizing");
    r.state.readiness.complete(&clip);
    r.settle();
    assert_eq!(r.requests().len(), 1, "duplicates dropped");
    // Already uploaded: nothing queued.
    r.svc.enqueue(&mut r.state, clip.clone());
    r.settle();
    assert_eq!(r.requests().len(), 1);

    let failed = r.clip("c.mp4");
    r.state.readiness.engine_committed(&failed, true);
    r.svc.enqueue(&mut r.state, failed.clone());
    r.state.readiness.finalization_failed(&failed, "x", false);
    r.settle();
    assert_eq!(r.requests().len(), 1, "a failed finalization never uploads");
    assert!(!r.state.upload_status.contains_key(&failed));
    // Partial files and images are never queued.
    let partial = r.clip("d.partial.mp4");
    let image = r.clip("e.png");
    r.svc.enqueue(&mut r.state, partial);
    r.svc.enqueue(&mut r.state, image);
    assert!(!r.svc.busy());
}

#[test]
fn settings_problem_offers_the_settings() {
    let mut r = rig(
        r#"cat >/dev/null; echo '{"ok":false,"message":"Your server rejected the authorization header (HTTP 401).","upload_info":{},"settings_problem":true}'; exit 1"#,
        json!({}),
    );
    let clip = r.clip("f.mp4");
    r.svc.enqueue(&mut r.state, clip);
    r.settle();
    let e = r.state.error_bar.current().unwrap().clone();
    assert_eq!(e.title, "[Error 020] UPLOAD FAILED");
    let labels: Vec<&str> = e.actions.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, vec!["OPEN UPLOAD SETTINGS", "RETRY NOW"]);
    r.state.page = fthrust_ui::state::Page::Clips;
    assert!(r.svc.error_action(&mut r.state, &e.actions[0].id));
    assert_eq!(r.state.page, fthrust_ui::state::Page::Settings);
}

#[test]
fn failure_toast_with_retry() {
    let mut r = rig(
        r#"read -r line; echo "$line" >> "$(dirname "$0")/requests.jsonl"; echo '{"ok":false,"message":"Server said no"}'; exit 1"#,
        json!({}),
    );
    let clip = r.clip("f.mp4");
    r.svc.enqueue(&mut r.state, clip.clone());
    r.settle();
    assert_eq!(
        r.state.pending_cards,
        vec![CardKind::UploadFailed {
            detail: "Server said no".into()
        }]
    );
    let e = r.state.error_bar.current().unwrap().clone();
    assert_eq!(e.title, "[Error 020] UPLOAD FAILED");
    assert_eq!(e.detail, "Server said no");
    assert_eq!(e.actions[0].label, "RETRY NOW");
    assert!(!r.state.upload_status.contains_key(&clip));
    // The action re-queues.
    assert!(r.svc.error_action(&mut r.state, &e.actions[0].id));
    r.settle();
    assert_eq!(r.requests().len(), 2);

    let mut r = rig(
        r#"cat >/dev/null; echo '{"ok":false,"message":""}'"#,
        json!({}),
    );
    let clip = r.clip("g.mp4");
    r.svc.enqueue(&mut r.state, clip);
    r.settle();
    // `_do_single_upload` turns an empty message into "Upload failed".
    assert_eq!(r.state.error_bar.current().unwrap().detail, "Upload failed");
}

#[test]
fn disabled_does_nothing() {
    let mut r = rig(
        &uploader_ok("https://x/y"),
        json!({"upload_enabled": false}),
    );
    let clip = r.clip("h.mp4");
    r.svc.enqueue(&mut r.state, clip);
    r.settle();
    assert!(r.requests().is_empty());
    assert!(r.state.pending_cards.is_empty());
}

#[test]
fn missing_binary_message() {
    let mut r = rig(&uploader_ok("https://x/y"), json!({}));
    r.svc = UploadService::with_exe(None);
    let clip = r.clip("i.mp4");
    r.svc.enqueue(&mut r.state, clip);
    r.settle();
    assert_eq!(
        r.state.error_bar.current().unwrap().detail,
        "The uploader component is missing from this installation."
    );
}

#[test]
fn oversized_catbox_asks_then_compresses() {
    let mut r = rig(
        &uploader_ok("https://files.catbox.moe/z.mp4"),
        json!({"upload_provider": "catbox"}),
    );
    let clip = r.clips.join("Desktop").join("big.mp4");
    if support::write_test_clip(&clip, 3, true).is_none() {
        eprintln!("no encoder; skipped");
        return;
    }
    let size = std::fs::metadata(&clip).unwrap().len();
    r.svc.set_limit_bytes_for_tests(size - 1);
    r.svc.enqueue(&mut r.state, clip.clone());
    r.settle();
    assert!(
        r.requests().is_empty(),
        "nothing uploaded before the answer"
    );
    let Some(DialogKind::Question {
        title,
        message,
        tag,
    }) = r.state.dialogs.modal.back().cloned()
    else {
        panic!("no question");
    };
    assert_eq!(title, "Compress before upload?");
    assert!(
        message.starts_with(&format!(
            "big.mp4 is {:.1} MB. Catbox accepts files up to ",
            size as f64 / 1048576.0
        )),
        "{message}"
    );
    assert!(message.ends_with(
        "\n\nAutomatically compress a copy and upload it? The original clip will stay unchanged."
    ));
    r.state.dialogs.modal.clear();
    r.state
        .dialogs
        .results
        .push(fthrust_ui::state::DialogResult::Answer { tag, yes: true });
    r.settle();
    let reqs = r.requests();
    assert_eq!(reqs.len(), 1, "the copy is uploaded");
    let sent = PathBuf::from(reqs[0]["path"].as_str().unwrap());
    assert!(sent.starts_with(r.clips.join("Shared")), "{sent:?}");
    let name = sent.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("big_catbox_upload_") && name.ends_with(".mp4"),
        "{name}"
    );
    assert!(std::fs::metadata(&sent).unwrap().len() < size);
    assert!(clip.exists(), "original kept");
    let h = History::load(&r.state.paths);
    let alias = h.get(&clip.canonicalize().unwrap()).unwrap();
    assert_eq!(alias.0["uploaded_copy"], json!("compressed"));
    assert_eq!(alias.0["original_preserved"], json!(true));
    assert_eq!(
        r.svc.transient_status(0.5).unwrap().text,
        "UPLOADED COMPRESSED COPY"
    );
    assert_eq!(
        r.state.pending_cards,
        vec![CardKind::Upload {
            filename: "big.mp4 · COMPRESSED COPY".into()
        }]
    );
    assert!(
        r.error_titles()
            .iter()
            .any(|t| t == "COMPRESSED COPY UPLOADED"),
        "{:?}",
        r.error_titles()
    );

    // With automatic compression on, no question is asked.
    let mut r2 = rig(
        &uploader_ok("https://files.catbox.moe/z2.mp4"),
        json!({"upload_provider": "catbox", "upload_auto_compress": true}),
    );
    let clip2 = r2.clips.join("Desktop").join("big2.mp4");
    std::fs::copy(&clip, &clip2).unwrap();
    r2.svc.set_limit_bytes_for_tests(size - 1);
    r2.svc.enqueue(&mut r2.state, clip2);
    r2.settle();
    assert!(r2.state.dialogs.modal.is_empty());
    assert_eq!(r2.requests().len(), 1);
}

#[test]
fn immediate_mode_on_save() {
    let mut r = rig(
        &uploader_ok("https://x/i"),
        json!({"upload_mode": "immediate"}),
    );
    let clip = r.clip("saved.mp4");
    r.state.saved_clips.push(clip.clone());
    r.settle();
    assert_eq!(r.requests().len(), 1);
    let mut r = rig(
        &uploader_ok("https://x/i"),
        json!({"upload_mode": "manual"}),
    );
    let clip = r.clip("saved.mp4");
    r.state.saved_clips.push(clip);
    r.settle();
    assert!(r.requests().is_empty());
}

#[test]
fn interval_scan_includes_imports_and_skips_fresh_files() {
    let mut r = rig(
        &uploader_ok("https://x/s"),
        json!({"upload_mode": "interval", "upload_interval_value": 1, "upload_interval_unit": "minutes"}),
    );
    let old = SystemTime::now() - Duration::from_secs(120);
    let a = r.clip("old.mp4");
    std::fs::File::options()
        .write(true)
        .open(&a)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let fresh = r.clip("fresh.mp4");
    let imports = r.dir.path().join("imports");
    std::fs::create_dir_all(&imports).unwrap();
    let b = imports.join("imported.mkv");
    std::fs::write(&b, b"x").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&b)
        .unwrap()
        .set_modified(old)
        .unwrap();
    r.state
        .settings
        .set("imported_clip_folders", json!([imports.to_string_lossy()]));
    // Before the first interval nothing happens.
    r.svc.tick(&mut r.state, 1.0);
    r.svc.tick(&mut r.state, 59.0);
    assert!(!r.svc.busy());
    r.svc.tick(&mut r.state, 61.0);
    let start = Instant::now();
    while r.requests().len() < 2 && start.elapsed() < Duration::from_secs(20) {
        r.svc.tick(&mut r.state, 61.5);
        std::thread::sleep(Duration::from_millis(20));
    }
    r.settle();
    let sent: Vec<String> = r
        .requests()
        .iter()
        .map(|q| q["path"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent.iter().any(|p| p.ends_with("old.mp4")));
    assert!(sent.iter().any(|p| p.ends_with("imported.mkv")));
    assert!(!sent.iter().any(|p| p == &fresh.to_string_lossy()));
}

#[test]
fn shutdown_kills_child() {
    let mut r = rig(
        "cat >/dev/null; echo $$ > \"$(dirname \"$0\")/pid\"; sleep 30",
        json!({}),
    );
    let clip = r.clip("k.mp4");
    r.svc.enqueue(&mut r.state, clip);
    let pid_file = r.dir.path().join("pid");
    let start = Instant::now();
    while !pid_file.exists() {
        r.svc.tick(&mut r.state, 0.0);
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let started = Instant::now();
    r.svc.shutdown();
    assert!(started.elapsed() < Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(100));
    let alive = Path::new(&format!("/proc/{pid}")).exists()
        && !std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .unwrap_or_default()
            .contains(") Z ");
    assert!(!alive, "uploader {pid} still running");
}

#[allow(dead_code)]
fn unused(_: Entry) {}

#[test]
fn turning_uploads_off_drops_the_queue_quietly() {
    let body = format!("sleep 0.5\n{}", uploader_ok("https://x/q"));
    let mut r = rig(&body, json!({}));
    let clips: Vec<PathBuf> = (0..3).map(|i| r.clip(&format!("q{i}.mp4"))).collect();
    for c in &clips {
        r.svc.enqueue(&mut r.state, c.clone());
    }
    r.svc.tick(&mut r.state, 0.0);
    r.state
        .uploader_settings
        .set("upload_enabled", json!(false));
    r.settle();
    assert!(r.requests().len() <= 1, "{:?}", r.requests());
    assert!(r.error_titles().is_empty(), "{:?}", r.error_titles());
    for c in &clips[1..] {
        assert!(!r.state.upload_status.contains_key(c));
    }
}

#[test]
fn scanned_paths_map_to_library_paths_behind_symlinks() {
    let mut r = rig(
        &format!("sleep 0.3\n{}", uploader_ok("https://x/s")),
        json!({"upload_mode": "interval", "upload_interval_value": 1}),
    );
    // The clips folder is reached through a symlink.
    let real = r.dir.path().join("real");
    std::fs::create_dir_all(real.join("Desktop")).unwrap();
    let link = r.dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    r.state
        .settings
        .set("clips_directory", json!(link.to_string_lossy()));
    r.state.library.clips_root = link.clone();
    let clip = link.join("Desktop").join("s.mp4");
    std::fs::write(&clip, b"x").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&clip)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(120))
        .unwrap();
    r.state.library.upsert(&clip, false);
    r.svc.tick(&mut r.state, 0.0);
    r.svc.tick(&mut r.state, 61.0);
    let start = Instant::now();
    while !r.svc.is_busy(&clip) {
        r.svc.tick(&mut r.state, 61.5);
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "never queued under its library path"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(r.state.upload_status.contains_key(&clip));
    // A click on the card does not start a second upload.
    r.svc.enqueue(&mut r.state, clip.clone());
    r.settle();
    assert_eq!(r.requests().len(), 1);
}
