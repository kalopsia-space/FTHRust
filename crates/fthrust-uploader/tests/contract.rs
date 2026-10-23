//! The one-shot contract and its gating (notes-uploader-reference 1).

use std::io::Write;
use std::process::{Command, Stdio};

use fthrust_core::FthrPaths;
use fthrust_core::upload::settings::UploaderSettings;
use fthrust_uploader::{Env, handle};
use serde_json::{Value, json};

struct Home {
    dir: tempfile::TempDir,
    paths: FthrPaths,
}

fn home(settings: Value) -> Home {
    let dir = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(dir.path());
    let mut s = UploaderSettings::default();
    for (k, v) in settings.as_object().unwrap() {
        s.set(k, v.clone());
    }
    s.save(&paths).unwrap();
    Home { dir, paths }
}

fn consented(extra: Value) -> Home {
    let mut s = json!({
        "upload_enabled": true,
        "uploader_terms_accepted_version": fthrust_core::upload::TERMS_VERSION,
        "uploader_privacy_accepted_version": fthrust_core::upload::PRIVACY_VERSION,
    });
    for (k, v) in extra.as_object().unwrap() {
        s[k] = v.clone();
    }
    home(s)
}

async fn ask(h: &Home, req: Value) -> (bool, String) {
    let env = Env::for_tests(h.paths.clone());
    let r = handle(&env, &req).await;
    (r.ok, r.message)
}

#[tokio::test]
async fn unknown_and_missing_action() {
    let h = consented(json!({}));
    assert_eq!(
        ask(&h, json!({"action": "dance"})).await,
        (false, "Unsupported uploader action: dance".into())
    );
    assert_eq!(
        ask(&h, json!({})).await,
        (false, "Unsupported uploader action: (missing)".into())
    );
    assert_eq!(
        ask(&h, json!([1])).await,
        (false, "Uploader request must be a JSON object.".into())
    );
}

#[tokio::test]
async fn retired_actions_rejected() {
    let h = consented(json!({"upload_provider": "custom"}));
    for a in ["register", "login", "account_status", "logout"] {
        let (ok, msg) = ask(&h, json!({"action": a})).await;
        assert!(!ok);
        assert_eq!(msg, format!("Unsupported uploader action: {a}"));
    }
}

#[tokio::test]
async fn disabled_refuses_network_actions() {
    let h = consented(json!({"upload_enabled": false}));
    for a in ["upload", "test_connection"] {
        assert_eq!(
            ask(&h, json!({"action": a})).await,
            (
                false,
                "The Upload Extension is disabled in FTHR Clips.".into()
            )
        );
    }
    // Credential management still validates while disabled.
    let (ok, msg) = ask(
        &h,
        json!({"action": "sftp_save", "config": {"sftp_host": ""}}),
    )
    .await;
    assert!(!ok);
    assert_eq!(
        msg,
        "Enter a host domain or IP address without a URL or path."
    );
}

#[tokio::test]
async fn old_terms_refused() {
    let h = consented(json!({"uploader_terms_accepted_version": "2026-10-15-v3"}));
    assert_eq!(
        ask(&h, json!({"action": "test_connection"})).await,
        (
            false,
            "Uploader Terms of Service must be accepted again.".into()
        )
    );
    let h = consented(json!({"uploader_privacy_accepted_version": "2026-10-15-v3"}));
    assert_eq!(
        ask(&h, json!({"action": "sftp_forget", "config": {}})).await,
        (
            false,
            "Uploader Privacy Policy must be accepted again.".into()
        )
    );
}

#[tokio::test]
async fn catbox_needs_consent() {
    let h = consented(json!({"upload_provider": "catbox"}));
    assert_eq!(
        ask(&h, json!({"action": "test_connection"})).await,
        (
            false,
            "Accept Catbox\u{2019}s legal policies in FTHR Clips first.".into()
        )
    );
    // The request config can select Catbox too.
    let h = consented(json!({"upload_provider": "sftp"}));
    let (_, msg) = ask(
        &h,
        json!({"action": "test_connection", "config": {"upload_provider": "CATBOX"}}),
    )
    .await;
    assert!(msg.starts_with("Accept Catbox"), "{msg}");
}

#[tokio::test]
async fn custom_and_sftp_need_no_provider_consent() {
    let h = consented(json!({"upload_provider": "your_server", "upload_server_url": ""}));
    assert_eq!(
        ask(&h, json!({"action": "test_connection"})).await,
        (
            false,
            "Set a server URL before connecting to your server.".into()
        )
    );
    let h = consented(json!({"upload_provider": "sftp", "sftp_host": "bad host"}));
    let (_, msg) = ask(&h, json!({"action": "test_connection"})).await;
    assert_eq!(
        msg,
        "Enter a host domain or IP address without a URL or path."
    );
}

#[tokio::test]
async fn retired_provider_unsupported() {
    let h = consented(json!({"upload_provider": "custom"}));
    assert_eq!(
        ask(
            &h,
            json!({"action": "test_connection", "config": {"upload_provider": "lustful"}})
        )
        .await,
        (false, "Unsupported upload provider.".into())
    );
}

#[tokio::test]
async fn upload_outside_roots_refused() {
    let h =
        consented(json!({"upload_provider": "custom", "upload_server_url": "http://127.0.0.1:9/"}));
    let clips = h.dir.path().join("clips");
    let other = h.dir.path().join("other");
    std::fs::create_dir_all(&clips).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let clip = other.join("a.mp4");
    std::fs::write(&clip, b"x").unwrap();
    let req = |roots: Value| json!({"action": "upload", "path": clip, "allowed_roots": roots});
    let refused = (
        false,
        "The uploader may only read approved clip folders.".to_owned(),
    );
    assert_eq!(ask(&h, req(json!([clips]))).await, refused);
    assert_eq!(ask(&h, req(json!([]))).await, refused);
    // A symlink inside the clips folder pointing outside is still outside.
    let link = clips.join("l.mp4");
    std::os::unix::fs::symlink(&clip, &link).unwrap();
    let r = json!({"action": "upload", "path": link, "allowed_roots": [clips]});
    assert_eq!(ask(&h, r).await, refused);
}

#[tokio::test]
async fn missing_clip() {
    let h =
        consented(json!({"upload_provider": "custom", "upload_server_url": "http://127.0.0.1:9/"}));
    let gone = h.dir.path().join("gone.mp4");
    let r = json!({"action": "upload", "path": gone, "allowed_roots": [h.dir.path()]});
    assert_eq!(
        ask(&h, r).await,
        (false, "The selected clip does not exist.".into())
    );
    let r = json!({"action": "upload", "path": h.dir.path(), "allowed_roots": [h.dir.path()]});
    assert_eq!(
        ask(&h, r).await,
        (false, "The selected clip does not exist.".into())
    );
}

fn run_binary(h: &Home, stdin: &str) -> (Option<i32>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fthrust-uploader"))
        .env("HOME", h.dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code(), String::from_utf8(out.stdout).unwrap())
}

#[test]
fn binary_round_trip() {
    let h = consented(json!({"upload_enabled": false}));
    let (code, out) = run_binary(&h, r#"{"action":"upload"}"#);
    assert_eq!(code, Some(1));
    assert!(out.ends_with('\n'));
    assert_eq!(out.lines().count(), 1);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(
        v,
        json!({"ok": false, "message": "The Upload Extension is disabled in FTHR Clips."})
    );
    // Compact separators, like the reference.
    assert!(out.starts_with(r#"{"ok":false,"message":"#), "{out}");

    let (code, out) = run_binary(&h, "garbage");
    assert_eq!(code, Some(1));
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["ok"], json!(false));
    assert_eq!(v["message"], json!("Uploader request is not valid JSON."));
}

/// The keyring is reached from inside the uploader's tokio runtime; with
/// zbus's tokio feature unified in, a blocking call there used to panic.
#[test]
fn keyring_failure_is_a_message_not_a_panic() {
    let h = consented(json!({}));
    let req = r#"{"action":"sftp_forget","config":{"sftp_host":"h.example","sftp_username":"u"}}"#;
    let mut child = Command::new(env!("CARGO_BIN_EXE_fthrust-uploader"))
        .env("HOME", h.dir.path())
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/fthrust-test-bus",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(req.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let v: Value = serde_json::from_str(String::from_utf8(out.stdout).unwrap().trim()).unwrap();
    assert_eq!(
        v["message"],
        json!("Could not remove the saved SFTP password."),
        "{v}"
    );
}
