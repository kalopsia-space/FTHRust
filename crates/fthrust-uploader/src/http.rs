//! Catbox and your own server (`upload_runtime.py`), plus the upload
//! wrapper every provider goes through (size limit, retries, history,
//! auto-delete).

use std::path::Path;
use std::sync::{Arc, Mutex};
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

/// An HTTP client. `read` is reqwest's read timeout, which is one timer from
/// the start of the request until the response headers arrive: right for a
/// request without a body (the connection tests), wrong for an upload, which
/// must not be cut off while it is still moving (see [`multipart_post`]).
fn client(
    connect: Duration,
    read: Option<Duration>,
    follow_redirects: bool,
) -> Result<reqwest::Client, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let policy = if follow_redirects {
        reqwest::redirect::Policy::default()
    } else {
        reqwest::redirect::Policy::none()
    };
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(connect)
        .redirect(policy);
    if let Some(read) = read {
        builder = builder.read_timeout(read);
    }
    builder
        .build()
        .map_err(|e| format!("The uploader could not start its HTTP client: {e}"))
}

/// A readable account of a request error: a lower-case clause for
/// "Could not reach …: <clause>", without the full stop.
pub fn describe(e: &reqwest::Error) -> String {
    // reqwest names the whole URL in its errors ("error sending request for
    // url (…)"), and a server URL is where secrets get put.
    scrub_urls(&describe_raw(e))
}

/// `text` with every http(s) URL in it cut down to scheme, host, port and
/// path: no `user:password@`, no `?token=…`, no `#fragment`.
fn scrub_urls(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(start) = ["http://", "https://"]
        .iter()
        .filter_map(|scheme| lower[at..].find(scheme).map(|i| at + i))
        .min()
    {
        out.push_str(&text[at..start]);
        // A URL runs to the next space: query strings may hold any
        // punctuation, and what is left of one after a stray comma or
        // bracket is the part that would leak. Only the punctuation that
        // closes a sentence or a bracket at the very end is not part of it.
        let end = text[start..]
            .find(char::is_whitespace)
            .map_or(text.len(), |n| start + n);
        let url_end = start
            + text[start..end]
                .trim_end_matches([')', ']', '}', '>', '"', '\'', '.', ',', ';', ':', '!', '?'])
                .len();
        out.push_str(&plain_url(&text[start..url_end]));
        out.push_str(&text[url_end..end]);
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// One URL without user info, query and fragment.
fn plain_url(url: &str) -> String {
    if let Ok(mut u) = reqwest::Url::parse(url) {
        let _ = u.set_username("");
        let _ = u.set_password(None);
        u.set_query(None);
        u.set_fragment(None);
        return u.to_string();
    }
    // Not a URL that can be taken apart: cut it by hand.
    let url = url.find(['?', '#']).map_or(url, |i| &url[..i]);
    let after = url.find("://").map_or(0, |i| i + 3);
    let authority_end = url[after..].find('/').map_or(url.len(), |i| after + i);
    match url[after..authority_end].rfind('@') {
        Some(i) => format!("{}{}", &url[..after], &url[after + i + 1..]),
        None => url.to_owned(),
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

/// The pieces of the request body are read from the file this big, so the
/// watchdog sees the body move often even on a slow link.
const CHUNK: usize = 256 * 1024;

/// How far an upload's body has got, for the stall watchdog.
struct Progress {
    /// When hyper last took a piece of the body (when the request began, until it does).
    last: tokio::time::Instant,
    /// The last piece has been taken: the server's answer is next.
    finished: bool,
}

/// [`Progress`], shared between the body stream and the watchdog.
#[derive(Clone)]
struct Watch(Arc<Mutex<Progress>>);

impl Watch {
    fn new() -> Watch {
        Watch(Arc::new(Mutex::new(Progress {
            last: tokio::time::Instant::now(),
            finished: false,
        })))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, Progress> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A piece of the body was taken.
    fn moved(&self) {
        self.state().last = tokio::time::Instant::now();
    }

    /// The last piece of the body is being taken.
    fn finish(&self) {
        let mut p = self.state();
        p.last = tokio::time::Instant::now();
        p.finished = true;
    }
}

/// A request that went quiet.
enum Quiet {
    /// The body stopped going out.
    Sending,
    /// The body is out and the server does not answer.
    Answering,
}

/// `request.send()`, given up on when the body has not moved for `limit`
/// (or, once it is all out, when the server has not answered for `limit`).
///
/// reqwest's own read timeout cannot do this: it is one timer from the start
/// of the request until the response headers, and body progress never resets
/// it, so every upload that takes longer than the timeout to send would fail
/// however fast it was going.
async fn send_watched(
    request: reqwest::RequestBuilder,
    watch: &Watch,
    limit: Duration,
) -> Result<Result<reqwest::Response, reqwest::Error>, Quiet> {
    let send = request.send();
    tokio::pin!(send);
    loop {
        let due = watch.state().last + limit;
        tokio::select! {
            sent = &mut send => return Ok(sent),
            () = tokio::time::sleep_until(due) => {
                let p = watch.state();
                if p.last + limit <= tokio::time::Instant::now() {
                    return Err(if p.finished { Quiet::Answering } else { Quiet::Sending });
                }
            }
        }
    }
}

/// A duration for a message: whole seconds, else milliseconds.
fn words(d: Duration) -> String {
    let ms = d.as_millis();
    if ms >= 1000 && ms.is_multiple_of(1000) {
        let s = ms / 1000;
        format!("{s} second{}", if s == 1 { "" } else { "s" })
    } else {
        format!("{ms} ms")
    }
}

/// `who` at the start of a sentence.
fn capitalized(who: &str) -> String {
    let mut chars = who.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `_multipart_post`: one streamed `multipart/form-data` POST with the
/// reference's boundary and explicit length. Returns the status, the
/// trimmed body text and any `Location` header. `who` names the server in
/// network errors ("Catbox", "your server").
///
/// There is no overall timeout: a clip is as long as it is. The request is
/// failed when it goes quiet for `limit` instead (see [`send_watched`]).
async fn multipart_post(
    limit: Duration,
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
    let watch = Watch::new();
    let (taken, last) = (watch.clone(), watch.clone());
    let body =
        futures_util::stream::once(async move { Ok::<Bytes, std::io::Error>(Bytes::from(prefix)) })
            .chain(tokio_util::io::ReaderStream::with_capacity(file, CHUNK))
            .chain(futures_util::stream::once(async move {
                last.finish();
                Ok::<Bytes, std::io::Error>(Bytes::from(footer))
            }))
            .inspect(move |_| taken.moved());
    // No read timeout: reqwest's covers the whole upload, not its silence.
    let client = client(limit, None, false).map_err(Failure::Permanent)?;
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
    let mut response = match send_watched(request, &watch, limit).await {
        Ok(Ok(response)) => response,
        // Nothing went out: trying again cannot change what the request is.
        Ok(Err(e)) if e.is_builder() => {
            return Err(Failure::Permanent(format!(
                "The upload request could not be built: {}",
                sentence(&describe(&e))
            )));
        }
        Ok(Err(e)) => {
            return Err(Failure::Transient(format!(
                "Could not reach {who}: {}",
                sentence(&describe(&e))
            )));
        }
        Err(Quiet::Sending) => {
            return Err(Failure::Transient(format!(
                "The upload to {who} stalled: no data went out for {}.",
                words(limit)
            )));
        }
        Err(Quiet::Answering) => {
            return Err(Failure::Transient(format!(
                "{} did not answer within {} of the end of the upload.",
                capitalized(who),
                words(limit)
            )));
        }
    };
    let status = response.status().as_u16();
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut raw = Vec::new();
    while raw.len() < BODY_CAP {
        match tokio::time::timeout(limit, response.chunk()).await {
            Ok(Ok(Some(chunk))) => raw.extend_from_slice(&chunk),
            Ok(Ok(None)) => break,
            Ok(Err(e)) => {
                return Err(Failure::Transient(format!(
                    "The connection to {who} dropped: {}",
                    sentence(&describe(&e))
                )));
            }
            Err(_) => {
                return Err(Failure::Transient(format!(
                    "The answer from {who} stalled: no data for {}.",
                    words(limit)
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
        env.io_timeout,
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

/// Why a request to `url` with the Auth header `auth` can never be built
/// (the message for the settings), if so. reqwest only finds out when the
/// request is sent, and then calls it an error like any other.
fn unusable(url: &str, auth: Option<&str>) -> Option<&'static str> {
    if reqwest::Url::parse(url).is_err() {
        return Some("The server URL is not a valid address. Check the server URL.");
    }
    if auth.is_some_and(|a| reqwest::header::HeaderValue::from_str(a).is_err()) {
        return Some(
            "The Auth header has characters that can't be sent. Enter it again in the upload settings.",
        );
    }
    None
}

async fn upload_custom(
    env: &Env,
    settings: &Map<String, Value>,
    path: &Path,
) -> Result<Map<String, Value>, Failure> {
    let url =
        validate::custom_url(setting(settings, "upload_server_url")).map_err(Failure::Settings)?;
    let auth = validate::auth_header(setting(settings, "upload_auth_header"))
        .map_err(Failure::Settings)?;
    if let Some(problem) = unusable(&url, auth.as_deref()) {
        return Err(Failure::Settings(problem.into()));
    }
    let (status, text, location) = multipart_post(
        env.io_timeout,
        "your server",
        &url,
        &[],
        "clip",
        path,
        auth.as_deref(),
    )
    .await?;
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
        Provider::Custom => upload_custom(env, settings, path).await,
        Provider::Sftp => sftp::upload(env, settings, path).await,
    }
}

/// What a server says when it took the upload but gave no usable link.
const NO_LINK: &str = "Uploaded, but your server didn't reply with a link.";

/// The link of a finished upload, if it has one anybody can use: exactly
/// what the app's `History::link` accepts (an http(s) URL, or an `sftp://`
/// location with a host, a port and a path), on a single line. Only such a
/// link confirms an upload enough to delete the clip; a server may put
/// anything in its reply (`queued`, `file:///…`, a few lines of text).
fn usable_link(entry: &Map<String, Value>) -> Option<String> {
    let key = Path::new("upload");
    let mut history = History::default();
    history.record(key, Entry(entry.clone()));
    let url = history.link(key)?.url;
    (!url.chars().any(|c| c.is_whitespace() || c.is_control())).then_some(url)
}

/// `UploadRuntime.upload`: size limit, attempts, history, auto-delete.
/// Auto-delete only removes a clip inside `clips_root`.
pub async fn upload(
    env: &Env,
    provider: Provider,
    settings: &Map<String, Value>,
    path: &Path,
    clips_root: Option<&Path>,
) -> Response {
    let failed = |message: String| Response {
        ok: false,
        message,
        upload_info: Some(Map::new()),
        settings_problem: false,
        warning: None,
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
    let saved = History::update(&env.paths, |h| h.record(path, Entry(entry.clone())));
    let link = usable_link(&entry);
    let message = match &link {
        Some(link) => link.clone(),
        None if provider == Provider::Custom => NO_LINK.to_owned(),
        None => "Upload complete.".to_owned(),
    };
    if let Err(e) = saved {
        // The upload exists, but nothing records it, and the app learns a
        // clip's link only from the history. Deleting the clip now would
        // leave a copy on the server, no local file and no link anywhere:
        // keep it, and hand the link over in the reply.
        eprintln!("[Uploader] Upload history could not be saved: {e}");
        let was = match &link {
            Some(link) => format!("Uploaded to {link}"),
            None => "Uploaded".to_owned(),
        };
        return Response {
            ok: true,
            message,
            upload_info: Some(entry),
            settings_problem: false,
            warning: Some(format!(
                "{was}, but the upload history could not be saved ({e}). The clip was kept."
            )),
        };
    }
    let auto_delete = fthrust_core::upload::truthy(settings.get("upload_auto_delete"));
    let mut message = message;
    let mut warning = None;
    if link.is_none() {
        // Without a link the upload is not confirmed: keep the only copy, and
        // say that there is nothing to copy from the card.
        let said = if provider == Provider::Custom {
            NO_LINK
        } else {
            "Uploaded, but the reply had no usable link."
        };
        let kept = if auto_delete {
            " The clip was kept."
        } else {
            ""
        };
        warning = Some(format!("{said}{kept}"));
    } else if auto_delete
        // Clips outside the clips folder (imported libraries) are never ours
        // to delete.
        && clips_root.is_some_and(|root| path.starts_with(root))
        && let Err(e) = std::fs::remove_file(path)
    {
        message = format!("Uploaded, but local delete failed: {e}");
        warning = Some(message.clone());
    }
    Response {
        ok: true,
        message,
        upload_info: Some(entry),
        settings_problem: false,
        warning,
    }
}

/// Test connection to Catbox: any answer below 500 means reachable.
pub async fn test_catbox(env: &Env) -> (bool, String) {
    let client = match client(TEST_TIMEOUT, Some(TEST_TIMEOUT), true) {
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
    if let Some(problem) = unusable(&url, auth.as_deref()) {
        return Err(problem.into());
    }
    let client = client(TEST_TIMEOUT, Some(TEST_TIMEOUT), false)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_in_messages_lose_their_secrets() {
        for (text, want) in [
            (
                "error sending request for url (https://u:p@host:8080/a/b?token=1#x): boom",
                "error sending request for url (https://host:8080/a/b): boom",
            ),
            ("no address in this one", "no address in this one"),
            (
                "from http://a.example/x?k=v to HTTPS://B:C@b.example/y?z",
                "from http://a.example/x to https://b.example/y",
            ),
            // An address that cannot be taken apart is cut by hand.
            (
                "bad url (http://me:pw@[broken/up?x=1)",
                "bad url (http://[broken/up)",
            ),
            ("scheme only http://", "scheme only http://"),
            // Punctuation inside a query is no end of the URL.
            (
                "failed for url (http://h/p?a=1,b;c)d=SECRET): refused.",
                "failed for url (http://h/p): refused.",
            ),
        ] {
            assert_eq!(scrub_urls(text), want, "{text}");
        }
    }
}
