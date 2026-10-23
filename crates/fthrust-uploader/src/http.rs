//! Catbox and your own server (`upload_runtime.py`), plus the upload
//! wrapper every provider goes through (size limit, retries, history,
//! auto-delete).

use std::path::Path;
use std::time::{Duration, Instant};

use bytes::Bytes;
use fthrust_core::upload::CATBOX_LIMIT_BYTES;
use fthrust_core::upload::history::{Entry, History};
use fthrust_core::upload::settings::Provider;
use fthrust_core::upload::validate;
use futures_util::StreamExt;
use serde_json::{Map, Value, json};

use crate::{Env, Response, sftp};

const BOUNDARY: &str = "FTHRClipBoundary8f61d4";
const USER_AGENT: &str = "FTHR-Clips/Desktop-Uploader";
/// Socket timeout of an upload request (`timeout=120`).
const IO_TIMEOUT: Duration = Duration::from_secs(120);
/// Timeout of a connection test (`timeout=10`).
const TEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The most of a reply body that is read.
const BODY_CAP: usize = 1024 * 1024;
/// The most of a reply body shown as an error message.
const MESSAGE_CAP: usize = 400;

/// Why an attempt failed, and whether trying again could help.
#[derive(Debug)]
pub enum Failure {
    /// Network trouble, 429 or 5xx: worth another attempt.
    Transient(String),
    /// Anything else.
    Permanent(String),
}

impl Failure {
    fn message(self) -> String {
        match self {
            Failure::Transient(m) | Failure::Permanent(m) => m,
        }
    }
}

fn client(timeout: Duration, follow_redirects: bool) -> Result<reqwest::Client, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let policy = if follow_redirects {
        reqwest::redirect::Policy::default()
    } else {
        reqwest::redirect::Policy::none()
    };
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(timeout)
        .read_timeout(timeout)
        .redirect(policy)
        .build()
        .map_err(|e| format!("The uploader could not start its HTTP client: {e}"))
}

/// A readable account of a request error.
pub fn describe(e: &reqwest::Error) -> String {
    let text = describe_raw(e);
    // A server URL may carry `user:password@`; never show it.
    match e.url() {
        Some(u) if !u.username().is_empty() || u.password().is_some() => {
            let mut clean = u.clone();
            let _ = clean.set_username("");
            let _ = clean.set_password(None);
            text.replace(u.as_str(), clean.as_str())
        }
        _ => text,
    }
}

fn describe_raw(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return "The connection timed out.".into();
    }
    let mut root: &dyn std::error::Error = e;
    while let Some(next) = root.source() {
        root = next;
    }
    if e.is_connect() {
        format!("Could not connect: {root}")
    } else {
        let top = e.to_string();
        let cause = root.to_string();
        if top == cause {
            top
        } else {
            format!("{top}: {cause}")
        }
    }
}

fn capped(text: &str) -> String {
    if text.chars().count() <= MESSAGE_CAP {
        text.to_owned()
    } else {
        let mut s: String = text.chars().take(MESSAGE_CAP).collect();
        s.push('…');
        s
    }
}

fn transient_status(status: u16) -> bool {
    status == 429 || status >= 500
}

fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "mp4" => "video/mp4",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

/// `_multipart_post`: one streamed `multipart/form-data` POST with the
/// reference's boundary and explicit length. Returns the status and the
/// trimmed body text.
async fn multipart_post(
    url: &str,
    fields: &[(&str, &str)],
    file_field: &str,
    path: &Path,
    authorization: Option<&str>,
) -> Result<(u16, String), Failure> {
    let mut prefix = String::new();
    for (name, value) in fields {
        prefix.push_str(&format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().replace(['"', '\\'], "_"))
        .unwrap_or_default();
    prefix.push_str(&format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{file_field}\"; filename=\"{name}\"\r\nContent-Type: {}\r\n\r\n",
        content_type(path)
    ));
    let footer = format!("\r\n--{BOUNDARY}--\r\n");
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| Failure::Permanent(format!("The clip could not be read: {e}")))?;
    let size = file
        .metadata()
        .await
        .map_err(|e| Failure::Permanent(format!("File size could not be checked: {e}")))?
        .len();
    let length = prefix.len() as u64 + size + footer.len() as u64;
    let body =
        futures_util::stream::once(async move { Ok::<Bytes, std::io::Error>(Bytes::from(prefix)) })
            .chain(tokio_util::io::ReaderStream::with_capacity(
                file,
                1024 * 1024,
            ))
            .chain(futures_util::stream::once(async move {
                Ok::<Bytes, std::io::Error>(Bytes::from(footer))
            }));
    let client = client(IO_TIMEOUT, false).map_err(Failure::Permanent)?;
    let mut request = client
        .post(url)
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .header(reqwest::header::CONTENT_LENGTH, length)
        .body(reqwest::Body::wrap_stream(body));
    if let Some(auth) = authorization {
        request = request.header(reqwest::header::AUTHORIZATION, auth);
    }
    let mut response = request
        .send()
        .await
        .map_err(|e| Failure::Transient(describe(&e)))?;
    let status = response.status().as_u16();
    let mut raw = Vec::new();
    while raw.len() < BODY_CAP {
        match response.chunk().await {
            Ok(Some(chunk)) => raw.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(e) => return Err(Failure::Transient(describe(&e))),
        }
    }
    raw.truncate(BODY_CAP);
    Ok((status, String::from_utf8_lossy(&raw).trim().to_owned()))
}

fn setting<'a>(settings: &'a Map<String, Value>, key: &str) -> &'a str {
    settings.get(key).and_then(Value::as_str).unwrap_or("")
}

async fn upload_catbox(
    env: &Env,
    settings: &Map<String, Value>,
    path: &Path,
) -> Result<Map<String, Value>, Failure> {
    let userhash = setting(settings, "catbox_userhash").trim().to_owned();
    let mut fields = vec![("reqtype", "fileupload")];
    if !userhash.is_empty() {
        fields.push(("userhash", userhash.as_str()));
    }
    let (status, text) =
        multipart_post(&env.catbox_url, &fields, "fileToUpload", path, None).await?;
    if !(200..300).contains(&status) || !text.starts_with("https://") {
        let message = if text.is_empty() {
            format!("Catbox returned HTTP {status}.")
        } else {
            capped(&text)
        };
        return Err(if transient_status(status) {
            Failure::Transient(message)
        } else {
            Failure::Permanent(message)
        });
    }
    let mut m = Map::new();
    m.insert("url".into(), json!(text));
    m.insert("raw_url".into(), json!(text));
    m.insert("favorite".into(), json!(false));
    Ok(m)
}

async fn upload_custom(
    settings: &Map<String, Value>,
    path: &Path,
) -> Result<Map<String, Value>, Failure> {
    let url =
        validate::custom_url(setting(settings, "upload_server_url")).map_err(Failure::Permanent)?;
    let auth = validate::auth_header(setting(settings, "upload_auth_header"))
        .map_err(Failure::Permanent)?;
    let (status, text) = multipart_post(&url, &[], "clip", path, auth.as_deref()).await?;
    if !(200..300).contains(&status) {
        let message = if text.is_empty() {
            format!("Your server returned HTTP {status}.")
        } else {
            capped(&text)
        };
        return Err(if transient_status(status) {
            Failure::Transient(message)
        } else {
            Failure::Permanent(message)
        });
    }
    let url = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(data)) => {
            let pick = |k: &str| match data.get(k) {
                Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
                _ => None,
            };
            pick("url")
                .or_else(|| pick("raw_url"))
                .unwrap_or_default()
                .trim()
                .to_owned()
        }
        Ok(_) => String::new(),
        Err(_) if text.starts_with("http://") || text.starts_with("https://") => text.clone(),
        Err(_) => String::new(),
    };
    let mut m = Map::new();
    m.insert("url".into(), json!(url));
    m.insert("raw_url".into(), json!(url));
    m.insert(
        "response".into(),
        json!(text.chars().take(4096).collect::<String>()),
    );
    m.insert("favorite".into(), json!(false));
    Ok(m)
}

async fn attempt(
    env: &Env,
    provider: Provider,
    settings: &Map<String, Value>,
    path: &Path,
) -> Result<Map<String, Value>, Failure> {
    match provider {
        Provider::Catbox => upload_catbox(env, settings, path).await,
        Provider::Custom => upload_custom(settings, path).await,
        Provider::Sftp => sftp::upload(env, settings, path).await,
    }
}

/// `UploadRuntime.upload`: size limit, attempts, history, auto-delete.
pub async fn upload(
    env: &Env,
    provider: Provider,
    settings: &Map<String, Value>,
    path: &Path,
) -> Response {
    let failed = |message: String| Response {
        ok: false,
        message,
        upload_info: Some(Map::new()),
    };
    if !path.is_file() {
        return failed("File no longer exists.".into());
    }
    let actual = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => return failed(format!("File size could not be checked: {e}")),
    };
    if provider == Provider::Catbox && actual > CATBOX_LIMIT_BYTES {
        return failed(format!(
            "Catbox accepts files up to {} MB; this file is {:.1} MB. Use Automatically compress in FTHR Clips.",
            CATBOX_LIMIT_BYTES / (1024 * 1024),
            actual as f64 / (1024.0 * 1024.0)
        ));
    }
    // SFTP never repeats authentication or an ambiguous commit.
    let delays: &[Duration] = if provider == Provider::Sftp {
        &[]
    } else {
        &env.retry_delays
    };
    let started = Instant::now();
    let mut tries = 0;
    let result = loop {
        match attempt(env, provider, settings, path).await {
            Ok(r) => break Ok(r),
            Err(Failure::Transient(m)) if tries < delays.len() => {
                let wait = delays[tries];
                if started.elapsed() + wait > env.deadline {
                    break Err(m);
                }
                eprintln!(
                    "[Uploader] Attempt {} failed; retrying in {}s: {m}",
                    tries + 1,
                    wait.as_secs()
                );
                tokio::time::sleep(wait).await;
                tries += 1;
            }
            Err(f) => break Err(f.message()),
        }
    };
    let result = match result {
        Ok(r) => r,
        Err(m) => {
            return failed(if m.is_empty() {
                "Upload failed.".into()
            } else {
                m
            });
        }
    };
    let mut entry = Map::new();
    entry.insert("status".into(), json!("ok"));
    entry.insert("provider".into(), json!(provider.id()));
    entry.insert(
        "uploaded_at".into(),
        json!(fthrust_core::upload::utc_now_iso()),
    );
    entry.extend(result);
    if let Err(e) = History::update(&env.paths, |h| h.record(path, Entry(entry.clone()))) {
        eprintln!("[Uploader] Upload history could not be saved: {e}");
    }
    let url = entry.get("url").and_then(Value::as_str).unwrap_or("");
    let message = if url.is_empty() {
        "Upload complete.".to_owned()
    } else {
        url.to_owned()
    };
    // Without a link the upload is not confirmed: keep the only copy.
    if fthrust_core::upload::truthy(settings.get("upload_auto_delete"))
        && !url.is_empty()
        && let Err(e) = std::fs::remove_file(path)
    {
        return Response {
            ok: true,
            message: format!("Uploaded, but local delete failed: {e}"),
            upload_info: Some(entry),
        };
    }
    Response {
        ok: true,
        message,
        upload_info: Some(entry),
    }
}

/// Test connection to Catbox: any answer below 500 means reachable.
pub async fn test_catbox(env: &Env) -> (bool, String) {
    let client = match client(TEST_TIMEOUT, true) {
        Ok(c) => c,
        Err(e) => return (false, e),
    };
    match client.head(&env.catbox_url).send().await {
        Ok(r) if r.status().as_u16() < 500 => (
            true,
            format!("Catbox reachable (HTTP {}).", r.status().as_u16()),
        ),
        Ok(r) => (
            false,
            format!("Catbox returned HTTP {}.", r.status().as_u16()),
        ),
        Err(e) => (false, format!("Could not reach Catbox: {}", describe(&e))),
    }
}

/// Test connection to your own server: success, a redirect or 405 means
/// reachable; 401/403 means the header was refused; other errors fail.
pub async fn test_custom(settings: &Map<String, Value>) -> Result<(bool, String), String> {
    let url = validate::custom_url(setting(settings, "upload_server_url"))?;
    let auth = validate::auth_header(setting(settings, "upload_auth_header"))?;
    let client = client(TEST_TIMEOUT, true)?;
    let mut request = client.head(&url);
    if let Some(a) = auth {
        request = request.header(reqwest::header::AUTHORIZATION, a);
    }
    Ok(match request.send().await {
        Ok(r) => {
            let code = r.status().as_u16();
            match code {
                200..=399 | 405 => (true, format!("Your server reachable (HTTP {code}).")),
                401 | 403 => (
                    false,
                    format!("Your server rejected the authorization header (HTTP {code})."),
                ),
                _ => (false, format!("Your server returned HTTP {code}.")),
            }
        }
        Err(e) => (
            false,
            format!("Could not reach your server: {}", describe(&e)),
        ),
    })
}
