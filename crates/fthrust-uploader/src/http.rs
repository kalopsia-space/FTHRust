//! Catbox and your own server (`upload_runtime.py`).

use std::path::Path;

use fthrust_core::upload::settings::Provider;
use fthrust_core::upload::validate;
use serde_json::{Map, Value};

use crate::{Env, Response};

/// Test connection to Catbox.
pub async fn test_catbox(_env: &Env) -> (bool, String) {
    (
        false,
        "Catbox uploads are not available in this build.".into(),
    )
}

/// Test connection to your own server.
pub async fn test_custom(settings: &Map<String, Value>) -> Result<(bool, String), String> {
    let s = |k: &str| {
        settings
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    validate::custom_url(&s("upload_server_url"))?;
    validate::auth_header(&s("upload_auth_header"))?;
    Ok((
        false,
        "Server uploads are not available in this build.".into(),
    ))
}

/// Uploads `path` with `provider`.
pub async fn upload(
    _env: &Env,
    _provider: Provider,
    _settings: &Map<String, Value>,
    _path: &Path,
) -> Response {
    Response::fail("Uploads are not available in this build.")
}
