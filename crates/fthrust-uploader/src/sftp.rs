//! SFTP uploads (`sftp_provider.py`): password login only, host keys
//! checked against a `SHA256:` pin or the known_hosts files before any
//! credential is sent, uploads written as `.part`, size-checked and renamed.

use std::borrow::Cow;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fthrust_core::upload::validate::{self, SftpConfig};
use hmac::{KeyInit, Mac};
use russh::client;
use russh::keys::{Algorithm, EcdsaCurve, HashAlg, PublicKey, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use serde_json::{Map, Value, json};
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::Env;
use crate::http::Failure;
use crate::vault;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// How long removing the leftovers of a failed upload may take in all.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one SFTP request may go unanswered.
const SFTP_TIMEOUT_S: u64 = 20;
/// The same while the clip is going out (see [`WRITE_WINDOW`]).
const TRANSFER_TIMEOUT_S: u64 = 120;

/// Write requests in flight at once (each carries 32 KiB). On a link with a
/// long round trip the window decides the speed: window ÷ round trip. The
/// library's 16 (512 KiB) held a 60 ms link to 6.4 MB/s; 64 (2 MiB) is what
/// OpenSSH's `sftp` uses, and the SSH channel window of an OpenSSH server
/// (2 MiB) lets no more through anyway (128 measured the same).
const WRITE_WINDOW: usize = 64;
const TRANSFER: &str =
    "SFTP connection or transfer failed. Check the server, folder and permissions.";
const AUTH_FAILED: &str = "SFTP authentication failed. Check the username and password.";
const PIN_MISMATCH: &str = "SFTP host-key mismatch. Connection stopped before sending credentials.";
const KEY_CHANGED: &str = "SFTP host-key mismatch. Connection stopped before sending credentials. Verify the changed key with the server operator.";
const UNTRUSTED: &str = "This SFTP host is not trusted yet. Enter its SHA256 fingerprint from the server operator and test the connection once, or add its verified key to your SSH known_hosts file.";
const BAD_KNOWN_HOSTS: &str = "Could not read trusted SSH host keys. Check your known_hosts files.";
const TESTED: &str = "SFTP authenticated; host key verified and remote folder found. Write access is checked on upload.";

/// Host-key algorithms offered: the defaults without SHA-1 `ssh-rsa`.
const HOST_KEYS: &[Algorithm] = &[
    Algorithm::Ed25519,
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP256,
    },
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP384,
    },
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP521,
    },
    Algorithm::Rsa {
        hash: Some(HashAlg::Sha512),
    },
    Algorithm::Rsa {
        hash: Some(HashAlg::Sha256),
    },
];

/// How a host key compares with the known_hosts files.
#[derive(Debug, PartialEq, Eq)]
enum Trust {
    Known,
    Changed,
    Unknown,
}

fn host_name(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    }
}

fn host_matches(name: &str, field: &str) -> bool {
    field.split(',').any(|entry| {
        if let Some(rest) = entry.strip_prefix("|1|") {
            let engine = base64::engine::general_purpose::STANDARD;
            use base64::Engine;
            let mut parts = rest.split('|');
            let (Some(salt), Some(hash)) = (parts.next(), parts.next()) else {
                return false;
            };
            let (Ok(salt), Ok(hash)) = (engine.decode(salt), engine.decode(hash)) else {
                return false;
            };
            let Ok(mut mac) = hmac::Hmac::<sha1::Sha1>::new_from_slice(&salt) else {
                return false;
            };
            mac.update(name.as_bytes());
            mac.verify_slice(&hash).is_ok()
        } else {
            entry.eq_ignore_ascii_case(name)
        }
    })
}

/// The lines of a file read as bytes. known_hosts is text by convention
/// only: a comment in Latin-1 or a stray byte must not make the whole file
/// unreadable.
fn byte_lines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// The keys the known_hosts files record for `name`: missing files are
/// skipped, unparseable lines (and lines that are not UTF-8, which cannot
/// name a host or hold a key) ignored.
fn recorded_keys(files: &[PathBuf], name: &str) -> Result<Vec<PublicKey>, String> {
    let mut keys = Vec::new();
    for file in files {
        let bytes = match std::fs::read(file) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(BAD_KNOWN_HOSTS.into()),
        };
        for line in byte_lines(&bytes) {
            let Ok(line) = std::str::from_utf8(line) else {
                continue;
            };
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('@') {
                continue;
            }
            let mut f = line.split_whitespace();
            let (Some(hosts), Some(kind), Some(data)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            if !host_matches(name, hosts) {
                continue;
            }
            if let Ok(recorded) = PublicKey::from_openssh(&format!("{kind} {data}")) {
                keys.push(recorded);
            }
        }
    }
    Ok(keys)
}

/// Compares the server key with the known_hosts files.
fn trust(files: &[PathBuf], name: &str, key: &PublicKey) -> Result<Trust, String> {
    let mut changed = false;
    for recorded in recorded_keys(files, name)? {
        if recorded.algorithm() != key.algorithm() {
            continue;
        }
        if recorded.key_data() == key.key_data() {
            return Ok(Trust::Known);
        }
        changed = true;
    }
    Ok(if changed {
        Trust::Changed
    } else {
        Trust::Unknown
    })
}

/// [`HOST_KEYS`] in the order to offer them for a host whose recorded keys
/// have the `known` algorithms: those first, as OpenSSH does. A server with
/// several host keys presents the first one on our list that it has, and a
/// key of another type than the one recorded reads as an untrusted host.
fn host_key_order(known: &[Algorithm]) -> Vec<Algorithm> {
    // Every RSA signature variant stands for the one `ssh-rsa` key type.
    let same = |a: &Algorithm, k: &Algorithm| match (a, k) {
        (Algorithm::Rsa { .. }, Algorithm::Rsa { .. }) => true,
        (a, k) => a == k,
    };
    let (recorded, others): (Vec<_>, Vec<_>) = HOST_KEYS
        .iter()
        .cloned()
        .partition(|a| known.iter().any(|k| same(a, k)));
    recorded.into_iter().chain(others).collect()
}

/// `remember_verified_host`: records a pinned key in the uploader's own
/// known_hosts, replacing an older key of the same type for that host.
fn remember(env: &Env, name: &str, key: &PublicKey) -> Result<(), String> {
    let path = env.paths.uploader_known_hosts_file();
    let openssh = key.to_openssh().map_err(|e| e.to_string())?;
    let kind = key.algorithm().as_str().to_owned();
    // Only a missing file starts a new one; a file that cannot be read is
    // not ours to overwrite. Lines are kept as the bytes they are.
    let existing = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.to_string()),
    };
    let replaced = |line: &[u8]| {
        let Ok(text) = std::str::from_utf8(line) else {
            return false;
        };
        let mut f = text.split_whitespace();
        f.next() == Some(name) && f.next() == Some(kind.as_str())
    };
    let mut out = Vec::new();
    for line in byte_lines(&existing).into_iter().filter(|l| !replaced(l)) {
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out.extend_from_slice(format!("{name} {openssh}\n").as_bytes());
    // The uploader's folder holds credentials: create it private (0700),
    // as the settings store does, if this runs first.
    let dir = env.paths.ensure_uploader_dir().map_err(|e| e.to_string())?;
    let tmp = dir.join(".known_hosts.tmp");
    std::fs::write(&tmp, out).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn constant_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

#[derive(Default)]
struct Seen {
    key: Option<PublicKey>,
    refusal: Option<String>,
}

struct Checker {
    pin: Option<String>,
    files: Vec<PathBuf>,
    name: String,
    seen: Arc<Mutex<Seen>>,
}

impl client::Handler for Checker {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        offered: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let PublicKeyOrCertificate::PublicKey { key, .. } = offered else {
            seen.refusal = Some(UNTRUSTED.into());
            return Ok(false);
        };
        seen.key = Some(key.clone());
        let verdict = match &self.pin {
            Some(pin) => {
                let actual = key.fingerprint(HashAlg::Sha256).to_string();
                if constant_eq(&actual, pin) {
                    Ok(())
                } else {
                    Err(PIN_MISMATCH.to_owned())
                }
            }
            None => match trust(&self.files, &self.name, key) {
                Ok(Trust::Known) => Ok(()),
                Ok(Trust::Changed) => Err(KEY_CHANGED.to_owned()),
                Ok(Trust::Unknown) => Err(UNTRUSTED.to_owned()),
                Err(e) => Err(e),
            },
        };
        match verdict {
            Ok(()) => Ok(true),
            Err(m) => {
                seen.refusal = Some(m);
                Ok(false)
            }
        }
    }
}

/// An authenticated SFTP session with the folder checked.
struct Connection {
    handle: client::Handle<Checker>,
    sftp: SftpSession,
    folder: String,
}

impl Connection {
    async fn close(self) {
        let _ = self.sftp.close().await;
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }
}

fn password(env: &Env, cfg: &SftpConfig, settings: &Map<String, Value>) -> Result<String, String> {
    if let Some(Value::String(p)) = settings.get("sftp_password")
        && !p.is_empty()
    {
        return Ok(p.clone());
    }
    let service = vault::service_name(&cfg.host, cfg.port, &cfg.username);
    match env.vault.get(&service, &cfg.username)? {
        Some(p) if !p.is_empty() => Ok(p),
        _ => Err("Enter and save an SFTP password first.".into()),
    }
}

/// The SFTP client's settings: the library's, with the wider write window.
fn sftp_config() -> russh_sftp::client::Config {
    russh_sftp::client::Config {
        max_concurrent_writes: WRITE_WINDOW,
        ..Default::default()
    }
}

async fn connect(
    env: &Env,
    cfg: &SftpConfig,
    password: &str,
    is_test: bool,
) -> Result<Connection, String> {
    let name = host_name(&cfg.host, cfg.port);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let files = vec![
        env.paths.home().join(".ssh").join("known_hosts"),
        env.paths.uploader_known_hosts_file(),
    ];
    // Ask for the key types we have on record for this host first. (An
    // unreadable file is reported by the key check, before any credential.)
    // Not with a pinned fingerprint: it names one key and known_hosts is not
    // consulted for it, so an old line of another type in there (ECDSA, RSA)
    // must not make the server present that key and fail the pin every time.
    let known: Vec<Algorithm> = if cfg.fingerprint.is_some() {
        Vec::new()
    } else {
        recorded_keys(&files, &name)
            .unwrap_or_default()
            .iter()
            .map(PublicKey::algorithm)
            .collect()
    };
    let checker = Checker {
        pin: cfg.fingerprint.clone(),
        files,
        name: name.clone(),
        seen: seen.clone(),
    };
    let config = client::Config {
        preferred: russh::Preferred {
            key: Cow::Owned(host_key_order(&known)),
            ..Default::default()
        },
        inactivity_timeout: Some(Duration::from_secs(60)),
        ..Default::default()
    };
    let refused = || {
        seen.lock()
            .unwrap_or_else(|e| e.into_inner())
            .refusal
            .clone()
            .unwrap_or_else(|| TRANSFER.to_owned())
    };
    let mut handle = match tokio::time::timeout(
        CONNECT_TIMEOUT,
        client::connect(Arc::new(config), (cfg.host.as_str(), cfg.port), checker),
    )
    .await
    {
        Ok(Ok(h)) => h,
        _ => return Err(refused()),
    };
    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        handle.authenticate_password(cfg.username.clone(), password),
    )
    .await
    {
        Ok(Ok(r)) if r.success() => {}
        Ok(Ok(_)) => return Err(AUTH_FAILED.into()),
        _ => return Err(TRANSFER.into()),
    }
    let sftp = async {
        let channel = handle.channel_open_session().await.ok()?;
        channel.request_subsystem(true, "sftp").await.ok()?;
        let sftp = SftpSession::new_with_config(channel.into_stream(), sftp_config())
            .await
            .ok()?;
        sftp.set_timeout(SFTP_TIMEOUT_S);
        Some(sftp)
    };
    let sftp = match tokio::time::timeout(CONNECT_TIMEOUT, sftp).await {
        Ok(Some(s)) => s,
        _ => return Err(TRANSFER.into()),
    };
    let folder = sftp
        .canonicalize(cfg.folder.clone())
        .await
        .map_err(|_| TRANSFER.to_owned())?;
    let attrs = sftp
        .metadata(folder.clone())
        .await
        .map_err(|_| TRANSFER.to_owned())?;
    if !attrs.is_dir() {
        return Err("The configured remote folder is not a directory.".into());
    }
    if cfg.fingerprint.is_some() {
        let key = seen.lock().unwrap_or_else(|e| e.into_inner()).key.clone();
        if let Some(key) = key
            && remember(env, &name, &key).is_err()
            && is_test
        {
            return Err("Host verified, but its trusted key could not be saved.".into());
        }
    }
    Ok(Connection {
        handle,
        sftp,
        folder,
    })
}

/// Test connection: authenticate, verify the host key and the folder.
pub async fn test(env: &Env, settings: &Map<String, Value>) -> Result<(bool, String), String> {
    let cfg = validate::sftp_config(settings)?;
    let pass = password(env, &cfg, settings)?;
    let conn = connect(env, &cfg, &pass, true).await?;
    conn.close().await;
    Ok((true, TESTED.into()))
}

/// `quote(s, safe=…)`: percent-encodes all but unreserved characters and
/// those in `safe`.
fn quote(s: &str, safe: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || "_.-~".contains(c) || safe.contains(c) {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn remote_name(path: &Path) -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let suffix = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .filter(|e| {
            (1..=10).contains(&e.len())
                && e.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
        .map(|e| format!(".{e}"))
        .unwrap_or_else(|| ".clip".into());
    format!("fthr-{hex}{suffix}")
}

/// The name an upload of `path` is written under until it is whole. It only
/// depends on the clip, so an attempt that died with no chance to clean up
/// (the app ends a stuck uploader with SIGKILL) is cleaned up by the next
/// attempt at the same clip.
fn part_name(path: &Path) -> String {
    let digest = sha2::Sha256::digest(path.as_os_str().as_bytes());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("fthr-{hex}.part")
}

/// [`part_name`] with a random tail: for when a leftover of the fixed name
/// is there and cannot be removed. (The partial file is created exclusively,
/// so that leftover would make every upload of the clip fail, for good.)
fn spare_part_name(path: &Path) -> String {
    let mut bytes = [0u8; 4];
    let _ = getrandom::fill(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let fixed = part_name(path);
    format!("{}-{hex}.part", fixed.trim_end_matches(".part"))
}

/// `name` in the remote `folder`.
fn in_folder(folder: &str, name: &str) -> String {
    if folder.ends_with('/') {
        format!("{folder}{name}")
    } else {
        format!("{folder}/{name}")
    }
}

async fn send(
    conn: &Connection,
    path: &Path,
    part: &str,
    remote: &str,
    public: bool,
) -> Result<(), String> {
    let transfer = |_| TRANSFER.to_owned();
    let mut file = conn
        .sftp
        .open_with_flags(
            part,
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
        )
        .await
        .map_err(transfer)?;
    let private = FileAttributes {
        permissions: Some(0o600),
        ..FileAttributes::empty()
    };
    conn.sftp
        .set_metadata(part, private)
        .await
        .map_err(transfer)?;
    let mut local = tokio::fs::File::open(path)
        .await
        .map_err(|_| TRANSFER.to_owned())?;
    let size = local
        .metadata()
        .await
        .map_err(|_| TRANSFER.to_owned())?
        .len();
    // With a full window a write is answered only after the ones ahead of it
    // have gone out, which on a slow link takes longer than a lone request.
    conn.sftp.set_timeout(TRANSFER_TIMEOUT_S);
    let copied = async {
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = local.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).await?;
        }
        file.shutdown().await
    }
    .await;
    conn.sftp.set_timeout(SFTP_TIMEOUT_S);
    copied.map_err(|_| TRANSFER.to_owned())?;
    let written = conn.sftp.metadata(part).await.map_err(transfer)?.size;
    if written != Some(size) {
        return Err("Remote file size verification failed; the local clip was retained.".into());
    }
    conn.sftp.rename(part, remote).await.map_err(transfer)?;
    let written = conn.sftp.metadata(remote).await.map_err(transfer)?.size;
    if written != Some(size) {
        return Err("Final remote size verification failed; the local clip was retained.".into());
    }
    if public {
        let readable = FileAttributes {
            permissions: Some(0o644),
            ..FileAttributes::empty()
        };
        conn.sftp
            .set_metadata(remote, readable)
            .await
            .map_err(transfer)?;
    }
    Ok(())
}

/// Uploads `path` (one attempt; SFTP never retries).
pub async fn upload(
    env: &Env,
    settings: &Map<String, Value>,
    path: &Path,
) -> Result<Map<String, Value>, Failure> {
    let cfg = validate::sftp_config(settings).map_err(Failure::Permanent)?;
    let base = validate::public_base_url(settings).map_err(Failure::Permanent)?;
    let pass = password(env, &cfg, settings).map_err(Failure::Permanent)?;
    let conn = connect(env, &cfg, &pass, false)
        .await
        .map_err(Failure::Permanent)?;
    let name = remote_name(path);
    let remote = in_folder(&conn.folder, &name);
    let mut part = in_folder(&conn.folder, &part_name(path));
    // What an earlier attempt at this clip left behind if it was killed.
    let _ = conn.sftp.remove_file(part.clone()).await;
    if conn.sftp.metadata(part.clone()).await.is_ok() {
        // Still there: the server will not let it go. Do not fail the clip
        // for as long as that lasts; write under a name of our own.
        part = in_folder(&conn.folder, &spare_part_name(path));
    }
    let sent = send(&conn, path, &part, &remote, !base.is_empty()).await;
    if sent.is_err() {
        // A failed upload leaves nothing on the server: not the partial
        // file, and not a finished one that then failed its checks. (On a
        // dead connection these wait for nothing; do not wait long.)
        let _ = tokio::time::timeout(CLEANUP_TIMEOUT, async {
            let _ = conn.sftp.remove_file(part.clone()).await;
            let _ = conn.sftp.remove_file(remote.clone()).await;
        })
        .await;
    }
    conn.close().await;
    sent.map_err(Failure::Permanent)?;
    let authority = if cfg.host.contains(':') {
        format!("[{}]", cfg.host)
    } else {
        cfg.host.clone()
    };
    let sftp_url = format!("sftp://{authority}:{}{}", cfg.port, quote(&remote, "/"));
    let public_url = if base.is_empty() {
        String::new()
    } else {
        format!("{base}{}", quote(&name, ""))
    };
    let url = if public_url.is_empty() {
        sftp_url.clone()
    } else {
        public_url.clone()
    };
    let mut m = Map::new();
    m.insert("url".into(), json!(url));
    m.insert("raw_url".into(), json!(url));
    m.insert("sftp_url".into(), json!(sftp_url));
    m.insert("public_url".into(), json!(public_url));
    m.insert("remote_path".into(), json!(remote));
    m.insert("favorite".into(), json!(false));
    Ok(m)
}

/// `sftp_save`: validates and stores a supplied password.
pub fn save(env: &Env, config: &Map<String, Value>) -> Result<String, String> {
    let cfg = validate::sftp_config(config)?;
    if let Some(Value::String(p)) = config.get("sftp_password")
        && !p.is_empty()
    {
        let service = vault::service_name(&cfg.host, cfg.port, &cfg.username);
        env.vault.set(&service, &cfg.username, p)?;
    }
    Ok("SFTP credentials updated.".into())
}

/// `sftp_forget`: validates and removes the saved password.
pub fn forget(env: &Env, config: &Map<String, Value>) -> Result<String, String> {
    let cfg = validate::sftp_config(config)?;
    let service = vault::service_name(&cfg.host, cfg.port, &cfg.username);
    env.vault.delete(&service, &cfg.username)?;
    Ok("SFTP credentials updated.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const P256: Algorithm = Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP256,
    };
    const P384: Algorithm = Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP384,
    };
    const RSA512: Algorithm = Algorithm::Rsa {
        hash: Some(HashAlg::Sha512),
    };
    const RSA256: Algorithm = Algorithm::Rsa {
        hash: Some(HashAlg::Sha256),
    };

    /// The speed over a link with a long round trip is the write window
    /// divided by the round trip (`throughput_over_a_slow_link` in
    /// `tests/sftp.rs` measures it): the library's 16 requests held a 60 ms
    /// link to 6.4 MB/s, 64 reach the 2 MiB window of an OpenSSH server.
    #[test]
    fn the_write_window_is_wide_enough_for_a_slow_link() {
        let cfg = sftp_config();
        assert!(cfg.max_concurrent_writes >= 64, "{cfg:?}");
        // A write may wait for the whole window to be sent: at the slowest
        // link this still serves (the 120 s of a stalled HTTP upload), the
        // window has to drain within the request timeout.
        let window = cfg.max_concurrent_writes as u64 * u64::from(cfg.max_write_packet_len);
        assert!(window / TRANSFER_TIMEOUT_S >= 10_000, "{window} bytes");
        const { assert!(TRANSFER_TIMEOUT_S > SFTP_TIMEOUT_S) };
    }

    #[test]
    fn host_key_types_on_record_are_offered_first() {
        // Nothing on record: the defaults, Ed25519 first.
        assert_eq!(host_key_order(&[]), HOST_KEYS);
        // An ECDSA key on record moves to the front; the rest keep their order.
        assert_eq!(
            host_key_order(&[P256]),
            [
                P256,
                Algorithm::Ed25519,
                P384,
                HOST_KEYS[3].clone(),
                RSA512,
                RSA256
            ]
        );
        // `ssh-rsa` stands for both RSA signature variants.
        let rsa = host_key_order(&[Algorithm::Rsa { hash: None }]);
        assert_eq!(rsa[..2], [RSA512, RSA256]);
        assert_eq!(rsa[2], Algorithm::Ed25519);
        // Several on record: all of them first, in the default order.
        let two = host_key_order(&[RSA512, P384]);
        assert_eq!(two[..3], [P384, RSA512, RSA256]);
        // Whatever is on record, the same algorithms are offered.
        for known in [vec![], vec![P256], vec![RSA512, P384, Algorithm::Ed25519]] {
            let mut offered: Vec<String> = host_key_order(&known)
                .iter()
                .map(|a| a.to_string())
                .collect();
            let mut all: Vec<String> = HOST_KEYS.iter().map(|a| a.to_string()).collect();
            offered.sort();
            all.sort();
            assert_eq!(offered, all);
        }
    }
}
