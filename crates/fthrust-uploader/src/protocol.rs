//! Request gating and dispatch (`uploader_service.py`). The activation
//! receipt is gone: the uploader ships with the app, and the consent the
//! receipt proved is read from the uploader settings instead.

use std::path::{Path, PathBuf};

use fthrust_core::upload::settings::{Provider, UploaderSettings};
use fthrust_core::upload::validate::{resolve_lenient, under_roots};
use serde_json::{Map, Value};

use crate::{Env, Response, http, sftp};

const ACTIONS: [&str; 4] = ["upload", "test_connection", "sftp_save", "sftp_forget"];

/// Answers one request.
pub async fn handle(env: &Env, request: &Value) -> Response {
    match perform(env, request).await {
        Ok(r) => r,
        Err(message) => Response::fail(message),
    }
}

fn config_of(request: &Map<String, Value>) -> Map<String, Value> {
    match request.get("config") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    }
}

async fn perform(env: &Env, request: &Value) -> Result<Response, String> {
    let Value::Object(request) = request else {
        return Err("Uploader request must be a JSON object.".into());
    };
    let action = match request.get("action") {
        Some(Value::String(s)) => s.trim().to_owned(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    if !ACTIONS.contains(&action.as_str()) {
        let shown = if action.is_empty() {
            "(missing)"
        } else {
            action.as_str()
        };
        return Err(format!("Unsupported uploader action: {shown}"));
    }
    let settings = UploaderSettings::load(&env.paths);
    if matches!(action.as_str(), "upload" | "test_connection") && !settings.enabled() {
        return Err("The Upload Extension is disabled in FTHR Clips.".into());
    }
    if settings.str("uploader_terms_accepted_version") != fthrust_core::upload::TERMS_VERSION {
        return Err("Uploader Terms of Service must be accepted again.".into());
    }
    if settings.str("uploader_privacy_accepted_version") != fthrust_core::upload::PRIVACY_VERSION {
        return Err("Uploader Privacy Policy must be accepted again.".into());
    }
    let config = config_of(request);
    match action.as_str() {
        "sftp_save" => return sftp::save(env, &config).map(Response::done),
        "sftp_forget" => return sftp::forget(env, &config).map(Response::done),
        _ => {}
    }
    let provider_id = config
        .get("upload_provider")
        .and_then(Value::as_str)
        .unwrap_or_else(|| settings.str("upload_provider"))
        .to_owned();
    let provider = Provider::parse(&provider_id).ok_or("Unsupported upload provider.")?;
    if provider == Provider::Catbox && !settings.catbox_consent_current() {
        return Err("Accept Catbox\u{2019}s legal policies in FTHR Clips first.".into());
    }
    // Request config overrides the file (and may carry a transient password).
    let mut runtime = settings.data().clone();
    runtime.extend(config);
    if action == "test_connection" {
        let (ok, message) = match provider {
            Provider::Catbox => http::test_catbox(env).await,
            Provider::Custom => http::test_custom(&runtime).await?,
            Provider::Sftp => sftp::test(env, &runtime).await?,
        };
        return Ok(Response {
            ok,
            message,
            upload_info: None,
        });
    }
    let path = validated_clip_path(request)?;
    Ok(http::upload(env, provider, &runtime, &path).await)
}

/// `_validated_clip_path`: the real file must sit under an approved root.
fn validated_clip_path(request: &Map<String, Value>) -> Result<PathBuf, String> {
    let raw = request.get("path").and_then(Value::as_str).unwrap_or("");
    let missing = || "The selected clip does not exist.".to_owned();
    if raw.is_empty() {
        return Err(missing());
    }
    let target = resolve_lenient(Path::new(raw))
        .canonicalize()
        .map_err(|_| missing())?;
    if !target.is_file() {
        return Err(missing());
    }
    let roots: Vec<PathBuf> = match request.get("allowed_roots") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(|s| resolve_lenient(Path::new(s)))
            .collect(),
        _ => Vec::new(),
    };
    if !under_roots(&target, &roots) {
        return Err("The uploader may only read approved clip folders.".into());
    }
    Ok(target)
}
