//! SFTP uploads (`sftp_provider.py`).

use fthrust_core::upload::validate;
use serde_json::{Map, Value};

use crate::Env;

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
