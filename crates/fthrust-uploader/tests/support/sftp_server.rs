//! A small SSH server with an SFTP subsystem over a local folder, for the
//! uploader's SFTP tests: password auth, a fixed ed25519 host key, and
//! switches to make rename or chmod fail.

use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use russh::keys::ssh_key;
use russh::server::{Auth, Msg, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};

/// Switches and counters shared with the test.
#[derive(Default)]
pub struct Knobs {
    pub auth_attempts: AtomicUsize,
    pub fail_rename: AtomicBool,
    pub fail_chmod_644: AtomicBool,
}

pub struct TestServer {
    pub port: u16,
    pub root: PathBuf,
    pub key: ssh_key::PrivateKey,
    pub knobs: Arc<Knobs>,
    _dir: tempfile::TempDir,
}

pub const USER: &str = "clipper";
pub const PASSWORD: &str = "s3cret pw";

impl TestServer {
    pub fn fingerprint(&self) -> String {
        self.key
            .public_key()
            .fingerprint(ssh_key::HashAlg::Sha256)
            .to_string()
    }

    /// `host port keytype base64` line for a known_hosts file.
    pub fn known_hosts_line(&self) -> String {
        format!(
            "[127.0.0.1]:{} {}",
            self.port,
            self.key.public_key().to_openssh().unwrap()
        )
    }
}

pub async fn start(seed: u8) -> TestServer {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("up")).unwrap();
    std::fs::write(root.join("afile"), b"x").unwrap();
    let pair = ssh_key::private::Ed25519Keypair::from_seed(&[seed; 32]);
    let key = ssh_key::PrivateKey::from(pair);
    let config = Arc::new(russh::server::Config {
        auth_rejection_time: std::time::Duration::from_millis(1),
        auth_rejection_time_initial: Some(std::time::Duration::ZERO),
        keys: vec![key.clone()],
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let knobs = Arc::new(Knobs::default());
    let (r, k) = (root.clone(), knobs.clone());
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            let handler = Ssh {
                root: r.clone(),
                knobs: k.clone(),
                channels: HashMap::new(),
            };
            let config = config.clone();
            tokio::spawn(async move {
                if let Ok(session) = russh::server::run_stream(config, socket, handler).await {
                    let _ = session.await;
                }
            });
        }
    });
    TestServer {
        port,
        root,
        key,
        knobs,
        _dir: dir,
    }
}

struct Ssh {
    root: PathBuf,
    knobs: Arc<Knobs>,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl russh::server::Handler for Ssh {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.knobs.auth_attempts.fetch_add(1, Ordering::SeqCst);
        if user == USER && password == PASSWORD {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.close(channel)?;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match (name, self.channels.remove(&id)) {
            ("sftp", Some(channel)) => {
                session.channel_success(id)?;
                let sftp = Sftp {
                    root: self.root.clone(),
                    knobs: self.knobs.clone(),
                    handles: HashMap::new(),
                    next: 0,
                };
                tokio::spawn(russh_sftp::server::run(channel.into_stream(), sftp));
            }
            _ => session.channel_failure(id)?,
        }
        Ok(())
    }
}

struct Sftp {
    root: PathBuf,
    knobs: Arc<Knobs>,
    handles: HashMap<String, std::fs::File>,
    next: u32,
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

fn code(e: std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

/// Client path ("/up/x", "up/x", ".") → server path and its normal form.
fn normal(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

impl Sftp {
    fn local(&self, path: &str) -> PathBuf {
        self.root.join(normal(path).trim_start_matches('/'))
    }
}

impl russh_sftp::server::Handler for Sftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _: u32, _: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![File::dummy(normal(&path))],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let meta = std::fs::metadata(self.local(&path)).map_err(code)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.stat(id, path).await
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let opts: std::fs::OpenOptions = pflags.into();
        let file = opts.open(self.local(&filename)).map_err(code)?;
        self.next += 1;
        let handle = format!("h{}", self.next);
        self.handles.insert(handle.clone(), file);
        Ok(Handle { id, handle })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let f = self.handles.get_mut(&handle).ok_or(StatusCode::Failure)?;
        f.seek(SeekFrom::Start(offset)).map_err(code)?;
        f.write_all(&data).map_err(code)?;
        Ok(ok(id))
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let f = self.handles.get(&handle).ok_or(StatusCode::Failure)?;
        let meta = f.metadata().map_err(code)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(ok(id))
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        if let Some(mode) = attrs.permissions {
            if mode & 0o777 == 0o644 && self.knobs.fail_chmod_644.load(Ordering::SeqCst) {
                return Err(StatusCode::PermissionDenied);
            }
            let p = self.local(&path);
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode & 0o7777))
                .map_err(code)?;
        }
        Ok(ok(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        if self.knobs.fail_rename.load(Ordering::SeqCst) {
            return Err(StatusCode::Failure);
        }
        std::fs::rename(self.local(&oldpath), self.local(&newpath)).map_err(code)?;
        Ok(ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        std::fs::remove_file(self.local(&filename)).map_err(code)?;
        Ok(ok(id))
    }
}

/// Every file name in `dir`.
pub fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}
