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
use fthrust_uploader::{Env, Response, handle};
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
    server_with(replies.into_iter().map(|(s, b)| (s, b, "")).collect())
}

/// Like `server`, with extra raw header lines (`"Location: …\r\n"`) per reply.
fn server_with(replies: Vec<(u16, &str, &str)>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/upload?k=1", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let mut replies: VecDeque<(u16, String, String)> = replies
        .into_iter()
        .map(|(s, b, h)| (s, b.to_owned(), h.to_owned()))
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
            let (status, text, extra) =
                replies
                    .pop_front()
                    .unwrap_or((500, "no reply".into(), String::new()));
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{text}",
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

    /// The whole reply to an upload request for the clip.
    async fn respond(&self, env: &Env) -> Response {
        let clips = self.dir.path().join("clips");
        let req = json!({
            "action": "upload",
            "path": self.clip,
            "allowed_roots": [clips],
            "clips_root": clips,
        });
        handle(env, &req).await
    }

    async fn upload(&self, env: &Env) -> (bool, String, Value) {
        let r = self.respond(env).await;
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
        ("stored!", "", NO_LINK),
        ("[1,2]", "", NO_LINK),
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

const NO_LINK: &str = "Uploaded, but your server didn't reply with a link.";
const AT_UPLOAD: &str = "Check the server URL; FTHR Clips Server takes uploads at /upload.";

/// One upload to a custom server answering `replies`, with `auth` as the
/// Authorization setting. Returns the message and how many requests came.
async fn custom_upload(replies: Vec<(u16, &str)>, auth: &str) -> (bool, String, usize) {
    let srv = server(replies);
    let h = home(json!({
        "upload_provider": "custom", "upload_server_url": srv.url,
        "upload_auth_header": auth
    }));
    let (ok, msg, _) = h.upload(&h.env("x")).await;
    (ok, msg, srv.seen().len())
}

#[tokio::test]
async fn custom_error_statuses() {
    let token_hint = "Set the Auth header to \"Bearer <your upload token>\".";
    for (status, body, auth, want) in [
        (
            401,
            "authentication failed",
            "Bearer t",
            "Your server rejected the authorization header (HTTP 401).".to_owned(),
        ),
        (
            403,
            "forbidden",
            "Bearer t",
            "Your server rejected the authorization header (HTTP 403).".to_owned(),
        ),
        (
            401,
            "",
            "",
            format!("Your server needs an upload token (HTTP 401). {token_hint}"),
        ),
        (
            401,
            "",
            "abc123",
            "Your server rejected the authorization header (HTTP 401). \
          The Auth header usually looks like \"Bearer <your upload token>\"."
                .to_owned(),
        ),
        (
            404,
            "",
            "",
            format!("Your server has nothing at this URL (HTTP 404). {AT_UPLOAD}"),
        ),
        (
            405,
            "",
            "",
            format!("This URL doesn't take uploads (HTTP 405). {AT_UPLOAD}"),
        ),
        (
            413,
            "upload too large",
            "",
            "The clip is too large for your server (HTTP 413). This clip is 11 bytes.".to_owned(),
        ),
        (
            415,
            "unsupported media type",
            "",
            "Your server doesn't accept this file type (HTTP 415).".to_owned(),
        ),
        (
            400,
            "malformed multipart body",
            "",
            "Your server refused the upload (HTTP 400): malformed multipart body".to_owned(),
        ),
        (
            400,
            r#"{"error": "bad field"}"#,
            "",
            "Your server refused the upload (HTTP 400): bad field".to_owned(),
        ),
        (
            422,
            r#"{"message": "nope"}"#,
            "",
            "Your server refused the upload (HTTP 422): nope".to_owned(),
        ),
        (
            400,
            "<html><body>nginx</body></html>",
            "",
            "Your server refused the upload (HTTP 400).".to_owned(),
        ),
        (
            418,
            "",
            "",
            "Your server refused the upload (HTTP 418).".to_owned(),
        ),
    ] {
        let (ok, msg, n) = custom_upload(vec![(status, body)], auth).await;
        assert_eq!(
            (ok, msg.as_str()),
            (false, want.as_str()),
            "{status} {body}"
        );
        assert_eq!(n, 1, "{status} is not retried");
    }
}

#[tokio::test]
async fn custom_transient_statuses_after_retries() {
    for (status, body, want) in [
        (
            429,
            "rate limited",
            "Your server is limiting uploads right now (HTTP 429). Try again in a minute.",
        ),
        (
            500,
            r#"{"error": "internal error"}"#,
            "Your server hit an error (HTTP 500): internal error",
        ),
        (500, "", "Your server hit an error (HTTP 500)."),
        (
            502,
            "<html>Bad Gateway</html>",
            "Your server is unavailable right now (HTTP 502).",
        ),
        (
            503,
            "busy",
            "Your server is unavailable right now (HTTP 503): busy",
        ),
    ] {
        let replies = vec![(status, body); 4];
        let (ok, msg, n) = custom_upload(replies, "").await;
        assert_eq!((ok, msg.as_str()), (false, want), "{status}");
        assert_eq!(n, 4, "{status} is retried");
    }
}

#[tokio::test]
async fn custom_redirects_name_the_new_address() {
    let srv = server_with(vec![(
        301,
        "",
        "Location: https://clips.example.com/upload\r\n",
    )]);
    let h = home(json!({"upload_provider": "custom", "upload_server_url": srv.url}));
    let (ok, msg, _) = h.upload(&h.env("x")).await;
    assert_eq!(
        (ok, msg.as_str()),
        (
            false,
            "Your server redirected the upload to https://clips.example.com/upload. \
                 Use that address as the server URL."
        )
    );
    assert_eq!(srv.seen().len(), 1);

    let srv = server_with(vec![(308, "", "Location: /v2/upload\r\n")]);
    let base = srv.url.split("/upload").next().unwrap().to_owned();
    let h = home(json!({"upload_provider": "custom", "upload_server_url": srv.url}));
    assert_eq!(
        h.upload(&h.env("x")).await.1,
        format!(
            "Your server redirected the upload to {base}/v2/upload. Use that address as the server URL."
        )
    );

    let srv = server_with(vec![(302, "", "")]);
    let h = home(json!({"upload_provider": "custom", "upload_server_url": srv.url}));
    assert_eq!(
        h.upload(&h.env("x")).await.1,
        "Your server redirected the upload (HTTP 302). Check the server URL."
    );

    // The test does not follow redirects either: it would pass while uploads fail.
    let srv = server_with(vec![
        (301, "", "Location: https://clips.example.com/upload\r\n"),
        (200, "", ""),
    ]);
    let h = home(json!({"upload_provider": "custom", "upload_server_url": srv.url}));
    assert_eq!(
        h.test(&h.env("x")).await,
        (
            false,
            "Your server redirects to https://clips.example.com/upload. \
                 Use that address as the server URL."
                .into()
        )
    );
    assert_eq!(srv.seen().len(), 1);
}

#[tokio::test]
async fn custom_test_warns_about_a_bare_address() {
    let srv = server(vec![(200, ""), (200, "")]);
    let root = srv.url.split("/upload").next().unwrap().to_owned();
    for url in [root.clone(), format!("{root}/")] {
        let h = home(json!({"upload_provider": "custom", "upload_server_url": url}));
        assert_eq!(
            h.test(&h.env("x")).await,
            (
                false,
                "Your server answered (HTTP 200), but the URL has no path. \
                    FTHR Clips Server takes uploads at /upload."
                    .into()
            )
        );
    }
}

#[tokio::test]
async fn network_errors_say_what_went_wrong() {
    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/upload", dead.local_addr().unwrap());
    drop(dead);
    let refused = "the connection was refused. Check that it is running and the port is right.";
    let h = home(json!({"upload_provider": "custom", "upload_server_url": url}));
    assert_eq!(
        h.test(&h.env("x")).await.1,
        format!("Could not reach your server: {refused}")
    );
    assert_eq!(
        h.upload(&h.env("x")).await.1,
        format!("Could not reach your server: {refused}")
    );

    let h = home(json!({"upload_provider": "custom",
                        "upload_server_url": "http://fthr-clips.invalid/upload"}));
    assert_eq!(
        h.test(&h.env("x")).await.1,
        "Could not reach your server: the name fthr-clips.invalid could not be found."
    );

    // https:// to a server that speaks plain HTTP: it answers the TLS hello
    // with an HTTP error, or just hangs up.
    for reply in [
        &b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n"[..],
        b"",
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let tls = format!("https://{}/upload", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut hello = [0u8; 512];
                let _ = stream.read(&mut hello);
                let _ = stream.write_all(reply);
            }
        });
        let h = home(json!({"upload_provider": "custom", "upload_server_url": tls}));
        assert_eq!(
            h.test(&h.env("x")).await.1,
            "Could not reach your server: it did not answer with HTTPS. \
             Check whether the URL should start with http://.",
            "{reply:?}"
        );
    }
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
    assert_eq!(
        (ok, msg.as_str()),
        (false, "Your server is unavailable right now (HTTP 504): d")
    );
    assert_eq!(srv.seen().len(), 4, "four attempts at most");

    // Nobody listening: a connection error, retried, then reported.
    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", dead.local_addr().unwrap());
    drop(dead);
    let h = home(json!({"upload_provider": "custom", "upload_server_url": url}));
    let (ok, msg, _) = h.upload(&h.env("x")).await;
    assert!(!ok);
    assert!(msg.starts_with("Could not reach your server: "), "{msg}");
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
    assert_eq!(
        (ok, msg.as_str()),
        (
            false,
            "Your server is unavailable right now (HTTP 503): busy"
        )
    );
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

/// Whatever an upload that worked leaves the user to know travels as the
/// reply's `warning` (the app shows it); a clean success has none.
#[tokio::test]
async fn an_upload_that_worked_with_a_caveat_says_so() {
    // No link: there is nothing for the card to copy.
    for auto_delete in [false, true] {
        let srv = server(vec![(200, "stored!")]);
        let h = home(json!({
            "upload_provider": "custom", "upload_server_url": srv.url,
            "upload_auto_delete": auto_delete
        }));
        let r = h.respond(&h.env("x")).await;
        assert!(r.ok);
        assert_eq!(r.message, NO_LINK);
        let kept = if auto_delete {
            " The clip was kept."
        } else {
            ""
        };
        assert_eq!(
            r.warning.as_deref(),
            Some(format!("{NO_LINK}{kept}").as_str())
        );
        assert!(
            r.to_line()
                .contains(r#""warning":"Uploaded, but your server"#)
        );
    }
    // A clean success has no warning, and no key in the line.
    let srv = server(vec![(200, "https://cdn.example/fine")]);
    let h = home(json!({"upload_provider": "custom", "upload_server_url": srv.url}));
    let r = h.respond(&h.env("x")).await;
    assert_eq!(r.warning, None);
    assert!(!r.to_line().contains("warning"), "{}", r.to_line());
    // The clip could not be deleted.
    if nix_is_root() {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let srv = server(vec![(200, "https://cdn.example/a")]);
    let h = home(json!({
        "upload_provider": "custom", "upload_server_url": srv.url, "upload_auto_delete": true
    }));
    let folder = h.dir.path().join("clips");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let r = h.respond(&h.env("x")).await;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(r.ok);
    assert!(
        r.message.starts_with("Uploaded, but local delete failed: "),
        "{}",
        r.message
    );
    assert_eq!(r.warning.as_deref(), Some(r.message.as_str()));
}

/// Auto-delete only ever removes files in the clips folder. Imported folders
/// (and links into them) upload like everything else but are never deleted:
/// interval mode plus auto-delete would otherwise empty a whole imported
/// library.
#[tokio::test]
async fn auto_delete_stays_inside_the_clips_folder() {
    let srv = server(vec![(200, "https://cdn.example/ok"); 5]);
    let h = home(json!({
        "upload_provider": "custom", "upload_server_url": srv.url, "upload_auto_delete": true
    }));
    let clips = h.dir.path().join("clips");
    let imports = h.dir.path().join("imports");
    std::fs::create_dir_all(&imports).unwrap();
    let make = |dir: &Path, name: &str| {
        let p = dir.join(name);
        std::fs::write(&p, b"VIDEO-BYTES").unwrap();
        p.canonicalize().unwrap()
    };
    let ours = make(&clips, "ours.mp4");
    let theirs = make(&imports, "theirs.mp4");
    let real = make(&imports, "real.mp4");
    let link = clips.join("link.mp4");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let env = h.env("x");
    let ask = |path: PathBuf, clips_root: Option<&PathBuf>| {
        let mut req = json!({
            "action": "upload", "path": path, "allowed_roots": [clips, imports],
        });
        if let Some(root) = clips_root {
            req["clips_root"] = json!(root);
        }
        let env = &env;
        async move { handle(env, &req).await }
    };
    for (path, deleted, what) in [
        (ours.clone(), true, "a clip in the clips folder"),
        (theirs.clone(), false, "a clip in an imported folder"),
        (
            link,
            false,
            "a link in the clips folder to an imported clip",
        ),
    ] {
        let r = ask(path.clone(), Some(&clips)).await;
        assert!(r.ok, "{what}: {}", r.message);
        assert_eq!(r.message, "https://cdn.example/ok", "{what}");
        assert!(r.warning.is_none(), "{what}: {:?}", r.warning);
        let target = path.canonicalize().unwrap_or(path.clone());
        assert_eq!(!target.exists(), deleted, "{what}");
        assert!(
            h.history().is_uploaded(&target) || deleted,
            "{what}: not recorded"
        );
    }
    assert!(real.exists() && theirs.exists());
    // Without a clips folder in the request nothing is ever deleted.
    let again = make(&clips, "again.mp4");
    let r = ask(again.clone(), None).await;
    assert!(r.ok, "{}", r.message);
    assert!(
        again.exists(),
        "deleted without a clips folder to be inside of"
    );
}

/// Auto-delete needs a link the card could offer: whatever else a server
/// puts in its reply, "the server said something" is not a confirmation.
#[tokio::test]
async fn only_a_real_link_allows_auto_delete() {
    for (body, what) in [
        (r#"{"url": "queued"}"#, "a word"),
        (r#"{"url": "file:///etc/hostname"}"#, "a file URL"),
        (r#"{"url": "ftp://files.example/a.mp4"}"#, "an ftp URL"),
        (
            r#"{"url": "sftp://files.example"}"#,
            "an sftp URL without a path",
        ),
        (r#"{"url": "https://cdn.example/a b"}"#, "a space"),
        (
            r#"{"url": "https://cdn.example/a\nhttps://cdn.example/b"}"#,
            "two lines in JSON",
        ),
        (
            "https://cdn.example/a\nand some words",
            "plain text, two lines",
        ),
    ] {
        let srv = server(vec![(200, body)]);
        let h = home(json!({
            "upload_provider": "custom", "upload_server_url": srv.url, "upload_auto_delete": true
        }));
        let real = h.real_clip();
        let r = h.respond(&h.env("x")).await;
        assert!(r.ok, "{what}: {}", r.message);
        assert!(real.exists(), "{what}: the clip was deleted");
        assert_eq!(r.message, NO_LINK, "{what}");
        assert!(h.history().is_uploaded(&real), "{what}: not recorded");
    }
    // Real links still delete (the SFTP ones are tested with the SFTP server).
    for url in ["https://cdn.example/a", "http://192.168.1.5:8080/f/a.mp4"] {
        let srv = server(vec![(200, url)]);
        let h = home(json!({
            "upload_provider": "custom", "upload_server_url": srv.url, "upload_auto_delete": true
        }));
        let real = h.real_clip();
        let r = h.respond(&h.env("x")).await;
        assert_eq!((r.ok, r.message.as_str()), (true, url));
        assert!(!real.exists(), "{url}: the clip was kept");
    }
}

/// The upload worked but nothing records it: deleting the clip would leave
/// a copy on the server, no local file and no link anywhere.
#[tokio::test]
async fn a_history_that_cannot_be_saved_keeps_the_clip_and_hands_over_the_link() {
    let srv = server(vec![(200, "https://cdn.example/a")]);
    let h = home(json!({
        "upload_provider": "custom", "upload_server_url": srv.url, "upload_auto_delete": true
    }));
    // A folder where the history file belongs: it cannot be written.
    std::fs::create_dir_all(h.paths.upload_history_file()).unwrap();
    let real = h.real_clip();
    let r = h.respond(&h.env("x")).await;
    assert!(real.exists(), "the clip was deleted: {r:?}");
    assert!(r.ok, "the upload itself worked: {}", r.message);
    assert_eq!(r.message, "https://cdn.example/a");
    let warning = r.warning.clone().expect("no warning");
    assert!(
        warning.starts_with(
            "Uploaded to https://cdn.example/a, but the upload history could not be saved ("
        ),
        "{warning}"
    );
    assert!(warning.ends_with("). The clip was kept."), "{warning}");
    assert_eq!(
        r.upload_info.as_ref().unwrap()["url"],
        json!("https://cdn.example/a")
    );
    assert!(
        r.to_line()
            .contains(r#""warning":"Uploaded to https://cdn.example/a"#)
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
        (
            false,
            format!("Your server has nothing at this URL (HTTP 404). {AT_UPLOAD}")
        )
    );
    assert_eq!(
        custom(500).await,
        (false, "Your server hit an error (HTTP 500).".into())
    );
    assert_eq!(
        custom(503).await,
        (
            false,
            "Your server is unavailable right now (HTTP 503).".into()
        )
    );
    assert_eq!(
        custom(429).await,
        (
            false,
            "Your server is limiting requests right now (HTTP 429). Try again in a minute.".into()
        )
    );
    assert_eq!(
        custom(418).await,
        (false, "Your server returned HTTP 418.".into())
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
    assert_eq!((ok, msg.as_str()), (true, NO_LINK));
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

#[tokio::test]
async fn settings_problems_are_flagged() {
    async fn flagged(status: u16, extra: &str, auth: &str) -> (bool, String) {
        let srv = server_with(vec![(status, "", extra)]);
        let h = home(json!({
            "upload_provider": "custom", "upload_server_url": srv.url,
            "upload_auth_header": auth
        }));
        let req = json!({
            "action": "upload", "path": h.clip,
            "allowed_roots": [h.dir.path().join("clips")],
        });
        let r = handle(&h.env("x"), &req).await;
        assert!(!r.ok);
        (r.settings_problem, r.to_line())
    }
    for (status, extra) in [
        (401, ""),
        (403, ""),
        (404, ""),
        (405, ""),
        (301, "Location: https://e.example/upload\r\n"),
    ] {
        let (flag, line) = flagged(status, extra, "Bearer t").await;
        assert!(flag, "{status}");
        assert!(line.contains(r#""settings_problem":true"#), "{line}");
    }
    for status in [400, 413, 415] {
        let (flag, line) = flagged(status, "", "Bearer t").await;
        assert!(!flag, "{status}");
        assert!(!line.contains("settings_problem"), "{line}");
    }
    // No server URL at all.
    let h = home(json!({"upload_provider": "custom", "upload_server_url": ""}));
    let req = json!({
        "action": "upload", "path": h.clip,
        "allowed_roots": [h.dir.path().join("clips")],
    });
    let r = handle(&h.env("x"), &req).await;
    assert!(!r.ok && r.settings_problem, "{}", r.message);
}

/// Uploads that are slow, or that go quiet, against servers on the tokio
/// runtime (the stall limit is shortened through `Env::io_timeout`).
mod stalls {
    use std::time::Instant;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// How the server treats the upload.
    #[derive(Clone, Copy)]
    enum Mode {
        /// Takes the body at a steady 8 MB/s, then answers.
        Slow,
        /// Takes a little of the body, then never reads again.
        StopsReading,
        /// Takes the whole body, then never answers.
        NeverAnswers,
        /// Answers with headers and a few bytes, then goes quiet.
        StallsAnswering,
    }

    /// A server on a socket with a small receive buffer, so the upload feels
    /// how fast the server reads instead of vanishing into the kernel's
    /// buffers: tiny when the server stops reading (the upload backs up
    /// after a few kilobytes), 64 KiB when it reads at a steady pace (a
    /// tiny window would make the transfer itself crawl). Returns the
    /// upload URL.
    fn serve(mode: Mode) -> String {
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        let buffer = if matches!(mode, Mode::Slow) {
            64 * 1024
        } else {
            4096
        };
        socket.set_recv_buffer_size(buffer).unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = socket.listen(8).unwrap();
        let url = format!("http://{}/upload", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let _ = respond(&mut s, mode).await;
                });
            }
        });
        url
    }

    async fn respond(s: &mut tokio::net::TcpStream, mode: Mode) -> std::io::Result<()> {
        let mut head = Vec::new();
        let mut chunk = [0u8; 4096];
        let end = loop {
            let n = s.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            head.extend_from_slice(&chunk[..n]);
            if let Some(i) = head.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let text = String::from_utf8_lossy(&head[..end]).to_lowercase();
        let length: usize = text
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse().ok())
            .unwrap();
        let mut got = head.len() - end;
        let mut buf = vec![0u8; 64 * 1024];
        let quiet = Duration::from_secs(60);
        while got < length {
            if matches!(mode, Mode::StopsReading) && got > 32 * 1024 {
                tokio::time::sleep(quiet).await;
            }
            let n = s.read(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            got += n;
            if matches!(mode, Mode::Slow) {
                tokio::time::sleep(Duration::from_secs_f64(n as f64 / 8_000_000.0)).await;
            }
        }
        match mode {
            Mode::Slow => {
                let body = "https://cdn.example/slow.mp4";
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(reply.as_bytes()).await?;
            }
            Mode::StallsAnswering => {
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nabc")
                    .await?;
                tokio::time::sleep(quiet).await;
            }
            Mode::NeverAnswers | Mode::StopsReading => tokio::time::sleep(quiet).await,
        }
        Ok(())
    }

    /// An upload of `size` bytes with the stall limit at `limit_ms` and no
    /// retries; a hang is a failure, not a stuck test run.
    async fn upload(
        provider: &str,
        mode: Mode,
        size: usize,
        limit_ms: u64,
    ) -> (bool, String, Duration) {
        let url = serve(mode);
        let h = if provider == "catbox" {
            home(json!({"upload_provider": "catbox"}))
        } else {
            home(json!({"upload_provider": "custom", "upload_server_url": url}))
        };
        std::fs::write(&h.clip, vec![5u8; size]).unwrap();
        let mut env = h.env(&url);
        env.io_timeout = Duration::from_millis(limit_ms);
        env.retry_delays = Vec::new();
        let started = Instant::now();
        let (ok, msg, _) = tokio::time::timeout(Duration::from_secs(20), h.upload(&env))
            .await
            .expect("the upload hung");
        (ok, msg, started.elapsed())
    }

    #[tokio::test]
    async fn a_slow_but_steady_upload_is_not_cut_off() {
        // 24 MiB at 8 MB/s takes over 3 s, twice the stall limit; the kernel's
        // buffers swallow the first few megabytes, so the pace only shows
        // in the rest, and the tail takes half a second to drain.
        let (ok, msg, took) = upload("custom", Mode::Slow, 24 << 20, 1500).await;
        assert!(ok, "{msg}");
        assert_eq!(msg, "https://cdn.example/slow.mp4");
        assert!(
            took > Duration::from_millis(2000),
            "not slow enough: {took:?}"
        );
    }

    #[tokio::test]
    async fn a_server_that_stops_reading_stalls_the_upload() {
        for provider in ["custom", "catbox"] {
            let who = if provider == "catbox" {
                "Catbox"
            } else {
                "your server"
            };
            // More than any socket buffer holds, so the upload backs up.
            let (ok, msg, took) = upload(provider, Mode::StopsReading, 48 << 20, 500).await;
            assert!(!ok);
            assert_eq!(
                msg,
                format!("The upload to {who} stalled: no data went out for 500 ms.")
            );
            assert!(took < Duration::from_secs(5), "{took:?}");
        }
    }

    #[tokio::test]
    async fn a_server_that_never_answers_is_given_up_on() {
        let (ok, msg, took) = upload("custom", Mode::NeverAnswers, 1000, 500).await;
        assert!(!ok);
        assert_eq!(
            msg,
            "Your server did not answer within 500 ms of the end of the upload."
        );
        assert!(took < Duration::from_secs(5), "{took:?}");
        let (_, msg, _) = upload("catbox", Mode::NeverAnswers, 1000, 500).await;
        assert_eq!(
            msg,
            "Catbox did not answer within 500 ms of the end of the upload."
        );
    }

    #[tokio::test]
    async fn an_answer_that_stalls_is_given_up_on() {
        let (ok, msg, took) = upload("custom", Mode::StallsAnswering, 1000, 500).await;
        assert!(!ok);
        assert_eq!(
            msg,
            "The answer from your server stalled: no data for 500 ms."
        );
        assert!(took < Duration::from_secs(5), "{took:?}");
    }
}
