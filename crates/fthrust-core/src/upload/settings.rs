//! `~/.fthr/uploader/settings.json` (`UploadManager._load_settings` and
//! `save_settings`).

use serde_json::{Map, Value, json};

use super::{CATBOX_LEGAL_VERSION, PRIVACY_VERSION, TERMS_VERSION, truthy};
use crate::FthrPaths;
use crate::settings::SettingsStore;

/// Upload destinations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// catbox.moe.
    Catbox,
    /// The user's own HTTP endpoint (`custom`, `own_server`, `your_server`).
    Custom,
    /// An SFTP server.
    Sftp,
}

impl Provider {
    /// Parses a stored provider id (case-insensitive, aliases included).
    pub fn parse(id: &str) -> Option<Provider> {
        match id.to_lowercase().as_str() {
            "catbox" => Some(Provider::Catbox),
            "custom" | "own_server" | "your_server" => Some(Provider::Custom),
            "sftp" => Some(Provider::Sftp),
            _ => None,
        }
    }

    /// The canonical id written to settings and history.
    pub fn id(self) -> &'static str {
        match self {
            Provider::Catbox => "catbox",
            Provider::Custom => "custom",
            Provider::Sftp => "sftp",
        }
    }
}

const KNOWN_PROVIDERS: [&str; 5] = ["catbox", "custom", "own_server", "your_server", "sftp"];
const LEGACY_CORE_KEYS: [&str; 7] = [
    "upload_enabled",
    "upload_server_url",
    "upload_auth_header",
    "upload_mode",
    "upload_interval_value",
    "upload_interval_unit",
    "upload_auto_delete",
];

/// The reference's `_DEFAULT_SETTINGS`.
pub fn defaults() -> Map<String, Value> {
    let v = json!({
        "upload_enabled": false,
        "upload_provider": "catbox",
        "catbox_userhash": "",
        "upload_server_url": "",
        "upload_auth_header": "",
        "sftp_host": "",
        "sftp_port": 22,
        "sftp_username": "",
        "sftp_directory": ".",
        "sftp_fingerprint": "",
        "sftp_public_enabled": false,
        "sftp_public_base_url": "",
        "upload_mode": "manual",
        "upload_interval_value": 5,
        "upload_interval_unit": "minutes",
        "upload_auto_compress": false,
        "upload_auto_delete": false,
        "uploader_terms_accepted_version": "",
        "uploader_privacy_accepted_version": "",
        "uploader_accepted_at": "",
        "catbox_legal_accepted_version": "",
        "catbox_legal_accepted_at": "",
    });
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

/// The uploader's settings: defaults merged with the file.
#[derive(Clone, Debug, PartialEq)]
pub struct UploaderSettings {
    data: Map<String, Value>,
}

impl Default for UploaderSettings {
    fn default() -> Self {
        Self { data: defaults() }
    }
}

impl UploaderSettings {
    /// Loads the file: defaults for anything missing, no `sftp_password` or
    /// `lustful_*` keys, a retired provider turns uploads off on Catbox, an
    /// unreadable file gives the defaults.
    pub fn load(paths: &FthrPaths) -> Self {
        let text = match std::fs::read_to_string(paths.uploader_settings_file()) {
            Ok(t) => t,
            Err(_) => return Self::default(),
        };
        let Ok(Value::Object(file)) = serde_json::from_str::<Value>(&text) else {
            return Self::default();
        };
        Self::from_map(file)
    }

    /// Applies the load rules to a raw settings object.
    pub fn from_map(file: Map<String, Value>) -> Self {
        let mut data = defaults();
        for (k, v) in file {
            data.insert(k, v);
        }
        data.remove("sftp_password");
        data.retain(|k, _| !k.starts_with("lustful_"));
        let known = data
            .get("upload_provider")
            .and_then(Value::as_str)
            .is_some_and(|p| KNOWN_PROVIDERS.contains(&p));
        if !known {
            data.insert("upload_enabled".into(), json!(false));
            data.insert("upload_provider".into(), json!("catbox"));
        }
        Self { data }
    }

    /// Writes the file atomically with sorted keys, never the password.
    pub fn save(&self, paths: &FthrPaths) -> crate::Result<()> {
        let path = paths.uploader_settings_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut keys: Vec<&String> = self.data.keys().collect();
        keys.sort();
        let sorted: Map<String, Value> = keys
            .into_iter()
            .filter(|k| k.as_str() != "sftp_password")
            .map(|k| (k.clone(), self.data[k].clone()))
            .collect();
        crate::write_json_atomic(&path, &Value::Object(sorted), true)
    }

    /// All values.
    pub fn data(&self) -> &Map<String, Value> {
        &self.data
    }

    /// One value.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.data.get(key)
    }

    /// A string value, `""` when missing or not a string.
    pub fn str(&self, key: &str) -> &str {
        self.data.get(key).and_then(Value::as_str).unwrap_or("")
    }

    /// A boolean value by Python truthiness.
    pub fn flag(&self, key: &str) -> bool {
        truthy(self.data.get(key))
    }

    /// Sets a value.
    pub fn set(&mut self, key: &str, value: Value) {
        self.data.insert(key.to_owned(), value);
    }

    /// The selected provider (unknown ids read as Catbox).
    pub fn provider(&self) -> Provider {
        Provider::parse(self.str("upload_provider")).unwrap_or(Provider::Catbox)
    }

    /// `upload_enabled`.
    pub fn enabled(&self) -> bool {
        self.flag("upload_enabled")
    }

    /// Both uploader documents accepted at their current versions.
    pub fn consent_current(&self) -> bool {
        self.str("uploader_terms_accepted_version") == TERMS_VERSION
            && self.str("uploader_privacy_accepted_version") == PRIVACY_VERSION
    }

    /// Catbox's policies accepted at the current version.
    pub fn catbox_consent_current(&self) -> bool {
        self.str("catbox_legal_accepted_version") == CATBOX_LEGAL_VERSION
    }

    /// Records acceptance of the Terms and Privacy Policy at `now`.
    pub fn accept_terms(&mut self, now: &str) {
        self.set("uploader_terms_accepted_version", json!(TERMS_VERSION));
        self.set("uploader_privacy_accepted_version", json!(PRIVACY_VERSION));
        self.set("uploader_accepted_at", json!(now));
    }

    /// Records acceptance of Catbox's policies at `now`.
    pub fn accept_catbox(&mut self, now: &str) {
        self.set("catbox_legal_accepted_version", json!(CATBOX_LEGAL_VERSION));
        self.set("catbox_legal_accepted_at", json!(now));
    }
}

/// `_migrate_legacy_state`: moves the pre-extension upload keys out of the
/// core settings. Returns whether anything moved (the caller saves both).
pub fn migrate_legacy_core_keys(core: &mut SettingsStore, up: &mut UploaderSettings) -> bool {
    let mut changed = false;
    for key in LEGACY_CORE_KEYS {
        if let Some(value) = core.remove(key) {
            up.set(key, value);
            changed = true;
        }
    }
    if changed {
        let provider = if truthy(up.get("upload_server_url")) {
            "custom"
        } else {
            "catbox"
        };
        up.set("upload_provider", json!(provider));
    }
    changed
}
