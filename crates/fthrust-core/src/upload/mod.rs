//! The optional uploader's shared model, with no network code: its settings
//! file, the upload history, consent versions and the validation the
//! settings widget and the `fthrust-uploader` binary both apply.

pub mod history;
pub mod settings;
pub mod validate;

/// Version of Catbox's legal policies the user accepts before Catbox uploads.
pub const CATBOX_LEGAL_VERSION: &str = "catbox-legal-2021-03-06";
/// Uploader Terms of Service text (shown from the Upload settings link).
pub const TERMS_TEXT: &str = include_str!("legal/TERMS_OF_SERVICE.txt");
/// Uploader Privacy Policy text.
pub const PRIVACY_TEXT: &str = include_str!("legal/PRIVACY_POLICY.txt");
/// Catbox's donation page (the "Support Catbox hosting" button).
pub const CATBOX_SUPPORT_URL: &str = "https://catbox.moe/support.php";
/// Catbox's legal page (the consent dialog link).
pub const CATBOX_LEGAL_URL: &str = "https://catbox.moe/legal.php";
/// Catbox's upload size limit.
pub const CATBOX_LIMIT_BYTES: u64 = 200 * 1024 * 1024;

/// The current UTC time as Python's `isoformat(timespec='seconds')`.
pub fn utc_now_iso() -> String {
    let fmt =
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]+00:00");
    time::OffsetDateTime::now_utc()
        .format(&fmt)
        .unwrap_or_default()
}

/// Python's `str()` of a JSON value, for fields the reference reads with
/// `str(settings.get(...))`.
pub(crate) fn py_str(v: Option<&serde_json::Value>, default: &str) -> String {
    use serde_json::Value;
    match v {
        None => default.to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) => "None".into(),
        Some(Value::Bool(true)) => "True".into(),
        Some(Value::Bool(false)) => "False".into(),
        Some(other) => other.to_string(),
    }
}

/// Python truthiness of a JSON value.
pub fn truthy(v: Option<&serde_json::Value>) -> bool {
    use serde_json::Value;
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}
