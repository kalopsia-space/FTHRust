//! Unix-socket server (engine side) and client (UI side) over the framing.

use std::path::{Path, PathBuf};

use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};

use crate::framing::{FrameReader, FrameWriter};
use crate::{Command, Event, IpcError, PROTOCOL_VERSION};

/// Channel depth for commands and events.
const QUEUE: usize = 64;

/// One accepted UI connection.
pub struct Connection {
    /// Commands decoded from the client, in order; closes when it hangs up.
    pub commands: mpsc::Receiver<Command>,
    /// Events to encode to the client.
    pub events: mpsc::Sender<Event>,
    /// Flips to `true` when either direction has ended.
    pub closed: watch::Receiver<bool>,
}

/// The engine's listening socket. Dropping it removes the socket file.
pub struct EngineServer {
    listener: UnixListener,
    path: PathBuf,
}

impl EngineServer {
    /// Binds `path` (parent directory created with mode 0700, socket mode
    /// 0600). A leftover socket file nobody listens on is replaced; a live
    /// one is an error.
    pub async fn bind(path: &Path) -> Result<EngineServer, IpcError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
        if path.exists() {
            match UnixStream::connect(path).await {
                Ok(_) => {
                    return Err(IpcError::Io(std::io::Error::new(
                        std::io::ErrorKind::AddrInUse,
                        format!("another engine is listening on {}", path.display()),
                    )));
                }
                Err(_) => std::fs::remove_file(path)?,
            }
        }
        let listener = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(EngineServer {
            listener,
            path: path.to_path_buf(),
        })
    }

    /// The socket path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Waits for the next client.
    pub async fn accept(&self) -> Result<Connection, IpcError> {
        let (stream, _) = self.listener.accept().await?;
        Ok(spawn_connection(stream))
    }
}

impl Drop for EngineServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

use std::os::unix::fs::PermissionsExt;

fn spawn_connection(stream: UnixStream) -> Connection {
    let (read_half, write_half) = stream.into_split();
    let (command_tx, command_rx) = mpsc::channel(QUEUE);
    let (event_tx, event_rx) = mpsc::channel(QUEUE);
    let (closed_tx, closed_rx) = watch::channel(false);
    let closed_for_reader = closed_tx.clone();
    tokio::spawn(read_commands(read_half, command_tx, closed_for_reader));
    tokio::spawn(write_events(write_half, event_rx, closed_tx));
    Connection {
        commands: command_rx,
        events: event_tx,
        closed: closed_rx,
    }
}

async fn read_commands(
    read_half: OwnedReadHalf,
    tx: mpsc::Sender<Command>,
    closed: watch::Sender<bool>,
) {
    let mut reader = FrameReader::new(read_half);
    loop {
        match reader.read::<Command>().await {
            Ok(Some(cmd)) => {
                if tx.send(cmd).await.is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                tracing::debug!("ipc read ended: {e}");
                break;
            }
        }
    }
    let _ = closed.send(true);
}

async fn write_events(
    write_half: OwnedWriteHalf,
    mut rx: mpsc::Receiver<Event>,
    closed: watch::Sender<bool>,
) {
    let mut writer = FrameWriter::new(write_half);
    while let Some(event) = rx.recv().await {
        if let Err(e) = writer.write(&event).await {
            tracing::debug!("ipc write ended: {e}");
            break;
        }
    }
    let _ = closed.send(true);
}

/// The `Hello` an engine sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    /// The engine's pid.
    pub pid: u32,
    /// The engine's crate version.
    pub version: String,
}

/// A connection to an engine.
pub struct EngineClient {
    commands: mpsc::Sender<Command>,
    events: mpsc::Receiver<Event>,
    hello: Hello,
}

impl EngineClient {
    /// Connects and reads the `Hello`, checking the protocol version.
    pub async fn connect(path: &Path) -> Result<EngineClient, IpcError> {
        let mut client = Self::connect_raw(path).await?;
        match client.events.recv().await {
            Some(Event::Hello {
                protocol_version,
                pid,
                version,
            }) => {
                if protocol_version != PROTOCOL_VERSION {
                    return Err(IpcError::ProtocolMismatch {
                        expected: PROTOCOL_VERSION,
                        got: protocol_version,
                    });
                }
                client.hello = Hello { pid, version };
                Ok(client)
            }
            Some(other) => Err(IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("expected Hello, got {other:?}"),
            ))),
            None => Err(IpcError::Truncated),
        }
    }

    /// Connects without waiting for the `Hello` (tests and probes).
    pub async fn connect_raw(path: &Path) -> Result<EngineClient, IpcError> {
        let stream = UnixStream::connect(path).await?;
        let (read_half, write_half) = stream.into_split();
        let (command_tx, mut command_rx) = mpsc::channel::<Command>(QUEUE);
        let (event_tx, event_rx) = mpsc::channel::<Event>(QUEUE);
        tokio::spawn(async move {
            let mut writer = FrameWriter::new(write_half);
            while let Some(cmd) = command_rx.recv().await {
                if writer.write(&cmd).await.is_err() {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            let mut reader = FrameReader::new(read_half);
            while let Ok(Some(event)) = reader.read::<Event>().await {
                if event_tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        Ok(EngineClient {
            commands: command_tx,
            events: event_rx,
            hello: Hello {
                pid: 0,
                version: String::new(),
            },
        })
    }

    /// What the engine announced.
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// Queues a command.
    pub async fn send(&self, command: Command) -> Result<(), IpcError> {
        self.commands.send(command).await.map_err(|_| {
            IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "engine connection closed",
            ))
        })
    }

    /// Events from the engine, in order; ends when the engine hangs up.
    pub fn events(&mut self) -> &mut mpsc::Receiver<Event> {
        &mut self.events
    }
}
