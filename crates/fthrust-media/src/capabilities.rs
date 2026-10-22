//! Which encoder backends actually work on this machine, proven by a
//! one-frame encode, and the cache that spares every app start the driver
//! initialisation. Ports `core/encoder_capabilities.py` (Linux table).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::encoder::encoder_works;

/// The argument a host binary recognises to run one encoder probe and exit
/// (see [`handle_probe_argv`]).
pub const PROBE_ARG: &str = "--fthrust-probe-encoder";

/// Per-encoder time limit for the child probe, as in the reference.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Cache schema version (the Linux value of the reference).
pub const CACHE_VERSION: u32 = 3;

/// A backend the settings UI can offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderCapability {
    /// Backend key (`nvenc`, `amf`, `qsv`, `software`).
    pub key: String,
    /// Display label.
    pub label: String,
    /// Codec keys that completed a real encode, in table order.
    pub codecs: Vec<String>,
    /// `(codec, encoder name)` for each working codec.
    pub codec_names: Vec<(String, String)>,
    /// Whether the backend honours the quality preset.
    pub supports_presets: bool,
}

impl EncoderCapability {
    /// The encoder name for `codec`, or `""`.
    pub fn encoder_name_for(&self, codec: &str) -> &str {
        self.codec_names
            .iter()
            .find(|(c, _)| c == codec)
            .map_or("", |(_, name)| name.as_str())
    }
}

/// One row of the backend table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendSpec {
    /// Backend key.
    pub key: &'static str,
    /// Display label.
    pub label: &'static str,
    /// Whether the backend honours the quality preset.
    pub supports_presets: bool,
    /// `(codec, candidate encoder names in order)`.
    pub codecs: Vec<(&'static str, Vec<&'static str>)>,
}

/// Backend keys in UI order.
pub fn backend_keys() -> [&'static str; 4] {
    ["nvenc", "amf", "qsv", "software"]
}

/// The Linux backend table: the AMD key probes VA-API first (it also
/// drives Intel GPUs here) with AMF as fallback, matching the engine's
/// candidate order.
pub fn backend_table() -> Vec<BackendSpec> {
    vec![
        BackendSpec {
            key: "nvenc",
            label: "NVIDIA NVENC",
            supports_presets: true,
            codecs: vec![
                ("h264", vec!["h264_nvenc"]),
                ("hevc", vec!["hevc_nvenc"]),
                ("av1", vec!["av1_nvenc"]),
            ],
        },
        BackendSpec {
            key: "amf",
            label: "VA-API (AMD / Intel)",
            supports_presets: false,
            codecs: vec![
                ("h264", vec!["h264_vaapi", "h264_amf"]),
                ("hevc", vec!["hevc_vaapi", "hevc_amf"]),
                ("av1", vec!["av1_vaapi", "av1_amf"]),
            ],
        },
        BackendSpec {
            key: "qsv",
            label: "Intel Quick Sync",
            supports_presets: false,
            codecs: vec![
                ("h264", vec!["h264_qsv"]),
                ("hevc", vec!["hevc_qsv"]),
                ("av1", vec!["av1_qsv"]),
            ],
        },
        BackendSpec {
            key: "software",
            label: "Software",
            supports_presets: false,
            codecs: vec![
                ("h264", vec!["libopenh264", "libx264"]),
                ("hevc", vec!["libkvazaar", "libx265"]),
                ("av1", vec!["libsvtav1", "libaom-av1", "librav1e"]),
            ],
        },
    ]
}

/// Discovers backends with an injected "does this encoder work" check.
pub fn probe_capabilities_with(
    mut works: impl FnMut(&'static str) -> bool,
) -> Vec<EncoderCapability> {
    let mut found = Vec::new();
    for spec in backend_table() {
        let mut codecs = Vec::new();
        let mut codec_names = Vec::new();
        for (codec, names) in &spec.codecs {
            if let Some(name) = names.iter().copied().find(|n| works(n)) {
                codecs.push((*codec).to_string());
                codec_names.push(((*codec).to_string(), name.to_string()));
            }
        }
        if !codecs.is_empty() {
            found.push(EncoderCapability {
                key: spec.key.to_string(),
                label: spec.label.to_string(),
                codecs,
                codec_names,
                supports_presets: spec.supports_presets,
            });
        }
    }
    found
}

/// Discovers backends by a real one-frame encode per candidate, inside
/// this process. A crashing or hanging driver takes the caller with it, so
/// prefer [`probe_capabilities`] from a UI.
pub fn probe_capabilities_in_process() -> Vec<EncoderCapability> {
    probe_capabilities_with(encoder_works)
}

/// Discovers backends with each one-frame encode run in a child process
/// (this executable re-invoked with [`PROBE_ARG`]), so a driver that aborts
/// or hangs only costs that candidate. The host binary must call
/// [`handle_probe_argv`] first thing in `main`. Call from a worker.
pub fn probe_capabilities() -> Vec<EncoderCapability> {
    match std::env::current_exe() {
        Ok(exe) => probe_capabilities_with(child_probe_runner(exe, PROBE_TIMEOUT)),
        Err(e) => {
            tracing::warn!(
                "cannot locate this executable for child probes, probing in-process: {e}"
            );
            probe_capabilities_in_process()
        }
    }
}

/// If this process was started as a probe child (`<exe> --fthrust-probe-encoder
/// NAME`), runs the probe and exits with status 0 (works) or 1. Returns
/// normally otherwise. Call before anything else in `main`.
pub fn handle_probe_argv() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some(PROBE_ARG) {
        return;
    }
    let name: &'static str = Box::leak(args.get(2).cloned().unwrap_or_default().into_boxed_str());
    std::process::exit(if encoder_works(name) { 0 } else { 1 });
}

/// A "does this encoder work" check that runs `exe --fthrust-probe-encoder
/// NAME` with no inherited stdio and treats a non-zero exit, a signal, a
/// spawn failure or a timeout (the child is killed) as "does not work".
pub fn child_probe_runner(exe: PathBuf, timeout: Duration) -> impl FnMut(&'static str) -> bool {
    move |name| {
        let mut child = match Command::new(&exe)
            .arg(PROBE_ARG)
            .arg(name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                tracing::debug!(encoder = name, "probe child could not start: {e}");
                return false;
            }
        };
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) => {
                    if started.elapsed() >= timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        tracing::debug!(encoder = name, "probe child timed out");
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    tracing::debug!(encoder = name, "probe child wait failed: {e}");
                    let _ = child.kill();
                    return false;
                }
            }
        }
    }
}

/// Codec keys in UI priority order for one selected backend (`auto` = any).
pub fn available_codecs(capabilities: &[EncoderCapability], encoder_key: &str) -> Vec<String> {
    let present: Vec<&str> = if encoder_key == "auto" {
        capabilities
            .iter()
            .flat_map(|c| c.codecs.iter().map(String::as_str))
            .collect()
    } else {
        capabilities
            .iter()
            .find(|c| c.key == encoder_key)
            .map(|c| c.codecs.iter().map(String::as_str).collect())
            .unwrap_or_default()
    };
    ["h264", "hevc", "av1"]
        .iter()
        .filter(|c| present.contains(c))
        .map(|c| (*c).to_string())
        .collect()
}

/// What the cache was computed against: the FFmpeg build in use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheSignature {
    /// `"linux"`.
    pub platform: String,
    /// The FFmpeg library (or executable) path the result belongs to.
    pub ffmpeg: String,
    /// Its size.
    pub ffmpeg_size: u64,
    /// Its modification time in nanoseconds.
    pub ffmpeg_mtime_ns: i64,
}

/// The libavcodec shared object this process loaded, if it is one.
fn linked_avcodec_path() -> Option<PathBuf> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    maps.lines()
        .filter_map(|line| line.split_whitespace().nth(5))
        .find(|p| p.contains("libavcodec.so"))
        .map(PathBuf::from)
}

impl CacheSignature {
    /// The signature of the FFmpeg this process links against (the shared
    /// libavcodec when present, else the executable it is built into).
    pub fn current() -> std::io::Result<Self> {
        let path = linked_avcodec_path()
            .map(Ok)
            .unwrap_or_else(std::env::current_exe)?;
        Self::for_path(&path)
    }

    /// The signature for an FFmpeg file at `path`.
    pub fn for_path(path: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let meta = std::fs::metadata(&resolved)?;
        Ok(Self {
            platform: "linux".into(),
            ffmpeg: resolved.to_string_lossy().into_owned(),
            ffmpeg_size: meta.len(),
            ffmpeg_mtime_ns: meta
                .mtime()
                .saturating_mul(1_000_000_000)
                .saturating_add(meta.mtime_nsec()),
        })
    }

    fn matches(&self, raw: &Map<String, Value>) -> bool {
        raw.get("version") == Some(&json!(CACHE_VERSION))
            && raw.get("platform") == Some(&json!(self.platform))
            && raw.get("ffmpeg") == Some(&json!(self.ffmpeg))
            && raw.get("ffmpeg_size") == Some(&json!(self.ffmpeg_size))
            && raw.get("ffmpeg_mtime_ns") == Some(&json!(self.ffmpeg_mtime_ns))
    }
}

fn serialize(item: &EncoderCapability) -> Value {
    json!({
        "key": item.key,
        "label": item.label,
        "codecs": item.codecs,
        "codec_names": item.codec_names.iter().map(|(c, n)| json!([c, n])).collect::<Vec<_>>(),
        "supports_presets": item.supports_presets,
    })
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn deserialize(raw: &Map<String, Value>) -> Option<EncoderCapability> {
    let key = text(raw.get("key")?);
    let label = text(raw.get("label")?);
    let codecs = raw
        .get("codecs")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(text).collect())
        .unwrap_or_default();
    let codec_names = raw
        .get("codec_names")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|pair| pair.as_array())
                .filter(|pair| pair.len() == 2)
                .map(|pair| (text(&pair[0]), text(&pair[1])))
                .collect()
        })
        .unwrap_or_default();
    Some(EncoderCapability {
        key,
        label,
        codecs,
        codec_names,
        supports_presets: raw
            .get("supports_presets")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Loads a cached discovery when it was made for the same FFmpeg build.
/// Missing, stale or malformed caches are a miss.
pub fn load_cached(cache_file: &Path, expected: &CacheSignature) -> Option<Vec<EncoderCapability>> {
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(cache_file).ok()?).ok()?;
    let raw = raw.as_object()?;
    if !expected.matches(raw) {
        return None;
    }
    let items = raw.get("capabilities")?.as_array()?;
    items
        .iter()
        .filter_map(Value::as_object)
        .map(deserialize)
        .collect()
}

/// Persists a discovery (temporary file, then rename).
pub fn store_cached(
    cache_file: &Path,
    signature: &CacheSignature,
    capabilities: &[EncoderCapability],
) -> std::io::Result<()> {
    let data = json!({
        "version": CACHE_VERSION,
        "platform": signature.platform,
        "ffmpeg": signature.ffmpeg,
        "ffmpeg_size": signature.ffmpeg_size,
        "ffmpeg_mtime_ns": signature.ffmpeg_mtime_ns,
        "capabilities": capabilities.iter().map(serialize).collect::<Vec<_>>(),
    });
    if let Some(parent) = cache_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut temporary = cache_file.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, serde_json::to_string_pretty(&data)?)?;
    std::fs::rename(&temporary, cache_file)
}
