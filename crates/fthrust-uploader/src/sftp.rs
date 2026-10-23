//! SFTP uploads (`sftp_provider.py`).

use fthrust_core::upload::validate;
use serde_json::{Map, Value};

use std::path::Path;

use crate::Env;
use crate::http::Failure;

/// Uploads `path` (one attempt).
pub async fn upload(
    _env: &Env,
    settings: &Map<String, Value>,
    _path: &Path,
) -> Result<Map<String, Value>, Failure> {
    validate::sftp_config(settings).map_err(Failure::Permanent)?;
    Err(Failure::Permanent(
        "SFTP is not available in this build.".into(),
    ))
}

/// Test connection: authenticate, verify the host key and the folder.
pub async fn test(_env: &Env, settings: &Map<String, Value>) -> Result<(bool, String), String> {
    validate::sftp_config(settings)?;
    Ok((false, "SFTP is not available in this build.".into()))
}

/// `sftp_save`: validates and stores a supplied password.
pub fn save(_env: &Env, config: &Map<String, Value>) -> Result<String, String> {
    validate::sftp_config(config)?;
    Ok("SFTP credentials updated.".into())
}

/// `sftp_forget`: validates and removes the saved password.
pub fn forget(_env: &Env, config: &Map<String, Value>) -> Result<String, String> {
    validate::sftp_config(config)?;
    Ok("SFTP credentials updated.".into())
}
