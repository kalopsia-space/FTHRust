//! SFTP uploads, host keys and saved passwords (notes-uploader-reference 2.2).

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use fthrust_core::FthrPaths;
use fthrust_core::upload::history::History;
use fthrust_core::upload::settings::UploaderSettings;
use fthrust_uploader::{Env, handle, vault};
use serde_json::{Value, json};
use support::sftp_server::{self as srv, TestServer};

struct Home {
    dir: tempfile::TempDir,
    paths: FthrPaths,
    clip: PathBuf,
}

fn home(server: &TestServer, extra: Value) -> Home {
    let dir = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(dir.path());
    let mut s = UploaderSettings::default();
    s.set("upload_enabled", json!(true));
    s.accept_terms("2026-10-23T00:00:00+00:00");
    s.set("upload_provider", json!("sftp"));
    s.set("sftp_host", json!("127.0.0.1"));
    s.set("sftp_port", json!(server.port.to_string()));
    s.set("sftp_username", json!(srv::USER));
    s.set("sftp_directory", json!("up"));
    for (k, v) in extra.as_object().unwrap() {
        s.set(k, v.clone());
    }
    s.save(&paths).unwrap();
    let clips = dir.path().join("clips");
    std::fs::create_dir_all(&clips).unwrap();
    let clip = clips.join("Clip One.MP4");
    std::fs::write(&clip, vec![7u8; 300_000]).unwrap();
    Home { dir, paths, clip }
}

fn pinned(server: &TestServer) -> Value {
    json!({"sftp_fingerprint": server.fingerprint()})
}

impl Home {
    fn env(&self) -> Env {
        Env::for_tests(self.paths.clone())
    }

    async fn test(&self, env: &Env, password: Option<&str>) -> (bool, String) {
        let mut req = json!({"action": "test_connection"});
        if let Some(p) = password {
            req["config"] = json!({"sftp_password": p});
        }
        let r = handle(env, &req).await;
        (r.ok, r.message)
    }

    async fn upload(&self, env: &Env) -> (bool, String) {
        let req = json!({
            "action": "upload",
            "path": self.clip,
            "allowed_roots": [self.dir.path().join("clips")],
        });
        let r = handle(env, &req).await;
        (r.ok, r.message)
    }

    async fn save_password(&self, env: &Env, password: &str) -> (bool, String) {
        let settings = UploaderSettings::load(&self.paths);
        let mut config = settings.data().clone();
        config.insert("sftp_password".into(), json!(password));
        let r = handle(env, &json!({"action": "sftp_save", "config": config})).await;
        (r.ok, r.message)
    }

    fn remembered(&self) -> String {
        std::fs::read_to_string(self.paths.uploader_known_hosts_file()).unwrap_or_default()
    }
}

const UNTRUSTED: &str = "This SFTP host is not trusted yet. Enter its SHA256 fingerprint from the server operator and test the connection once, or add its verified key to your SSH known_hosts file.";

#[tokio::test]
async fn pin_mismatch_never_authenticates() {
    let server = srv::start(1).await;
    let other = srv::start(2).await;
    let h = home(&server, json!({"sftp_fingerprint": other.fingerprint()}));
    let (ok, msg) = h.test(&h.env(), Some(srv::PASSWORD)).await;
    assert!(!ok);
    assert_eq!(
        msg,
        "SFTP host-key mismatch. Connection stopped before sending credentials."
    );
    assert_eq!(server.knobs.auth_attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unknown_host_without_pin_refused() {
    let server = srv::start(3).await;
    let h = home(&server, json!({}));
    assert_eq!(
        h.test(&h.env(), Some(srv::PASSWORD)).await,
        (false, UNTRUSTED.into())
    );
    assert_eq!(server.knobs.auth_attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn known_hosts_trusts_and_changed_key_refused() {
    let server = srv::start(4).await;
    let h = home(&server, json!({}));
    let ssh = h.dir.path().join(".ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::write(
        ssh.join("known_hosts"),
        format!(
            "# comment\n@cert-authority * ssh-ed25519 AAAA\nother.host ssh-ed25519 garbage\n{}\n",
            server.known_hosts_line()
        ),
    )
    .unwrap();
    let (ok, msg) = h.test(&h.env(), Some(srv::PASSWORD)).await;
    assert!(ok, "{msg}");
    assert_eq!(
        msg,
        "SFTP authenticated; host key verified and remote folder found. Write access is checked on upload."
    );
    // A different key recorded for the host is a changed key.
    let other = srv::start(5).await;
    let line = other
        .known_hosts_line()
        .replace(&other.port.to_string(), &server.port.to_string());
    std::fs::write(ssh.join("known_hosts"), format!("{line}\n")).unwrap();
    let (ok, msg) = h.test(&h.env(), Some(srv::PASSWORD)).await;
    assert!(!ok);
    assert_eq!(
        msg,
        "SFTP host-key mismatch. Connection stopped before sending credentials. Verify the changed key with the server operator."
    );
}

#[tokio::test]
async fn verified_pin_is_remembered() {
    let server = srv::start(6).await;
    let h = home(&server, pinned(&server));
    std::fs::create_dir_all(h.paths.uploader_dir()).unwrap();
    std::fs::write(
        h.paths.uploader_known_hosts_file(),
        "keep.example ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHx\n",
    )
    .unwrap();
    let (ok, msg) = h.test(&h.env(), Some(srv::PASSWORD)).await;
    assert!(ok, "{msg}");
    let text = h.remembered();
    assert!(text.contains("keep.example ssh-ed25519"), "{text}");
    assert!(
        text.contains(&format!("[127.0.0.1]:{} ssh-ed25519 ", server.port)),
        "{text}"
    );
    // Now trusted without the pin.
    let h2 = Home {
        dir: h.dir,
        paths: h.paths.clone(),
        clip: h.clip.clone(),
    };
    let mut s = UploaderSettings::load(&h2.paths);
    s.set("sftp_fingerprint", json!(""));
    s.save(&h2.paths).unwrap();
    assert!(h2.test(&h2.env(), Some(srv::PASSWORD)).await.0);
    // Remembering again does not duplicate the line.
    let mut s = UploaderSettings::load(&h2.paths);
    s.set("sftp_fingerprint", json!(server.fingerprint()));
    s.save(&h2.paths).unwrap();
    assert!(h2.test(&h2.env(), Some(srv::PASSWORD)).await.0);
    assert_eq!(h2.remembered().matches("[127.0.0.1]").count(), 1);
}

#[tokio::test]
async fn auth_failure_sanitised() {
    let server = srv::start(7).await;
    let h = home(&server, pinned(&server));
    let (ok, msg) = h.test(&h.env(), Some("wrong-secret-xyz")).await;
    assert!(!ok);
    assert_eq!(
        msg,
        "SFTP authentication failed. Check the username and password."
    );
    assert!(!msg.contains("wrong-secret-xyz"));
    // Upload never retries a failed login.
    let env = h.env();
    env.vault
        .set(
            &vault::service_name("127.0.0.1", server.port, srv::USER),
            srv::USER,
            "nope",
        )
        .unwrap();
    let before = server.knobs.auth_attempts.load(Ordering::SeqCst);
    let (ok, msg) = h.upload(&env).await;
    assert!(!ok);
    assert_eq!(
        msg,
        "SFTP authentication failed. Check the username and password."
    );
    assert!(server.knobs.auth_attempts.load(Ordering::SeqCst) - before <= 2);
}

#[tokio::test]
async fn test_checks_folder_without_writing() {
    let server = srv::start(8).await;
    let h = home(&server, pinned(&server));
    assert!(h.test(&h.env(), Some(srv::PASSWORD)).await.0);
    assert!(srv::names(&server.root.join("up")).is_empty());
    let h = home(
        &server,
        json!({"sftp_fingerprint": server.fingerprint(), "sftp_directory": "/afile"}),
    );
    assert_eq!(
        h.test(&h.env(), Some(srv::PASSWORD)).await,
        (
            false,
            "The configured remote folder is not a directory.".into()
        )
    );
}

#[tokio::test]
async fn upload_part_rename_and_urls() {
    let server = srv::start(9).await;
    let h = home(&server, pinned(&server));
    let env = h.env();
    assert_eq!(
        h.save_password(&env, srv::PASSWORD).await,
        (true, "SFTP credentials updated.".into())
    );
    let (ok, msg) = h.upload(&env).await;
    assert!(ok, "{msg}");
    let files = srv::names(&server.root.join("up"));
    assert_eq!(files.len(), 1, "{files:?}");
    let name = &files[0];
    assert!(
        name.starts_with("fthr-") && name.ends_with(".mp4"),
        "{name}"
    );
    assert_eq!(name.len(), "fthr-".len() + 32 + ".mp4".len());
    let remote = server.root.join("up").join(name);
    assert_eq!(std::fs::metadata(&remote).unwrap().len(), 300_000);
    assert_eq!(
        std::fs::metadata(&remote).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let expected = format!("sftp://127.0.0.1:{}/up/{name}", server.port);
    assert_eq!(msg, expected);
    let e = History::load(&h.paths)
        .get(&h.clip.canonicalize().unwrap())
        .unwrap();
    assert_eq!(e.0["provider"], json!("sftp"));
    assert_eq!(e.0["sftp_url"], json!(expected));
    assert_eq!(e.0["public_url"], json!(""));
    assert_eq!(e.0["remote_path"], json!(format!("/up/{name}")));
    assert_eq!(e.0["url"], json!(expected));
    assert!(h.clip.exists());

    // Public links: readable file and an HTTPS link of base + name.
    let h = home(
        &server,
        json!({"sftp_fingerprint": server.fingerprint(), "sftp_public_enabled": true,
               "sftp_public_base_url": "https://videos.example/clips"}),
    );
    let env = h.env();
    h.save_password(&env, srv::PASSWORD).await;
    let (ok, msg) = h.upload(&env).await;
    assert!(ok, "{msg}");
    let newest = srv::names(&server.root.join("up"))
        .into_iter()
        .find(|n| msg.ends_with(n.as_str()))
        .unwrap();
    assert_eq!(msg, format!("https://videos.example/clips/{newest}"));
    let mode = std::fs::metadata(server.root.join("up").join(&newest))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o644);
}

#[tokio::test]
async fn failed_rename_cleans_part_and_keeps_clip() {
    let server = srv::start(10).await;
    server.knobs.fail_rename.store(true, Ordering::SeqCst);
    let h = home(&server, pinned(&server));
    let env = h.env();
    h.save_password(&env, srv::PASSWORD).await;
    let (ok, msg) = h.upload(&env).await;
    assert!(!ok);
    assert_eq!(
        msg,
        "SFTP connection or transfer failed. Check the server, folder and permissions."
    );
    assert!(srv::names(&server.root.join("up")).is_empty());
    assert!(h.clip.exists());
}

#[tokio::test]
async fn chmod_failure_fails_without_link() {
    let server = srv::start(11).await;
    server.knobs.fail_chmod_644.store(true, Ordering::SeqCst);
    let h = home(
        &server,
        json!({"sftp_fingerprint": server.fingerprint(), "sftp_public_enabled": true,
               "sftp_public_base_url": "https://videos.example/clips/", "upload_auto_delete": true}),
    );
    let env = h.env();
    h.save_password(&env, srv::PASSWORD).await;
    let (ok, _) = h.upload(&env).await;
    assert!(!ok);
    assert!(h.clip.exists());
    assert!(
        History::load(&h.paths)
            .get(&h.clip.canonicalize().unwrap())
            .is_none()
    );
}

#[tokio::test]
async fn unsafe_public_url_rejected_before_network() {
    let server = srv::start(12).await;
    let h = home(
        &server,
        json!({"sftp_public_enabled": true, "sftp_public_base_url": "http://plain.example/"}),
    );
    let (ok, msg) = h.upload(&h.env()).await;
    assert!(!ok);
    assert_eq!(
        msg,
        "Enter a public HTTPS folder URL without credentials, query or fragment."
    );
    assert_eq!(server.knobs.auth_attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn password_from_vault_scoped_by_host_port_user() {
    let server = srv::start(13).await;
    let h = home(&server, pinned(&server));
    let env = h.env();
    let (ok, _) = h.upload(&env).await;
    assert!(!ok);
    assert_eq!(
        h.upload(&env).await.1,
        "Enter and save an SFTP password first."
    );
    h.save_password(&env, srv::PASSWORD).await;
    assert!(h.upload(&env).await.0);
    // Another user name has no saved password.
    let mut s = UploaderSettings::load(&h.paths);
    s.set("sftp_username", json!("someone"));
    s.save(&h.paths).unwrap();
    assert_eq!(
        h.upload(&env).await.1,
        "Enter and save an SFTP password first."
    );
    assert_eq!(
        vault::service_name("h", 22, "u"),
        format!("FTHR Clips/SFTP/{}", {
            use sha2::Digest;
            let d = sha2::Sha256::digest(b"h\x0022\x00u");
            d.iter().map(|b| format!("{b:02x}")).collect::<String>()
        })
    );
}

#[tokio::test]
async fn save_and_forget() {
    let server = srv::start(14).await;
    let h = home(&server, pinned(&server));
    let env = h.env();
    // No password: validation only.
    let settings = UploaderSettings::load(&h.paths);
    let r = handle(
        &env,
        &json!({"action": "sftp_save", "config": settings.data()}),
    )
    .await;
    assert_eq!(
        (r.ok, r.message.as_str()),
        (true, "SFTP credentials updated.")
    );
    assert!(h.save_password(&env, " spaced ").await.0);
    let service = vault::service_name("127.0.0.1", server.port, srv::USER);
    assert_eq!(
        env.vault.get(&service, srv::USER).unwrap(),
        Some(" spaced ".into())
    );
    let text = std::fs::read_to_string(h.paths.uploader_settings_file()).unwrap();
    assert!(!text.contains("spaced"));
    let r = handle(
        &env,
        &json!({"action": "sftp_forget", "config": settings.data()}),
    )
    .await;
    assert!(r.ok);
    assert_eq!(env.vault.get(&service, srv::USER).unwrap(), None);
    // Forgetting twice is fine.
    let r = handle(
        &env,
        &json!({"action": "sftp_forget", "config": settings.data()}),
    )
    .await;
    assert!(r.ok);
}
