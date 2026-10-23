//! The uploader's no-network model (notes-uploader-reference sections 2, 3, 5).

use std::path::{Path, PathBuf};

use fthrust_core::FthrPaths;
use fthrust_core::settings::SettingsStore;
use fthrust_core::upload::history::{Entry, History};
use fthrust_core::upload::settings::{Provider, UploaderSettings, migrate_legacy_core_keys};
use fthrust_core::upload::validate;
use fthrust_core::upload::{CATBOX_LEGAL_VERSION, PRIVACY_VERSION, TERMS_VERSION};
use serde_json::{Map, Value, json};

fn home() -> (tempfile::TempDir, FthrPaths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(dir.path());
    (dir, paths)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[test]
fn settings_defaults_and_load_rules() {
    let (_d, paths) = home();
    let s = UploaderSettings::load(&paths);
    let expected = json!({
        "upload_enabled": false, "upload_provider": "catbox", "catbox_userhash": "",
        "upload_server_url": "", "upload_auth_header": "", "sftp_host": "", "sftp_port": 22,
        "sftp_username": "", "sftp_directory": ".", "sftp_fingerprint": "",
        "sftp_public_enabled": false, "sftp_public_base_url": "", "upload_mode": "manual",
        "upload_interval_value": 5, "upload_interval_unit": "minutes",
        "upload_auto_compress": false, "upload_auto_delete": false,
        "uploader_terms_accepted_version": "", "uploader_privacy_accepted_version": "",
        "uploader_accepted_at": "", "catbox_legal_accepted_version": "",
        "catbox_legal_accepted_at": ""
    });
    for (k, v) in expected.as_object().unwrap() {
        assert_eq!(s.get(k), Some(v), "{k}");
    }
    assert_eq!(s.provider(), Provider::Catbox);
    assert!(!s.enabled());

    write(
        &paths.uploader_settings_file(),
        r#"{"upload_enabled": true, "upload_provider": "own_server", "sftp_password": "x",
            "lustful_token": "t", "sftp_provider_name": "VaultScope"}"#,
    );
    let s = UploaderSettings::load(&paths);
    assert!(s.get("sftp_password").is_none());
    assert!(s.get("lustful_token").is_none());
    assert_eq!(s.get("sftp_provider_name"), Some(&json!("VaultScope")));
    assert_eq!(s.provider(), Provider::Custom);
    assert!(s.enabled());

    write(
        &paths.uploader_settings_file(),
        r#"{"upload_enabled": true, "upload_provider": "fthr"}"#,
    );
    let s = UploaderSettings::load(&paths);
    assert!(!s.enabled());
    assert_eq!(s.get("upload_provider"), Some(&json!("catbox")));

    write(&paths.uploader_settings_file(), "{not json");
    let s = UploaderSettings::load(&paths);
    assert_eq!(s.get("sftp_directory"), Some(&json!(".")));
}

#[test]
fn settings_save_sorted_atomic_without_password() {
    let (_d, paths) = home();
    let mut s = UploaderSettings::load(&paths);
    s.set("sftp_password", json!("secret"));
    s.set("zz_unknown", json!(1));
    s.save(&paths).unwrap();
    let text = std::fs::read_to_string(paths.uploader_settings_file()).unwrap();
    assert!(!text.contains("secret"));
    assert!(!paths.uploader_dir().join("settings.json.tmp").exists());
    let keys: Vec<String> = serde_json::from_str::<Map<String, Value>>(&text)
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    // Python's json.dumps(indent=2) layout.
    assert!(text.starts_with("{\n  \"catbox_legal_accepted_at\": \"\",\n"));
}

#[test]
fn consent_versions() {
    let (_d, paths) = home();
    assert_eq!(TERMS_VERSION, "2026-10-23-v4");
    assert_eq!(PRIVACY_VERSION, "2026-10-23-v4");
    assert_eq!(CATBOX_LEGAL_VERSION, "catbox-legal-2021-03-06");
    let mut s = UploaderSettings::load(&paths);
    s.set("uploader_terms_accepted_version", json!("2026-10-15-v3"));
    s.set("uploader_privacy_accepted_version", json!("2026-10-15-v3"));
    assert!(!s.consent_current());
    s.accept_terms("2026-10-23T12:00:00+00:00");
    assert!(s.consent_current());
    assert_eq!(
        s.get("uploader_accepted_at"),
        Some(&json!("2026-10-23T12:00:00+00:00"))
    );
    assert!(!s.catbox_consent_current());
    s.accept_catbox("2026-10-23T12:00:01+00:00");
    assert!(s.catbox_consent_current());
    assert_eq!(
        s.get("catbox_legal_accepted_at"),
        Some(&json!("2026-10-23T12:00:01+00:00"))
    );
    let now = fthrust_core::upload::utc_now_iso();
    assert_eq!(now.len(), 25, "{now}");
    assert!(now.ends_with("+00:00"));
}

#[test]
fn legacy_core_keys_migrate() {
    let (_d, paths) = home();
    let mut core = SettingsStore::load(paths.clone()).unwrap();
    let mut up = UploaderSettings::load(&paths);
    assert!(!migrate_legacy_core_keys(&mut core, &mut up));

    core.set("upload_enabled", json!(true));
    core.set("upload_server_url", json!("https://x.example/up"));
    core.set("upload_mode", json!("immediate"));
    assert!(migrate_legacy_core_keys(&mut core, &mut up));
    assert!(core.get("upload_server_url").is_none());
    assert!(core.get("upload_enabled").is_none());
    assert_eq!(up.get("upload_provider"), Some(&json!("custom")));
    assert_eq!(up.get("upload_mode"), Some(&json!("immediate")));
    assert!(up.enabled());

    let mut up = UploaderSettings::load(&paths);
    core.set("upload_auto_delete", json!(true));
    assert!(migrate_legacy_core_keys(&mut core, &mut up));
    assert_eq!(up.get("upload_provider"), Some(&json!("catbox")));
}

fn ok_entry(url: &str) -> Entry {
    Entry(map(json!({
        "status": "ok", "provider": "catbox", "uploaded_at": "2026-10-23T12:00:00+00:00",
        "url": url, "raw_url": url, "favorite": false
    })))
}

#[test]
fn history_rules() {
    let (_d, paths) = home();
    let clip = PathBuf::from("/clips/a.mp4");
    let mut h = History::load(&paths);
    assert!(!h.is_uploaded(&clip));
    h.record(&clip, ok_entry("https://files.catbox.moe/abc.mp4"));
    assert!(h.is_uploaded(&clip));
    assert_eq!(
        h.link(&clip).unwrap().url,
        "https://files.catbox.moe/abc.mp4"
    );
    assert!(!h.link(&clip).unwrap().sftp);
    h.save(&paths).unwrap();
    let h2 = History::load(&paths);
    assert!(h2.is_uploaded(&clip));

    // Failure entries never count.
    let mut h = History::default();
    h.record(
        &clip,
        Entry(map(json!({"status": "error", "url": "https://x/"}))),
    );
    assert!(h.get(&clip).is_none());
    assert!(!h.is_uploaded(&clip));

    // Expiry: past and not favourite → not uploaded; favourite keeps it.
    let mut e = ok_entry("https://x/a");
    e.0.insert("expires_at".into(), json!("2000-01-01T00:00:00Z"));
    h.record(&clip, e.clone());
    assert!(!h.is_uploaded(&clip));
    e.0.insert("favorite".into(), json!(true));
    h.record(&clip, e.clone());
    assert!(h.is_uploaded(&clip));
    e.0.insert("favorite".into(), json!(false));
    e.0.insert("expires_at".into(), json!("garbage"));
    h.record(&clip, e);
    assert!(h.is_uploaded(&clip));

    // Link validity.
    for (url, ok) in [
        ("http://h/x", true),
        ("sftp://host:2222/home/u/fthr-1.mp4", true),
        ("sftp://[::1]/x.mp4", true),
        ("sftp://user@host/x.mp4", false),
        ("sftp://host/", false),
        ("sftp://host:70000/x", false),
        ("ftp://host/x", false),
        ("", false),
    ] {
        h.record(&clip, ok_entry(url));
        assert_eq!(h.link(&clip).is_some(), ok, "{url}");
    }
    h.record(&clip, ok_entry("sftp://host/x.mp4"));
    assert!(h.link(&clip).unwrap().sftp);
    // raw_url is the fallback.
    let mut e = ok_entry("");
    e.0.insert("raw_url".into(), json!("https://raw/x"));
    h.record(&clip, e);
    assert_eq!(h.link(&clip).unwrap().url, "https://raw/x");

    // Compressed alias and rename.
    let copy = PathBuf::from("/clips/Shared/a_catbox_upload_12-00-00.mp4");
    h.record(&copy, ok_entry("https://c/x"));
    h.alias_compressed(&clip, &copy, 1234);
    let alias = h.get(&clip).unwrap();
    assert_eq!(alias.0["uploaded_copy"], json!("compressed"));
    assert_eq!(alias.0["uploaded_path"], json!(copy.to_string_lossy()));
    assert_eq!(alias.0["uploaded_size_bytes"], json!(1234));
    assert_eq!(alias.0["original_preserved"], json!(true));
    assert_eq!(alias.0["url"], json!("https://c/x"));
    let renamed = PathBuf::from("/clips/b.mp4");
    h.rename(&clip, &renamed);
    assert!(h.get(&clip).is_none());
    assert!(h.is_uploaded(&renamed));
}

#[test]
fn legacy_history_copied_once() {
    let (_d, paths) = home();
    write(
        &paths.legacy_upload_history_file(),
        r#"{"/c/a.mp4": {"status": "ok", "url": "https://l/a"}}"#,
    );
    History::migrate_legacy(&paths).unwrap();
    assert!(History::load(&paths).is_uploaded(Path::new("/c/a.mp4")));
    write(
        &paths.legacy_upload_history_file(),
        r#"{"/c/b.mp4": {"status": "ok"}}"#,
    );
    History::migrate_legacy(&paths).unwrap();
    assert!(!History::load(&paths).is_uploaded(Path::new("/c/b.mp4")));
}

fn sftp(extra: Value) -> Map<String, Value> {
    let mut m = map(json!({
        "sftp_host": "Example.COM.", "sftp_port": 22, "sftp_username": "me",
        "sftp_directory": "/up", "sftp_fingerprint": "", "sftp_public_enabled": false,
        "sftp_public_base_url": ""
    }));
    for (k, v) in extra.as_object().unwrap() {
        m.insert(k.clone(), v.clone());
    }
    m
}

#[test]
fn sftp_validation_messages() {
    let c = validate::sftp_config(&sftp(json!({}))).unwrap();
    assert_eq!(c.host, "example.com");
    assert_eq!(
        (c.port, c.username.as_str(), c.folder.as_str()),
        (22, "me", "/up")
    );
    assert_eq!(c.fingerprint, None);
    assert_eq!(
        validate::sftp_config(&sftp(json!({"sftp_host": "10.0.0.2"})))
            .unwrap()
            .host,
        "10.0.0.2"
    );
    assert_eq!(
        validate::sftp_config(&sftp(json!({"sftp_host": "bücher.de"})))
            .unwrap()
            .host,
        "xn--bcher-kva.de"
    );
    let err = |extra: Value| validate::sftp_config(&sftp(extra)).unwrap_err();
    assert_eq!(
        err(json!({"sftp_host": "https://x.com/path"})),
        "Enter a host domain or IP address without a URL or path."
    );
    assert_eq!(
        err(json!({"sftp_host": ""})),
        "Enter a host domain or IP address without a URL or path."
    );
    assert_eq!(
        err(json!({"sftp_port": 0})),
        "SFTP port must be between 1 and 65535."
    );
    assert_eq!(
        err(json!({"sftp_port": "abc"})),
        "SFTP port must be between 1 and 65535."
    );
    assert_eq!(
        validate::sftp_config(&sftp(json!({"sftp_port": "2222"})))
            .unwrap()
            .port,
        2222
    );
    assert_eq!(
        err(json!({"sftp_username": " "})),
        "Enter an SFTP username without control characters."
    );
    assert_eq!(
        err(json!({"sftp_username": "a\u{7}"})),
        "Enter an SFTP username without control characters."
    );
    assert_eq!(
        err(json!({"sftp_directory": "C:\\up"})),
        "Enter a remote folder using forward slashes."
    );
    assert_eq!(
        validate::sftp_config(&sftp(json!({"sftp_directory": ""})))
            .unwrap()
            .folder,
        "."
    );
    assert_eq!(
        err(json!({"sftp_fingerprint": "MD5:aa"})),
        "Enter the server SHA256 host-key fingerprint from its operator."
    );
    // 43 base64 chars whose last char carries non-zero padding bits.
    let bad = format!("SHA256:{}B", "A".repeat(42));
    assert_eq!(
        err(json!({"sftp_fingerprint": bad})),
        "Invalid SHA256 host-key fingerprint."
    );
    let good = format!("SHA256:{}A", "A".repeat(42));
    assert_eq!(
        validate::sftp_config(&sftp(json!({"sftp_fingerprint": good.clone()})))
            .unwrap()
            .fingerprint,
        Some(good)
    );
    // The public URL is checked first.
    let public = |url: &str| {
        sftp(json!({"sftp_public_enabled": true, "sftp_public_base_url": url, "sftp_port": 0}))
    };
    assert_eq!(
        validate::sftp_config(&public("http://v.example/c")).unwrap_err(),
        "Enter a public HTTPS folder URL without credentials, query or fragment."
    );
    for bad in [
        "https://u:p@v.example/",
        "https://v.example/?q=1",
        "https://v.example/#f",
        "https://v.example/a b",
        "https:///x",
    ] {
        assert!(validate::public_base_url(&public(bad)).is_err(), "{bad}");
    }
    assert_eq!(
        validate::public_base_url(&public("https://v.example/clips")).unwrap(),
        "https://v.example/clips/"
    );
    assert_eq!(
        validate::public_base_url(&public("https://v.example:8443/clips///")).unwrap(),
        "https://v.example:8443/clips/"
    );
    assert_eq!(
        validate::public_base_url(&sftp(json!({"sftp_public_base_url": "junk"}))).unwrap(),
        ""
    );
}

#[test]
fn custom_url_and_header() {
    assert_eq!(
        validate::custom_url("clips.example/upload").unwrap(),
        "https://clips.example/upload"
    );
    assert_eq!(
        validate::custom_url(" HTTP://h.example/u?k=1 ").unwrap(),
        "HTTP://h.example/u?k=1"
    );
    assert_eq!(
        validate::custom_url("").unwrap_err(),
        "Set a server URL before connecting to your server."
    );
    assert_eq!(
        validate::custom_url("https://").unwrap_err(),
        "Your server URL must use http:// or https://."
    );
    assert_eq!(
        validate::auth_header("  Bearer t ").unwrap(),
        Some("Bearer t".into())
    );
    assert_eq!(validate::auth_header("").unwrap(), None);
    assert_eq!(
        validate::auth_header("a\nb").unwrap_err(),
        "The authorization header contains an invalid line break."
    );
    assert_eq!(
        validate::save_server_url("clips.example/upload"),
        "https://clips.example/upload"
    );
    assert_eq!(validate::save_server_url(""), "");
}

#[test]
fn vaultscope_paste() {
    let ok = r#"{"providerName":"VaultScope","domain":"SFTP.vaultscope.de","port":2222,"username":"u1","password":"p w"}"#;
    let l = validate::vaultscope_paste(ok).unwrap();
    assert_eq!(
        (
            l.host.as_str(),
            l.port,
            l.username.as_str(),
            l.password.as_str()
        ),
        ("sftp.vaultscope.de", 2222, "u1", "p w")
    );
    let msg = "Copy the complete VaultScope login JSON, then paste again. It must include providerName, domain, port, username and password.";
    for bad in [
        "nope",
        r#"{"providerName":"VaultScope","domain":"evil.de","port":22,"username":"u","password":"p"}"#,
        r#"{"providerName":"VaultScope","domain":"sftp.vaultscope.de","port":true,"username":"u","password":"p"}"#,
        r#"{"providerName":"VaultScope","domain":"sftp.vaultscope.de","port":22,"username":"u"}"#,
        r#"{"providerName":"Other","domain":"sftp.vaultscope.de","port":22,"username":"u","password":"p"}"#,
    ] {
        assert_eq!(validate::vaultscope_paste(bad).unwrap_err(), msg, "{bad}");
    }
    assert!(validate::vaultscope_paste(&"x".repeat(70_000)).is_err());
}

#[test]
fn uploadable_and_roots() {
    assert!(validate::is_uploadable(Path::new("/c/a.MP4")));
    assert!(!validate::is_uploadable(Path::new("/c/a.partial.mp4")));
    assert!(!validate::is_uploadable(Path::new("/c/a.png")));
    let roots = vec![PathBuf::from("/clips"), PathBuf::from("/imports/one")];
    assert!(validate::under_roots(Path::new("/clips"), &roots));
    assert!(validate::under_roots(
        Path::new("/clips/Desktop/a.mp4"),
        &roots
    ));
    assert!(validate::under_roots(
        Path::new("/imports/one/x.mkv"),
        &roots
    ));
    assert!(!validate::under_roots(
        Path::new("/clips-other/a.mp4"),
        &roots
    ));
    assert!(!validate::under_roots(Path::new("/imports/a.mp4"), &[]));

    let (d, paths) = home();
    let mut core = SettingsStore::load(paths).unwrap();
    let imp = d.path().join("imp");
    core.set("imported_clip_folders", json!([imp.to_string_lossy(), 5]));
    let r = validate::allowed_roots(Path::new("/clips/../clips"), &core);
    assert_eq!(r, vec![PathBuf::from("/clips"), imp]);
}

#[test]
fn concurrent_history_updates_keep_every_entry() {
    let (_d, paths) = home();
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let paths = paths.clone();
            std::thread::spawn(move || {
                for i in 0..25 {
                    let p = PathBuf::from(format!("/c/{t}-{i}.mp4"));
                    History::update(&paths, |h| h.record(&p, ok_entry("https://x/y"))).unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let h = History::load(&paths);
    for t in 0..8 {
        for i in 0..25 {
            assert!(
                h.is_uploaded(Path::new(&format!("/c/{t}-{i}.mp4"))),
                "lost {t}-{i}"
            );
        }
    }
}
