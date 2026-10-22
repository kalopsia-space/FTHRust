//! Credential redaction for log lines and diagnostic text.
//!
//! Ports the regexes of `reference/FTHR_UI/core/diagnostics.py`. Matching
//! extra text is preferable to exposing a token in a bug report.

use std::sync::LazyLock;

use regex::Regex;

/// Replacement text for anything credential-shaped.
pub const REDACTED: &str = "<redacted>";

static RE_URL_CREDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"://([^/\s:@]+):([^/\s@]+)@").expect("valid regex"));
static RE_BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(Bearer|Token|Basic|ApiKey)\s+\S+").expect("valid regex"));
static RE_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(Authorization|Cookie|Set-Cookie|X-Api-Key|X-Auth-Token|Proxy-Authorization)\s*[:=]\s*("[^"]*"|'[^']*'|[^,;\s}\]]+)"#,
    )
    .expect("valid regex")
});
static RE_KV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(token|api_key|apikey|password|passwd|secret|auth|access_token|refresh_token|upload_auth_header)\b(\s*["']?\s*[:=]\s*)("[^"]*"|'[^']*'|[^,;&\s}\]]+)"#,
    )
    .expect("valid regex")
});

/// Strips anything credential-shaped out of `text`: URL user-info, bearer
/// style schemes, authorization-style headers and secret-looking key/value
/// pairs. Never fails.
pub fn redact(text: &str) -> String {
    let text = RE_URL_CREDS.replace_all(text, format!("://{REDACTED}:{REDACTED}@").as_str());
    let text = RE_BEARER.replace_all(&text, |caps: &regex::Captures| {
        format!("{} {REDACTED}", &caps[1])
    });
    let text = RE_HEADER.replace_all(&text, |caps: &regex::Captures| {
        format!("{}: {REDACTED}", &caps[1])
    });
    let text = RE_KV.replace_all(&text, |caps: &regex::Captures| {
        format!("{}{}{REDACTED}", &caps[1], &caps[2])
    });
    text.into_owned()
}

/// Redacts credentials and then replaces the user's home directory (and the
/// temp root when `TMPDIR`/`TEMP` is set) with the reference's placeholders,
/// case-insensitively, so exported diagnostics carry no profile paths.
pub fn redact_path(text: &str) -> String {
    let mut out = redact(text);
    let mut roots: Vec<(&str, String)> = Vec::new();
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && home != "/"
    {
        roots.push(("%USERPROFILE%", home));
    }
    for var in ["TMPDIR", "TEMP", "TMP"] {
        if let Ok(tmp) = std::env::var(var)
            && !tmp.is_empty()
            && tmp != "/"
            && tmp != "/tmp"
        {
            roots.push(("%TEMP%", tmp));
            break;
        }
    }
    for (token, root) in roots {
        let root = root.trim_end_matches('/');
        if root.is_empty() {
            continue;
        }
        if let Ok(re) = Regex::new(&format!("(?i){}", regex::escape(root))) {
            out = re.replace_all(&out, token).into_owned();
        }
    }
    out
}

/// Reduces a URL to its credential-free base and hides the whole query
/// string, which is where upload tokens live.
pub fn sanitize_url_for_log(url: &str) -> String {
    let text = RE_URL_CREDS.replace_all(url, format!("://{REDACTED}:{REDACTED}@").as_str());
    match text.split_once('?') {
        Some((base, _)) => format!("{base}?{REDACTED}"),
        None => text.into_owned(),
    }
}

/// Object keys whose values are always replaced, whatever they contain.
const SECRET_KEYS: &[&str] = &[
    "token",
    "api_key",
    "apikey",
    "password",
    "passwd",
    "secret",
    "auth",
    "access_token",
    "refresh_token",
    "upload_auth_header",
    "authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-auth-token",
    "proxy-authorization",
    "userhash",
    "catbox_userhash",
];

/// Redacts a JSON value in place: string values go through [`redact_path`],
/// and any object entry whose key names a secret is replaced wholesale.
pub fn redact_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => *s = redact_path(s),
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact_json),
        serde_json::Value::Object(map) => {
            for (key, entry) in map.iter_mut() {
                if SECRET_KEYS.contains(&key.to_ascii_lowercase().as_str()) {
                    *entry = serde_json::Value::String(REDACTED.to_string());
                } else {
                    redact_json(entry);
                }
            }
        }
        _ => {}
    }
}
