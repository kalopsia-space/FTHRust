//! Audio-source sidecars (`<media>.fthr-audio.json`) bound to the media hash,
//! and clip renames that keep the sidecar attached.

use crate::clip_files::audio_manifest_sidecar;
use crate::error::{CoreError, Result};
use crate::transactional_output::StagedOutput;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Only manifest version understood by this crate.
pub const MANIFEST_VERSION: u32 = 1;
/// Maximum number of audio sources in one manifest.
pub const MAX_SOURCES: usize = 10;

static SAFE_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9 ._+()'\-]{1,128}$").expect("static regex"));
static RESERVED_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(\..*)?$").expect("static regex")
});

/// Kind of audio source captured into a stream.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    /// A single application's audio.
    Application,
    /// A microphone input.
    Microphone,
    /// The system mix.
    System,
}

/// One audio stream described by the manifest.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct AudioSource {
    /// Clip-local source UUID.
    pub source_uuid: String,
    /// Audio stream index in the media (>= 1, unique).
    pub stream_index: u32,
    /// Kind of source.
    pub source_type: SourceType,
    /// Human-readable, manifest-safe label.
    pub display_name: String,
    /// Stable, manifest-safe identity (never a path).
    pub persistent_identity: String,
    /// Optional manifest-safe icon reference.
    pub icon_reference: Option<String>,
    /// First active time in 100 ns units.
    pub first_active_100ns: i64,
    /// Last active time in 100 ns units.
    pub last_active_100ns: i64,
    /// Sample rate in Hz (8000..=192000).
    pub sample_rate: u32,
    /// Channel count (1..=8).
    pub channels: u8,
    /// Sample format label.
    pub sample_format: String,
}

/// Sidecar document binding audio sources to one exact media file.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct AudioManifest {
    /// Schema version (must be 1).
    pub manifest_version: u32,
    /// Recording transaction UUID.
    pub transaction_id: String,
    /// Media file base name this manifest belongs to.
    pub media_file: String,
    /// Lowercase hex SHA-256 of the media file.
    pub media_sha256: String,
    /// Described sources (1..=10).
    pub sources: Vec<AudioSource>,
}

/// Path of the sidecar for `media` (`<media>.fthr-audio.json`).
pub fn manifest_path_for(media: &Path) -> PathBuf {
    audio_manifest_sidecar(media)
}

/// Lowercase hex SHA-256 of a file's contents.
pub fn sha256_file(p: &Path) -> Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn safe(field: &str, v: &str) -> std::result::Result<(), String> {
    if SAFE_TEXT.is_match(v) {
        Ok(())
    } else {
        Err(format!("{field} is not manifest-safe"))
    }
}

fn is_uuid(v: &str) -> bool {
    uuid::Uuid::parse_str(v).is_ok()
}

impl AudioManifest {
    /// Check every field against the manifest contract (no media access).
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.manifest_version != MANIFEST_VERSION {
            return Err("unsupported manifest version".into());
        }
        if !is_uuid(&self.transaction_id) {
            return Err("transaction_id must be a UUID string".into());
        }
        if self.media_file.is_empty()
            || self.media_file.contains(['/', '\\'])
            || self.media_file == "."
            || self.media_file == ".."
        {
            return Err("media_file must be a basename".into());
        }
        let h = &self.media_sha256;
        if h.len() != 64 || !h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err("media_sha256 must be a SHA-256 digest".into());
        }
        if self.sources.is_empty() || self.sources.len() > MAX_SOURCES {
            return Err("sources must contain between one and ten entries".into());
        }
        let mut uuids = BTreeSet::new();
        let mut streams = BTreeSet::new();
        for s in &self.sources {
            if !is_uuid(&s.source_uuid) {
                return Err("source_uuid must be a UUID string".into());
            }
            let canon = s.source_uuid.to_ascii_lowercase();
            if !uuids.insert(canon) {
                return Err("source UUID appears more than once".into());
            }
            if s.stream_index < 1 {
                return Err("stream_index must be a positive audio stream index".into());
            }
            if !streams.insert(s.stream_index) {
                return Err("stream index appears more than once".into());
            }
            safe("display_name", &s.display_name)?;
            safe("persistent_identity", &s.persistent_identity)?;
            safe("sample_format", &s.sample_format)?;
            if let Some(i) = &s.icon_reference {
                safe("icon_reference", i)?;
            }
            if s.first_active_100ns < 0 || s.last_active_100ns < 0 {
                return Err("active times must be non-negative".into());
            }
            if s.last_active_100ns < s.first_active_100ns {
                return Err("source active interval is inverted".into());
            }
            if !(8000..=192_000).contains(&s.sample_rate) {
                return Err("sample_rate is out of range".into());
            }
            if !(1..=8).contains(&s.channels) {
                return Err("channels is out of range".into());
            }
        }
        Ok(())
    }

    /// Validate, require the manifest to match `media` (name and hash), and
    /// publish the sidecar atomically as compact, key-sorted, ASCII JSON.
    pub fn write(&self, media: &Path) -> Result<()> {
        self.validate().map_err(CoreError::Invalid)?;
        if media.file_name().and_then(|n| n.to_str()) != Some(self.media_file.as_str()) {
            return Err(CoreError::Invalid(
                "manifest media name does not match clip".into(),
            ));
        }
        if sha256_file(media)? != self.media_sha256 {
            return Err(CoreError::Invalid(
                "manifest media hash does not match clip".into(),
            ));
        }
        self.write_unbound(media)
    }

    fn write_unbound(&self, media: &Path) -> Result<()> {
        let value = serde_json::to_value(self).map_err(|e| CoreError::Invalid(e.to_string()))?;
        write_canonical(&manifest_path_for(media), &value)
    }

    /// The validated manifest for `media`, or `None` when the sidecar is
    /// missing, unparsable, invalid, names another file, or the media hash
    /// does not match. `Err` only for I/O failures other than NotFound.
    pub fn read_verified(media: &Path) -> Result<Option<AudioManifest>> {
        read_verified(media)
    }
}

/// Free-function form of [`AudioManifest::read_verified`].
pub fn read_verified(media: &Path) -> Result<Option<AudioManifest>> {
    let text = match std::fs::read_to_string(manifest_path_for(media)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let Ok(m) = serde_json::from_str::<AudioManifest>(&text) else {
        return Ok(None);
    };
    if m.validate().is_err() || media.file_name().and_then(|n| n.to_str()) != Some(&m.media_file) {
        return Ok(None);
    }
    let hash = match sha256_file(media) {
        Ok(h) => h,
        Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok((hash == m.media_sha256).then_some(m))
}

/// Re-hash `media` and rewrite its sidecar after a stream-preserving rewrite.
/// A missing, invalid or foreign sidecar is left alone (no-op, `Ok`).
pub fn rebind_after_media_replace(media: &Path) -> Result<()> {
    let text = match std::fs::read_to_string(manifest_path_for(media)) {
        Ok(t) => t,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData
            ) =>
        {
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    let Ok(mut m) = serde_json::from_str::<AudioManifest>(&text) else {
        return Ok(());
    };
    if m.validate().is_err() || media.file_name().and_then(|n| n.to_str()) != Some(&m.media_file) {
        return Ok(());
    }
    m.media_sha256 = sha256_file(media)?;
    m.write_unbound(media)
}

/// Serialise like Python `json.dumps(sort_keys=True, separators=(',', ':'),
/// ensure_ascii=True)`.
fn canonical_json(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                ascii_string(k, out);
                out.push(':');
                canonical_json(&map[k.as_str()], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_json(x, out);
            }
            out.push(']');
        }
        Value::String(s) => ascii_string(s, out),
        other => out.push_str(&other.to_string()),
    }
}

fn ascii_string(s: &str, out: &mut String) {
    use std::fmt::Write;
    let escaped = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    for c in escaped.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                let _ = write!(out, "\\u{u:04x}");
            }
        }
    }
}

fn write_canonical(dest: &Path, value: &Value) -> Result<()> {
    let mut text = String::new();
    canonical_json(value, &mut text);
    let staged = StagedOutput::new(dest);
    if let Err(e) = std::fs::write(staged.staged_path(), text.as_bytes()) {
        staged.discard();
        return Err(e.into());
    }
    staged.commit().map(|_| ())
}

fn validate_stem(stem: &str, ext: &str) -> Result<()> {
    let bad = stem.is_empty()
        || stem == "."
        || stem == ".."
        || stem.ends_with(['.', ' '])
        || stem
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || RESERVED_NAME.is_match(stem);
    if bad {
        return Err(CoreError::Invalid(
            "Choose a valid filename without / \\ : * ? \" < > | or a reserved Windows name."
                .into(),
        ));
    }
    if stem.len() + ext.len() > 255 {
        return Err(CoreError::Invalid(
            "The filename is too long. Choose a shorter name.".into(),
        ));
    }
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

/// Rename a clip to `new_stem` (extension kept), moving its sidecar and
/// rewriting `media_file` inside it. Existing targets are never overwritten;
/// a failed sidecar step restores the original media name.
pub fn rename_clip(media: &Path, new_stem: &str) -> Result<PathBuf> {
    let stem = new_stem.trim();
    let ext = media
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    validate_stem(stem, &ext)?;
    let dest = media.with_file_name(format!("{stem}{ext}"));
    if dest.exists() && !same_file(media, &dest) {
        return Err(CoreError::Invalid(
            "A file with that name already exists. Choose another name.".into(),
        ));
    }
    let old_side = manifest_path_for(media);
    let new_side = manifest_path_for(&dest);
    // Read and parse the sidecar before touching the media so a corrupt
    // sidecar fails early.
    let side_value = match std::fs::read_to_string(&old_side) {
        Ok(t) => {
            let mut v: Value =
                serde_json::from_str(&t).map_err(|e| CoreError::Invalid(e.to_string()))?;
            let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("");
            v.as_object_mut()
                .ok_or_else(|| CoreError::Invalid("manifest must be an object".into()))?
                .insert("media_file".into(), Value::String(name.into()));
            Some(v)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if side_value.is_some() && new_side.exists() && !same_file(&old_side, &new_side) {
        return Err(CoreError::Invalid(
            "Audio metadata with that name already exists.".into(),
        ));
    }
    std::fs::rename(media, &dest)?;
    if let Some(v) = side_value {
        let step = write_canonical(&new_side, &v).and_then(|()| {
            if old_side != new_side {
                std::fs::remove_file(&old_side)?;
            }
            Ok(())
        });
        if let Err(e) = step {
            let _ = std::fs::rename(&dest, media);
            if old_side != new_side {
                let _ = std::fs::remove_file(&new_side);
            }
            return Err(e);
        }
    }
    Ok(dest)
}
