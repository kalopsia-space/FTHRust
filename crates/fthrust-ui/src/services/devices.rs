//! What this machine offers the settings pages: the encoders that really
//! work (a one-frame test encode per candidate, cached in
//! `encoder_capabilities.json`) and the audio devices (the engine's
//! `--list-playback-devices` / `--list-microphones` listings, as the
//! reference asked its capture engine). Both run off the UI thread and come
//! back as events.

use std::path::Path;
use std::time::Duration;

use fthrust_media::capabilities::{
    CacheSignature, EncoderCapability, load_cached, probe_capabilities, store_cached,
};
use serde_json::Value;

/// How long one device listing may take.
pub const LISTING_TIMEOUT: Duration = Duration::from_secs(6);

/// `(label, id)` pairs, the shape the Audio page's combos take.
pub type DeviceList = Vec<(String, String)>;

/// The working encoders: the cache when it was made for the FFmpeg build in
/// use (unless `force`), else a fresh probe, which is then cached.
pub fn encoder_capabilities(cache: &Path, force: bool) -> Vec<EncoderCapability> {
    encoder_capabilities_with(
        cache,
        force,
        CacheSignature::current().ok(),
        probe_capabilities,
    )
}

/// [`encoder_capabilities`] with the signature and the probe injected.
pub fn encoder_capabilities_with(
    cache: &Path,
    force: bool,
    signature: Option<CacheSignature>,
    probe: impl FnOnce() -> Vec<EncoderCapability>,
) -> Vec<EncoderCapability> {
    if !force
        && let Some(sig) = &signature
        && let Some(cached) = load_cached(cache, sig)
    {
        return cached;
    }
    let found = probe();
    match &signature {
        Some(sig) => {
            if let Err(e) = store_cached(cache, sig, &found) {
                tracing::warn!(error = %e, "encoder capabilities not cached");
            }
        }
        None => tracing::warn!("no FFmpeg signature; encoder capabilities not cached"),
    }
    found
}

/// Parses one engine listing (`{"schema_version":1,"microphones":[...]}`,
/// the key is `microphones` for both) into `(display name, endpoint id)`.
pub fn parse_listing(text: &str) -> Result<DeviceList, String> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|e| e.to_string())?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(error.to_owned());
    }
    let items = value
        .get("microphones")
        .and_then(Value::as_array)
        .ok_or_else(|| "the device listing has no entries".to_owned())?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let id = item.get("endpoint_id")?.as_str()?.to_owned();
            if id.is_empty() {
                return None;
            }
            let name = item
                .get("display_name")
                .and_then(Value::as_str)
                .filter(|n| !n.trim().is_empty())
                .unwrap_or(&id)
                .to_owned();
            Some((name, id))
        })
        .collect())
}

/// Runs `engine FLAG` and parses its listing.
pub async fn list(engine: &Path, flag: &str) -> Result<DeviceList, String> {
    let run = tokio::process::Command::new(engine)
        .arg(flag)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(LISTING_TIMEOUT, run)
        .await
        .map_err(|_| format!("{flag} timed out"))?
        .map_err(|e| format!("{}: {e}", engine.display()))?;
    parse_listing(&String::from_utf8_lossy(&output.stdout))
}

/// Opens the folder holding `path` in the file manager with the file
/// selected (the OpenURI portal's `OpenDirectory`); without the portal,
/// opens the folder with `xdg-open`.
pub async fn reveal(path: &Path) {
    match reveal_with_portal(path).await {
        Ok(()) => {}
        Err(e) => {
            tracing::info!(error = %e, "OpenDirectory portal unavailable; opening the folder");
            let folder = path.parent().unwrap_or(path);
            crate::shell::open_url(&folder.display().to_string());
        }
    }
}

async fn reveal_with_portal(path: &Path) -> Result<(), String> {
    use ashpd::desktop::open_uri::OpenDirectoryRequest;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    OpenDirectoryRequest::default()
        .send(&file)
        .await
        .map_err(|e| e.to_string())?
        .response()
        .map_err(|e| e.to_string())
}
