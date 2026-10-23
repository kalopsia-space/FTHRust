//! Catbox and your own server (notes-uploader-reference 2.0, 2.1, 2.3, 5.3).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fthrust_core::FthrPaths;
use fthrust_core::upload::history::History;
use fthrust_core::upload::settings::UploaderSettings;
use fthrust_uploader::{Env, handle};
use serde_json::{Value, json};

#[derive(Clone, Debug, Default)]
struct Seen {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A one-thread HTTP server answering each request with the next reply.
struct Server {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

fn server(replies: Vec<(u16, &str)>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/upload?k=1", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let mut replies: VecDeque<(u16, String)> = replies
        .into_iter()
        .map(|(s, b)| (s, b.to_owned()))
        .collect();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or("").to_owned();
            let target = parts.next().unwrap_or("").to_owned();
            let mut headers = Vec::new();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                let h = h.trim_end();
                if h.is_empty() {
                    break;
                }
                let (k, v) = h.split_once(':').unwrap();
                headers.push((k.trim().to_owned(), v.trim().to_owned()));
            }
            let len: usize = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map(|(_, v)| v.parse().unwrap())
                .unwrap_or(0);
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            log.lock().unwrap().push(Seen {
                method,
                target,
                headers,
                body,
            });
            let (status, text) = replies.pop_front().unwrap_or((500, "no reply".into()));
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
        }
    });
    Server { url, seen }
}

impl Server {
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

struct Home {
    dir: tempfile::TempDir,
    paths: FthrPaths,
    clip: PathBuf,
}

fn home(settings: Value) -> Home {
    let dir = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(dir.path());
    let mut s = UploaderSettings::default();
    s.set("upload_enabled", json!(true));
    s.accept_terms("2026-10-23T00:00:00+00:00");
    s.accept_catbox("2026-10-23T00:00:00+00:00");
    for (k, v) in settings.as_object().unwrap() {
        s.set(k, v.clone());
    }
    s.save(&paths).unwrap();
    let clips = dir.path().join("clips");
    std::fs::create_dir_all(&clips).unwrap();
    let clip = clips.join("my \"clip\".mp4");
    std::fs::write(&clip, b"VIDEO-BYTES").unwrap();
    Home { dir, paths, clip }
}

impl Home {
    fn env(&self, catbox: &str) -> Env {
        let mut env = Env::for_tests(self.paths.clone());
        env.catbox_url = catbox.to_owned();
        env
    }

    async fn upload(&self, env: &Env) -> (bool, String, Value) {
        let req = json!({
            "action": "upload",
            "path": self.clip,
            "allowed_roots": [self.dir.path().join("clips")],
        });
        let r = handle(env, &req).await;
        let info = r.upload_info.map(Value::Object).unwrap_or(Value::Null);
        (r.ok, r.message, info)
    }

    async fn test(&self, env: &Env) -> (bool, String) {
        let r = handle(env, &json!({"action": "test_connection"})).await;
        (r.ok, r.message)
    }

    fn history(&self) -> History {
        History::load(&self.paths)
    }

    fn real_clip(&self) -> PathBuf {
        self.clip.canonicalize().unwrap()
    }
}

fn expected_body(fields: &[(&str, &str)], file_field: &str, name: &str, ct: &str) -> Vec<u8> {
    let mut b = Vec::new();
    for (k, v) in fields {
        b.extend_from_slice(
            format!(
                "--FTHRClipBoundary8f61d4\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n"
            )
            .as_bytes(),
        );
    }
    b.extend_from_slice(
        format!(
            "--FTHRClipBoundary8f61d4\r\nContent-Disposition: form-data; name=\"{file_field}\"; filename=\"{name}\"\r\nContent-Type: {ct}\r\n\r\n"
        )
        .as_bytes(),
    );
    b.extend_from_slice(b"VIDEO-BYTES");
    b.extend_from_slice(b"\r\n--FTHRClipBoundary8f61d4--\r\n");
    b
}

#[tokio::test]
async fn catbox_multipart_fields() {
    let srv = server(vec![(200, "https://files.catbox.moe/abc123.mp4\n")]);
    let h = home(json!({"upload_provider": "catbox", "catbox_userhash": " hash1 "}));
    let (ok, msg, info) = h.upload(&h.env(&srv.url)).await;
    assert!(ok, "{msg}");
    assert_eq!(msg, "https://files.catbox.moe/abc123.mp4");
    let seen = srv.seen();
    assert_eq!(seen.len(), 1);
    let r = &seen[0];
    assert_eq!(
        (r.method.as_str(), r.target.as_str()),
        ("POST", "/upload?k=1")
    );
    assert_eq!(
        r.header("content-type"),
        Some("multipart/form-data; boundary=FTHRClipBoundary8f61d4")
    );
    assert_eq!(r.header("user-agent"), Some("FTHR-Clips/Desktop-Uploader"));
    assert_eq!(r.header("authorization"), None);
    assert_eq!(
        r.body,
        expected_body(
            &[("reqtype", "fileupload"), ("userhash", "hash1")],
            "fileToUpload",
            "my _clip_.mp4",
            "video/mp4"
        )
    );
    let entry = h.history().get(&h.real_clip()).unwrap();
    assert_eq!(entry.0["status"], json!("ok"));
    assert_eq!(entry.0["provider"], json!("catbox"));
    assert_eq!(entry.0["url"], json!("https://files.catbox.moe/abc123.mp4"));
    assert_eq!(
        entry.0["raw_url"],
        json!("https://files.catbox.moe/abc123.mp4")
    );
    assert_eq!(entry.0["favorite"], json!(false));
    assert_eq!(entry.0["uploaded_at"].as_str().unwrap().len(), 25);
    assert_eq!(info["url"], entry.0["url"]);

    // No userhash: no field.
    let srv = server(vec![(200, "https://files.catbox.moe/x.mp4")]);
    let h = home(json!({"upload_provider": "catbox"}));
    assert!(h.upload(&h.env(&srv.url)).await.0);
    assert_eq!(
        srv.seen()[0].body,
        expected_body(
            &[("reqtype", "fileupload")],
            "fileToUpload",
            "my _clip_.mp4",
            "video/mp4"
        )
    );
}

#[tokio::test]
async fn catbox_non_url_body_is_error() {
    let srv = server(vec![(200, "File too large")]);
    let h = home(json!({"upload_provider": "catbox"}));
    let (ok, msg, info) = h.upload(&h.env(&srv.url)).await;
    assert_eq!((ok, msg.as_str()), (false, "File too large"));
    assert_eq!(info, json!({}));
    assert_eq!(srv.seen().len(), 1, "a body without a URL is not retried");
    assert!(h.history().get(&h.real_clip()).is_none());
    let srv = server(vec![(400, "")]);
    let (_, msg, _) = h.upload(&h.env(&srv.url)).await;
    assert_eq!(msg, "Catbox returned HTTP 400.");
}

#[tokio::test]
async fn catbox_size_limit_message() {
    let srv = server(vec![]);
    let h = home(json!({"upload_provider": "catbox"}));
    let f = std::fs::File::create(&h.clip).unwrap();
    f.set_len(201 * 1024 * 1024).unwrap();
    let (ok, msg, _) = h.upload(&h.env(&srv.url)).await;
    assert!(!ok);
    assert_eq!(
        msg,
        "Catbox accepts files up to 200 MB; this file is 201.0 MB. Use Automatically compress in FTHR Clips."
    );
    assert!(srv.seen().is_empty());
}

#[tokio::test]
async fn custom_field_header_and_json_url() {
    let srv = server(vec![(201, r#"{"url": "https://cdn.example/c.mp4"}"#)]);
    let h = home(json!({
        "upload_provider": "custom",
        "upload_server_url": srv.url,
        "upload_auth_header": " Bearer test-token ",
    }));
    let (ok, msg, _) = h.upload(&h.env("http://unused")).await;
    assert!(ok, "{msg}");
    assert_eq!(msg, "https://cdn.example/c.mp4");
    let r = &srv.seen()[0];
    assert_eq!(r.header("authorization"), Some("Bearer test-token"));
    assert_eq!(r.target, "/upload?k=1");
    assert_eq!(
        r.body,
        expected_body(&[], "clip", "my _clip_.mp4", "video/mp4")
    );
    let e = h.history().get(&h.real_clip()).unwrap();
    assert_eq!(e.0["provider"], json!("custom"));
    assert_eq!(
        e.0["response"],
        json!(r#"{"url": "https://cdn.example/c.mp4"}"#)
    );

    for (body, url, msg) in [
        (
            r#"{"raw_url": "https://r.example/x"}"#,
            "https://r.example/x",
            "https://r.example/x",
        ),
        (
            "http://bare.example/x",
            "http://bare.example/x",
            "http://bare.example/x",
        ),
        ("stored!", "", "Upload complete."),
        ("[1,2]", "", "Upload complete."),
    ] {
        let srv = server(vec![(200, body)]);
        let h = home(json!({"upload_provider": "custom", "upload_server_url": srv.url}));
        let (ok, m, _) = h.upload(&h.env("http://unused")).await;
        assert!(ok, "{body}");
        assert_eq!(m, msg, "{body}");
        let e = h.history().get(&h.real_clip()).unwrap();
        assert_eq!(e.0["url"], json!(url));
        assert_eq!(e.0["raw_url"], json!(url));
    }
}

#[tokio::test]
async fn custom_error_statuses() {
    let srv = server(vec![(403, "forbidden")]);
    let h = home(json!({"upload_provider": "custom", "upload_server_url": srv.url}));
    let (ok, msg, _) = h.upload(&h.env("x")).await;
    assert_eq!((ok, msg.as_str()), (false, "forbidden"));
    assert_eq!(srv.seen().len(), 1, "4xx is not retried");
    let srv = server(vec![(404, "")]);
    let h = home(json!({"upload_provider": "custom", "upload_server_url": srv.url}));
    assert_eq!(
        h.upload(&h.env("x")).await.1,
        "Your server returned HTTP 404."
    );
}

#[tokio::test]
async fn retry_transient_only() {
    let srv = server(vec![(503, "busy"), (200, "https://cdn.example/ok")]);
    let h = home(json!({"upload_provider": "custom", "upload_server_url": srv.url}));
    let (ok, msg, _) = h.upload(&h.env("x")).await;
    assert!(ok, "{msg}");
    assert_eq!(srv.seen().len(), 2);

    let srv = server(vec![
        (500, "a"),
        (502, "b"),
        (503, "c"),
        (504, "d"),
        (200, "https://late"),
    ]);
    let h = home(json!({"upload_provider": "custom", "upload_server_url": srv.url}));
    let (ok, msg, _) = h.upload(&h.env("x")).await;
    assert_eq!((ok, msg.as_str()), (false, "d"));
    assert_eq!(srv.seen().len(), 4, "four attempts at most");

    // Nobody listening: a connection error, retried, then reported.
    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", dead.local_addr().unwrap());
    drop(dead);
    let h = home(json!({"upload_provider": "custom", "upload_server_url": url}));
    let (ok, msg, _) = h.upload(&h.env("x")).await;
    assert!(!ok);
    assert!(msg.starts_with("Could not connect"), "{msg}");
}

#[tokio::test]
async fn retry_never_past_deadline() {
    let srv = server(vec![(503, "busy"), (200, "https://cdn.example/ok")]);
    let h = home(json!({"upload_provider": "custom", "upload_server_url": srv.url}));
    let mut env = h.env("x");
    env.retry_delays = vec![Duration::from_secs(5); 3];
    env.deadline = Duration::from_secs(2);
    let started = std::time::Instant::now();
    let (ok, msg, _) = h.upload(&env).await;
    assert_eq!((ok, msg.as_str()), (false, "busy"));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(srv.seen().len(), 1);
}

#[tokio::test]
async fn auto_delete_after_success() {
    let srv = server(vec![(200, "https://cdn.example/a")]);
    let h = home(json!({
        "upload_provider": "custom", "upload_server_url": srv.url, "upload_auto_delete": true
    }));
    let real = h.real_clip();
    let (ok, msg, _) = h.upload(&h.env("x")).await;
    assert_eq!((ok, msg.as_str()), (true, "https://cdn.example/a"));
    assert!(!real.exists());
    assert!(h.history().is_uploaded(&real));
}

#[tokio::test]
async fn auto_delete_failure_still_succeeds() {
    use std::os::unix::fs::PermissionsExt;
    let srv = server(vec![(200, "https://cdn.example/a")]);
    let h = home(json!({
        "upload_provider": "custom", "upload_server_url": srv.url, "upload_auto_delete": true
    }));
    let folder = h.dir.path().join("clips");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (ok, msg, _) = h.upload(&h.env("x")).await;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    if nix_is_root() {
        return;
    }
    assert!(ok);
    assert!(
        msg.starts_with("Uploaded, but local delete failed: "),
        "{msg}"
    );
}

fn nix_is_root() -> bool {
    std::fs::metadata("/proc/self")
        .map(|m| std::os::unix::fs::MetadataExt::uid(&m) == 0)
        .unwrap_or(false)
}

#[tokio::test]
async fn test_connection_statuses() {
    let custom = |status: u16| async move {
        let srv = server(vec![(status, "")]);
        let h = home(json!({
            "upload_provider": "custom", "upload_server_url": srv.url,
            "upload_auth_header": "Token abc"
        }));
        let r = h.test(&h.env("x")).await;
        let seen = srv.seen();
        assert_eq!(seen[0].method, "HEAD");
        assert_eq!(seen[0].header("authorization"), Some("Token abc"));
        r
    };
    assert_eq!(
        custom(200).await,
        (true, "Your server reachable (HTTP 200).".into())
    );
    assert_eq!(
        custom(405).await,
        (true, "Your server reachable (HTTP 405).".into())
    );
    assert_eq!(
        custom(401).await,
        (
            false,
            "Your server rejected the authorization header (HTTP 401).".into()
        )
    );
    assert_eq!(
        custom(404).await,
        (false, "Your server returned HTTP 404.".into())
    );
    assert_eq!(
        custom(500).await,
        (false, "Your server returned HTTP 500.".into())
    );

    let catbox = |status: u16| async move {
        let srv = server(vec![(status, "")]);
        let h = home(json!({"upload_provider": "catbox"}));
        h.test(&h.env(&srv.url)).await
    };
    assert_eq!(
        catbox(412).await,
        (true, "Catbox reachable (HTTP 412).".into())
    );
    assert_eq!(
        catbox(502).await,
        (false, "Catbox returned HTTP 502.".into())
    );

    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", dead.local_addr().unwrap());
    drop(dead);
    let h = home(json!({"upload_provider": "custom", "upload_server_url": url}));
    let (ok, msg) = h.test(&h.env("x")).await;
    assert!(!ok);
    assert!(msg.starts_with("Could not reach your server: "), "{msg}");
}

#[allow(dead_code)]
fn unused(_: &Path) {}

/// Real TLS against Catbox (network; run by hand).
#[tokio::test]
#[ignore]
async fn live_catbox_head() {
    let h = home(json!({"upload_provider": "catbox"}));
    let (ok, msg) = h.test(&h.env(fthrust_uploader::CATBOX_URL)).await;
    assert!(ok, "{msg}");
}

#[tokio::test]
async fn no_link_means_no_auto_delete() {
    let srv = server(vec![(200, "<html>captive portal</html>")]);
    let h = home(json!({
        "upload_provider": "custom", "upload_server_url": srv.url, "upload_auto_delete": true
    }));
    let real = h.real_clip();
    let (ok, msg, _) = h.upload(&h.env("x")).await;
    assert_eq!((ok, msg.as_str()), (true, "Upload complete."));
    assert!(real.exists(), "the only copy was deleted without a link");
}

#[tokio::test]
async fn url_credentials_stay_out_of_messages() {
    // Headers promise a body the server never sends.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else { break };
            let mut buf = [0u8; 65536];
            let _ = s.read(&mut buf);
            std::thread::sleep(Duration::from_millis(100));
            let _ = s.write_all(b"HTTP/1.1 500 X\r\nContent-Length: 100\r\n\r\nshort");
        }
    });
    let h = home(json!({
        "upload_provider": "custom",
        "upload_server_url": format!("http://alice:hunter2@127.0.0.1:{port}/up"),
    }));
    let (ok, msg, _) = h.upload(&h.env("x")).await;
    assert!(!ok);
    assert!(!msg.contains("hunter2"), "{msg}");
}
