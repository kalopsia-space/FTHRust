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
/// Where FTHR Clips Server takes uploads, for messages about a wrong URL.
const AT_UPLOAD: &str = "Check the server URL; FTHR Clips Server takes uploads at /upload.";
/// What the Auth header of FTHR Clips Server looks like.
const TOKEN_EXAMPLE: &str = "\"Bearer <your upload token>\"";

/// Why an attempt failed, and whether trying again could help.
#[derive(Debug)]
pub enum Failure {
    /// Network trouble, 429 or 5xx: worth another attempt.
    Transient(String),
    /// Anything else.
    Permanent(String),
    /// A permanent failure the upload settings can fix (URL, token).
    Settings(String),
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

/// A readable account of a request error: a lower-case clause for
/// "Could not reach …: <clause>", without the full stop.
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
    use std::io::ErrorKind;
    if e.is_timeout() {
        return "the connection timed out".into();
    }
    let mut root: &dyn std::error::Error = e;
    let mut next: Option<&dyn std::error::Error> = Some(e);
    while let Some(err) = next {
        let io = err.downcast_ref::<std::io::Error>();
        if let Some(tls) = err.downcast_ref::<rustls::Error>() {
            return tls_words(tls);
        }
        match io.map(std::io::Error::kind) {
            Some(ErrorKind::ConnectionRefused) => {
                return "the connection was refused. Check that it is running and the port \
                        is right"
                    .into();
            }
            Some(
                ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::BrokenPipe
                | ErrorKind::UnexpectedEof,
            ) => return "the connection was closed unexpectedly".into(),
            _ => {}
        }
        let text = err.to_string();
        if text.contains("tls handshake eof") {
            return tls_words(&rustls::Error::InvalidMessage(
                rustls::InvalidMessage::InvalidContentType,
            ));
        }
        if text.contains("dns error") || text.contains("failed to lookup address") {
            let host = e
                .url()
                .and_then(|u| u.host_str())
                .unwrap_or("of the server");
            return format!("the name {host} could not be found");
        }
        root = err;
        // An io::Error's source() skips the error it wraps; step into it.
        next = match io {
            Some(io) => io.get_ref().map(|inner| inner as &dyn std::error::Error),
            None => err.source(),
        };
    }
    if e.is_connect() {
        format!("could not connect ({root})")
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

fn tls_words(e: &rustls::Error) -> String {
    match e {
        rustls::Error::InvalidMessage(_) => "it did not answer with HTTPS. Check whether the URL \
                                              should start with http://"
            .into(),
        rustls::Error::InvalidCertificate(_) => {
            format!("its HTTPS certificate was not accepted ({e})")
        }
        _ => format!("the secure connection failed ({e})"),
    }
}

/// Ends a clause with a full stop unless it already has one.
fn sentence(clause: &str) -> String {
    if clause.ends_with(['.', '!', '?']) {
        clause.to_owned()
    } else {
        format!("{clause}.")
    }
}

/// A short explanation from an error reply: a JSON `error`, `message` or
/// `detail`, or plain text; never an HTML page.
fn server_detail(body: &str) -> Option<String> {
    let text = body.trim();
    if text.is_empty() || text.starts_with('<') {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        let Value::Object(m) = v else { return None };
        return ["error", "message", "detail"]
            .iter()
            .find_map(|k| match m.get(*k) {
                Some(Value::String(s)) if !s.trim().is_empty() => Some(capped(s.trim())),
                _ => None,
            });
    }
    Some(capped(text))
}

/// `base` plus ": detail", or a full stop when there is none.
fn with_detail(base: String, detail: Option<String>) -> String {
    match detail {
        Some(d) => format!("{base}: {d}"),
        None => format!("{base}."),
    }
}

fn auth_rejected(code: u16, auth: Option<&str>) -> String {
    match auth {
        None => format!(
            "Your server needs an upload token (HTTP {code}). Set the Auth header to {TOKEN_EXAMPLE}."
        ),
        Some(a) if !a.contains(char::is_whitespace) => format!(
            "Your server rejected the authorization header (HTTP {code}). The Auth header usually \
             looks like {TOKEN_EXAMPLE}."
        ),
        Some(_) => format!("Your server rejected the authorization header (HTTP {code})."),
    }
}

/// A redirect's target resolved against `url`, without user info.
fn redirect_target(url: &str, location: Option<&str>) -> Option<String> {
    let mut target = reqwest::Url::parse(url).ok()?.join(location?).ok()?;
    let _ = target.set_username("");
    let _ = target.set_password(None);
    Some(target.to_string())
}

fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} bytes")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

/// What a failed upload reply from your own server means.
fn custom_upload_failure(
    status: u16,
    body: &str,
    location: Option<String>,
    auth: Option<&str>,
    size: u64,
) -> Failure {
    let detail = || server_detail(body);
    let message = match status {
        300..=399 => match location {
            Some(to) => format!(
                "Your server redirected the upload to {to}. Use that address as the server URL."
            ),
            None => {
                format!("Your server redirected the upload (HTTP {status}). Check the server URL.")
            }
        },
        401 | 403 => auth_rejected(status, auth),
        404 => format!("Your server has nothing at this URL (HTTP 404). {AT_UPLOAD}"),
        405 => format!("This URL doesn't take uploads (HTTP 405). {AT_UPLOAD}"),
        413 => format!(
            "The clip is too large for your server (HTTP 413). This clip is {}.",
            human_size(size)
        ),
        415 => "Your server doesn't accept this file type (HTTP 415).".into(),
        429 => {
            "Your server is limiting uploads right now (HTTP 429). Try again in a minute.".into()
        }
        502..=504 => with_detail(
            format!("Your server is unavailable right now (HTTP {status})"),
            detail(),
        ),
        500.. => with_detail(
            format!("Your server hit an error (HTTP {status})"),
            detail(),
        ),
        _ => with_detail(
            format!("Your server refused the upload (HTTP {status})"),
            detail(),
        ),
    };
    if transient_status(status) {
        Failure::Transient(message)
    } else if matches!(status, 300..=399 | 401 | 403 | 404 | 405) {
        Failure::Settings(message)
    } else {
        Failure::Permanent(message)
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
/// reference's boundary and explicit length. Returns the status, the
/// trimmed body text and any `Location` header. `who` names the server in
/// network errors ("Catbox", "your server").
async fn multipart_post(
    who: &str,
    url: &str,
    fields: &[(&str, &str)],
    file_field: &str,
    path: &Path,
    authorization: Option<&str>,
) -> Result<(u16, String, Option<String>), Failure> {
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
    let mut response = request.send().await.map_err(|e| {
        Failure::Transient(format!(
            "Could not reach {who}: {}",
            sentence(&describe(&e))
        ))
    })?;
    let status = response.status().as_u16();
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut raw = Vec::new();
    while raw.len() < BODY_CAP {
        match response.chunk().await {
            Ok(Some(chunk)) => raw.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(e) => {
                return Err(Failure::Transient(format!(
                    "The connection to {who} dropped: {}",
                    sentence(&describe(&e))
                )));
            }
        }
    }
    raw.truncate(BODY_CAP);
    Ok((
        status,
        String::from_utf8_lossy(&raw).trim().to_owned(),
        location,
    ))
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
    let (status, text, _) = multipart_post(
        "Catbox",
        &env.catbox_url,
        &fields,
        "fileToUpload",
        path,
        None,
    )
    .await?;
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
        validate::custom_url(setting(settings, "upload_server_url")).map_err(Failure::Settings)?;
    let auth = validate::auth_header(setting(settings, "upload_auth_header"))
        .map_err(Failure::Settings)?;
    let (status, text, location) =
        multipart_post("your server", &url, &[], "clip", path, auth.as_deref()).await?;
    if !(200..300).contains(&status) {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let to = redirect_target(&url, location.as_deref());
        return Err(custom_upload_failure(
            status,
            &text,
            to,
            auth.as_deref(),
            size,
        ));
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
        settings_problem: false,
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
                    break Err((m, false));
                }
                eprintln!(
                    "[Uploader] Attempt {} failed; retrying in {}s: {m}",
                    tries + 1,
                    wait.as_secs()
                );
                tokio::time::sleep(wait).await;
                tries += 1;
            }
            Err(Failure::Transient(m) | Failure::Permanent(m)) => break Err((m, false)),
            Err(Failure::Settings(m)) => break Err((m, true)),
        }
    };
    let result = match result {
        Ok(r) => r,
        Err((m, settings_problem)) => {
            return Response {
                settings_problem,
                ..failed(if m.is_empty() {
                    "Upload failed.".into()
                } else {
                    m
                })
            };
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
    let message = if url.is_empty() && provider == Provider::Custom {
        "Uploaded, but your server didn't reply with a link.".to_owned()
    } else if url.is_empty() {
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
            settings_problem: false,
        };
    }
    Response {
        ok: true,
        message,
        upload_info: Some(entry),
        settings_problem: false,
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
        Err(e) => (
            false,
            format!("Could not reach Catbox: {}", sentence(&describe(&e))),
        ),
    }
}

/// Test connection to your own server: success or 405 means reachable.
/// Redirects are not followed, because uploads do not follow them either:
/// a test that passed through one would promise uploads that then fail.
pub async fn test_custom(settings: &Map<String, Value>) -> Result<(bool, String), String> {
    let url = validate::custom_url(setting(settings, "upload_server_url"))?;
    let auth = validate::auth_header(setting(settings, "upload_auth_header"))?;
    let client = client(TEST_TIMEOUT, false)?;
    let mut request = client.head(&url);
    if let Some(a) = &auth {
        request = request.header(reqwest::header::AUTHORIZATION, a);
    }
    let r = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            return Ok((
                false,
                format!("Could not reach your server: {}", sentence(&describe(&e))),
            ));
        }
    };
    let code = r.status().as_u16();
    let location = r
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok());
    Ok(match code {
        // A bare address answers with a web page; uploads there fail.
        200..=299 | 405 if reqwest::Url::parse(&url).is_ok_and(|u| u.path() == "/") => (
            false,
            format!(
                "Your server answered (HTTP {code}), but the URL has no path. FTHR Clips Server \
                 takes uploads at /upload."
            ),
        ),
        200..=299 | 405 => (true, format!("Your server reachable (HTTP {code}).")),
        300..=399 => match redirect_target(&url, location) {
            Some(to) => (
                false,
                format!("Your server redirects to {to}. Use that address as the server URL."),
            ),
            None => (
                false,
                format!("Your server redirects this URL (HTTP {code}). Check the server URL."),
            ),
        },
        401 | 403 => (false, auth_rejected(code, auth.as_deref())),
        404 => (
            false,
            format!("Your server has nothing at this URL (HTTP 404). {AT_UPLOAD}"),
        ),
        429 => (
            false,
            "Your server is limiting requests right now (HTTP 429). Try again in a minute.".into(),
        ),
        502..=504 => (
            false,
            format!("Your server is unavailable right now (HTTP {code})."),
        ),
        500.. => (false, format!("Your server hit an error (HTTP {code}).")),
        _ => (false, format!("Your server returned HTTP {code}.")),
    })
}
