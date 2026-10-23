//! Validation shared by the upload settings widget and the uploader: SFTP
//! fields (`sftp_provider.configuration`), the public folder URL, the custom
//! server URL and header, the VaultScope login paste, and which files may be
//! uploaded from where.

use std::path::{Component, Path, PathBuf};

use base64::Engine;
use serde_json::{Map, Value};

use super::{py_str, truthy};
use crate::settings::SettingsStore;

/// A validated SFTP connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpConfig {
    /// Lower-case host or IP (IDNA-encoded).
    pub host: String,
    /// Port.
    pub port: u16,
    /// User name.
    pub username: String,
    /// Remote folder.
    pub folder: String,
    /// `SHA256:…` host-key pin.
    pub fingerprint: Option<String>,
}

/// A parsed VaultScope login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpLogin {
    /// Host.
    pub host: String,
    /// Port.
    pub port: u16,
    /// User name.
    pub username: String,
    /// Password.
    pub password: String,
}

/// The parts of a URL the reference checks (Python `urlsplit`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct UrlParts {
    pub scheme: String,
    pub netloc: String,
    pub host: String,
    pub has_userinfo: bool,
    pub port_ok: bool,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

/// Splits `scheme://netloc/path?query#fragment`; `None` without `://`.
pub(crate) fn split_url(url: &str) -> Option<UrlParts> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        return None;
    }
    let (rest, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (netloc, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (has_userinfo, hostport) = match netloc.rsplit_once('@') {
        Some((_, hp)) => (true, hp),
        None => (false, netloc),
    };
    let (host, port) = if let Some(stripped) = hostport.strip_prefix('[') {
        match stripped.split_once(']') {
            Some((h, after)) => (h, after.strip_prefix(':')),
            None => return None,
        }
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        }
    };
    let port_ok = match port {
        None | Some("") => true,
        Some(p) => {
            p.chars().all(|c| c.is_ascii_digit())
                && p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n))
        }
    };
    Some(UrlParts {
        scheme: scheme.to_lowercase(),
        netloc: netloc.to_owned(),
        host: host.to_lowercase(),
        has_userinfo,
        port_ok,
        path: path.to_owned(),
        query: query.to_owned(),
        fragment: fragment.to_owned(),
    })
}

const PUBLIC_URL_ERROR: &str =
    "Enter a public HTTPS folder URL without credentials, query or fragment.";

/// `public_base_url`: `""` unless public links are on, else the HTTPS
/// folder URL with exactly one trailing slash.
pub fn public_base_url(settings: &Map<String, Value>) -> Result<String, String> {
    if !truthy(settings.get("sftp_public_enabled")) {
        return Ok(String::new());
    }
    let value = py_str(settings.get("sftp_public_base_url"), "")
        .trim()
        .to_owned();
    let p = split_url(&value).ok_or(PUBLIC_URL_ERROR)?;
    if p.scheme != "https"
        || p.host.is_empty()
        || p.has_userinfo
        || !p.query.is_empty()
        || !p.fragment.is_empty()
        || !p.port_ok
        || value.chars().any(|c| (c as u32) < 33)
        || value.contains('\\')
    {
        return Err(PUBLIC_URL_ERROR.into());
    }
    Ok(format!(
        "{}://{}{}/",
        p.scheme,
        p.netloc,
        p.path.trim_end_matches('/')
    ))
}

fn py_int(v: Option<&Value>, default: i64) -> Option<i64> {
    match v {
        None => Some(default),
        Some(Value::Bool(b)) => Some(*b as i64),
        Some(Value::Number(n)) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f.trunc() as i64)
        }),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

fn label_ok(label: &str) -> bool {
    let bytes = label.as_bytes();
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !bytes.is_empty()
        && bytes.len() <= 63
        && alnum(bytes[0])
        && alnum(bytes[bytes.len() - 1])
        && bytes.iter().all(|&b| alnum(b) || b == b'-')
}

fn idna_host(host: &str) -> Result<String, ()> {
    if host.is_empty() {
        return Ok(String::new());
    }
    let mut labels: Vec<&str> = host.split('.').collect();
    if labels.last() == Some(&"") {
        labels.pop();
    }
    let mut out = Vec::with_capacity(labels.len());
    for label in labels {
        let ascii = if label.is_ascii() {
            label.to_owned()
        } else {
            idna::domain_to_ascii(label).map_err(|_| ())?
        };
        if ascii.is_empty() || ascii.len() > 63 {
            return Err(());
        }
        out.push(ascii);
    }
    Ok(out.join("."))
}

/// `configuration`: validates every SFTP field (the public URL first).
pub fn sftp_config(settings: &Map<String, Value>) -> Result<SftpConfig, String> {
    public_base_url(settings)?;
    let mut host = py_str(settings.get("sftp_host"), "")
        .trim()
        .trim_end_matches('.')
        .to_lowercase();
    if host.parse::<std::net::IpAddr>().is_err() {
        host = idna_host(&host)
            .map_err(|_| "Enter a valid SFTP host domain or IP address.".to_owned())?;
        if host.len() > 253 || host.is_empty() || !host.split('.').all(label_ok) {
            return Err("Enter a host domain or IP address without a URL or path.".into());
        }
    }
    let port = py_int(settings.get("sftp_port"), 22)
        .filter(|p| (1..=65535).contains(p))
        .ok_or("SFTP port must be between 1 and 65535.")? as u16;
    let username = py_str(settings.get("sftp_username"), "").trim().to_owned();
    if username.is_empty() || username.chars().any(|c| (c as u32) < 32) {
        return Err("Enter an SFTP username without control characters.".into());
    }
    let folder = if truthy(settings.get("sftp_directory")) {
        py_str(settings.get("sftp_directory"), ".")
    } else {
        ".".to_owned()
    };
    let folder = folder.trim().to_owned();
    if folder.is_empty() || folder.chars().any(|c| (c as u32) < 32) || folder.contains('\\') {
        return Err("Enter a remote folder using forward slashes.".into());
    }
    let fingerprint = fingerprint(&py_str(settings.get("sftp_fingerprint"), ""))?;
    Ok(SftpConfig {
        host,
        port,
        username,
        folder,
        fingerprint,
    })
}

/// A host-key pin: blank is none, else `SHA256:` + 43 base64 characters
/// that round-trip exactly.
pub fn fingerprint(value: &str) -> Result<Option<String>, String> {
    let fp = value.trim();
    if fp.is_empty() {
        return Ok(None);
    }
    let body = fp.strip_prefix("SHA256:").unwrap_or("");
    let shaped = fp.starts_with("SHA256:")
        && body.len() == 43
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/');
    if !shaped {
        return Err("Enter the server SHA256 host-key fingerprint from its operator.".into());
    }
    let engine = base64::engine::general_purpose::STANDARD;
    let invalid = || "Invalid SHA256 host-key fingerprint.".to_owned();
    let digest = engine.decode(format!("{body}=")).map_err(|_| invalid())?;
    if engine.encode(digest).trim_end_matches('=') != body {
        return Err(invalid());
    }
    Ok(Some(fp.to_owned()))
}

/// `_custom_server_url`: adds `https://` when no http(s) scheme is given.
pub fn custom_url(value: &str) -> Result<String, String> {
    let mut url = value.trim().to_owned();
    let lower = url.to_lowercase();
    if !url.is_empty() && !lower.starts_with("http://") && !lower.starts_with("https://") {
        url = format!("https://{url}");
    }
    if url.is_empty() {
        return Err("Set a server URL before connecting to your server.".into());
    }
    match split_url(&url) {
        Some(p) if (p.scheme == "http" || p.scheme == "https") && !p.host.is_empty() => Ok(url),
        _ => Err("Your server URL must use http:// or https://.".into()),
    }
}

/// The settings widget's normalisation on Save (no validation).
pub fn save_server_url(value: &str) -> String {
    let url = value.trim();
    let lower = url.to_lowercase();
    if url.is_empty() || lower.starts_with("http://") || lower.starts_with("https://") {
        url.to_owned()
    } else {
        format!("https://{url}")
    }
}

/// `_custom_server_headers`: the trimmed `Authorization` value, if any.
pub fn auth_header(value: &str) -> Result<Option<String>, String> {
    let v = value.trim();
    if v.contains('\r') || v.contains('\n') {
        return Err("The authorization header contains an invalid line break.".into());
    }
    Ok((!v.is_empty()).then(|| v.to_owned()))
}

/// `_paste_vaultscope_login`: parses the copied login JSON.
pub fn vaultscope_paste(raw: &str) -> Result<SftpLogin, String> {
    let bad = || {
        "Copy the complete VaultScope login JSON, then paste again. It must include \
         providerName, domain, port, username and password."
            .to_owned()
    };
    if raw.chars().count() > 65536 {
        return Err(bad());
    }
    let Ok(Value::Object(d)) = serde_json::from_str::<Value>(raw) else {
        return Err(bad());
    };
    if d.get("providerName").and_then(Value::as_str) != Some("VaultScope") {
        return Err(bad());
    }
    let host = d.get("domain").and_then(Value::as_str).ok_or_else(bad)?;
    let username = d.get("username").and_then(Value::as_str).ok_or_else(bad)?;
    let password = d.get("password").and_then(Value::as_str).ok_or_else(bad)?;
    let port = match d.get("port") {
        Some(Value::Number(n)) => n.as_i64().ok_or_else(bad)?,
        _ => return Err(bad()),
    };
    if host.trim().to_lowercase() != "sftp.vaultscope.de"
        || username.trim().is_empty()
        || username.chars().any(|c| (c as u32) < 32)
        || password.is_empty()
        || !(1..=65535).contains(&port)
    {
        return Err(bad());
    }
    Ok(SftpLogin {
        host: host.trim().to_lowercase(),
        port: port as u16,
        username: username.trim().to_owned(),
        password: password.to_owned(),
    })
}

/// A finished video the uploader accepts.
pub fn is_uploadable(path: &Path) -> bool {
    crate::clip_files::is_completed_video_path(path)
}

/// `path` is one of `roots` or inside one.
pub fn under_roots(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| path.starts_with(r))
}

/// Python's `Path.resolve(strict=False)`: absolute, `..` folded, symlinks
/// of the existing part followed.
pub fn resolve_lenient(path: &Path) -> PathBuf {
    let expanded = match path.strip_prefix("~") {
        Ok(rest) => std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|| path.to_path_buf()),
        Err(_) => path.to_path_buf(),
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map(|d| d.join(&expanded))
            .unwrap_or(expanded)
    };
    let mut lexical = PathBuf::new();
    for c in absolute.components() {
        match c {
            Component::ParentDir => {
                lexical.pop();
            }
            Component::CurDir => {}
            other => lexical.push(other),
        }
    }
    let mut existing = lexical.clone();
    let mut tail = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            let mut out = real;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (
            existing.file_name().map(|f| f.to_owned()),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                existing = parent.to_path_buf();
            }
            _ => return lexical,
        }
    }
}

/// `_approved_roots`: the clips root plus every imported folder, resolved.
pub fn allowed_roots(clips_root: &Path, core: &SettingsStore) -> Vec<PathBuf> {
    let mut roots = vec![resolve_lenient(clips_root)];
    if let Some(list) = core.get("imported_clip_folders").and_then(Value::as_array) {
        roots.extend(
            list.iter()
                .filter_map(Value::as_str)
                .map(|s| resolve_lenient(Path::new(s))),
        );
    }
    roots
}
