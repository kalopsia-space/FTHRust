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
        r#"(?i)\b(Authorization|X-Api-Key|X-Auth-Token|Proxy-Authorization)\s*["']?\s*[:=]\s*("[^"]*"|'[^']*'|[^,;\s}\]]+)"#,
    )
    .expect("valid regex")
});
/// A cookie header holds several `name=value` pairs separated by `;`, and
/// every one of them is a credential: an unquoted value runs to the end of
/// the line.
static RE_COOKIE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(Set-Cookie|Cookie)\s*["']?\s*[:=]\s*("[^"]*"|'[^']*'|[^\r\n]*)"#)
        .expect("valid regex")
});
/// `key=value` / `"key": "value"` for secret-looking names. The name may
/// carry a prefix (`sftp_password`, `client_secret`, `upload_token`,
/// `catbox_userhash`, `clientSecret`): anything that ends in a secret word.
static RE_KV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b([a-z0-9_.-]*(?:token|api_?key|password|passwd|secret|auth|userhash|passphrase)|upload_auth_header)\b(\s*["']?\s*[:=]\s*)("[^"]*"|'[^']*'|[^,;&\s}\]]+)"#,
    )
    .expect("valid regex")
});
/// A userhash named in running text, without a `=` or `:` (`using userhash
/// 0123456789abcdef`): the hex that follows it.
static RE_USERHASH_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\buserhash\b[^\n]{0,40}?\b)[0-9a-f]{8,}\b").expect("valid regex")
});

/// Strips anything credential-shaped out of `text`: URL user-info, bearer
/// style schemes, authorization-style headers (a `Cookie` header to the end
/// of the line), secret-looking key/value pairs (`password=`,
/// `sftp_password=`, `client_secret=`, `upload_token=`...) and a userhash
/// named in running text. Never fails.
pub fn redact(text: &str) -> String {
    let text = RE_URL_CREDS.replace_all(text, format!("://{REDACTED}:{REDACTED}@").as_str());
    let text = RE_BEARER.replace_all(&text, |caps: &regex::Captures| {
        format!("{} {REDACTED}", &caps[1])
    });
    let text = RE_HEADER.replace_all(&text, |caps: &regex::Captures| {
        format!("{}: {REDACTED}", &caps[1])
    });
    let text = RE_COOKIE.replace_all(&text, |caps: &regex::Captures| {
        format!("{}: {REDACTED}", &caps[1])
    });
    let text = RE_KV.replace_all(&text, |caps: &regex::Captures| {
        format!("{}{}{REDACTED}", &caps[1], &caps[2])
    });
    let text = RE_USERHASH_TEXT.replace_all(&text, |caps: &regex::Captures| {
        format!("{}{REDACTED}", &caps[1])
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

/// Words that mark a JSON object key as holding a secret wherever they sit in
/// it (`sftp_password`, `X-Api-Key`, `clientSecret`, `catbox_userhash`).
const SECRET_WORDS: &[&str] = &[
    "token",
    "api_key",
    "apikey",
    "password",
    "passwd",
    "passphrase",
    "secret",
    "userhash",
    "authorization",
    "auth_header",
    "cookie",
    "credential",
    "credentials",
];

/// `clientSecret` and `X-Api-Key` become `client_secret` and `x_api_key`.
fn snake_case(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    let mut prev_lower = false;
    for c in key.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            out.push('_');
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        out.push(match c {
            '-' | '.' | ' ' => '_',
            c => c.to_ascii_lowercase(),
        });
    }
    out
}

/// Whether the value under the JSON object key `key` is always replaced,
/// whatever it contains.
fn is_secret_key(key: &str) -> bool {
    let padded = format!("_{}_", snake_case(key));
    // A key whose last word ends in `auth` (`auth`, `oauth`, `proxy_auth`) is
    // a secret, but one that merely starts with it (`auth_required`) is not.
    padded.ends_with("auth_")
        || SECRET_WORDS
            .iter()
            .any(|w| padded.contains(&format!("_{w}_")))
}

/// Redacts a JSON value in place: string values go through [`redact_path`],
/// and any object entry whose key names a secret (`password`, `sftp_password`,
/// `upload_token`, `Cookie`...) is replaced wholesale.
pub fn redact_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => *s = redact_path(s),
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact_json),
        serde_json::Value::Object(map) => {
            for (key, entry) in map.iter_mut() {
                if is_secret_key(key) {
                    *entry = serde_json::Value::String(REDACTED.to_string());
                } else {
                    redact_json(entry);
                }
            }
        }
        _ => {}
    }
}
