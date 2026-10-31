//! The ScreenCast portal session (ports the D-Bus flow of
//! `portal_screencast.cpp` onto ashpd): create session, select a monitor
//! with the cursor embedded, start (the user's dialog), persist the restore
//! token, open the PipeWire remote.
//!
//! Each session has its own D-Bus connection, which lives and dies with it;
//! the blocking entry points run on the process-wide portal runtime.

use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
    StartCastOptions,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, ResponseError, Session};
use ashpd::enumflags2::BitFlags;
use fthrust_core::FthrPaths;
use futures_util::StreamExt;
use tokio::sync::watch;

use crate::token;

/// A restored session whose stream does not negotiate is retried once
/// without its token; nothing else is retried automatically.
pub fn should_retry_startup(attempt: u32, had_token: bool) -> bool {
    attempt == 0 && had_token
}

/// How long closing a session may take.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// Time limits of the portal flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// The user's screen picker.
    pub dialog: Duration,
    /// Any other portal method.
    pub method: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            dialog: Duration::from_secs(120),
            method: Duration::from_secs(10),
        }
    }
}

/// Why the portal could not provide a stream.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PortalFailure {
    /// No portal, or one without ScreenCast.
    #[error("the ScreenCast portal is unavailable: {0}")]
    Unavailable(String),
    /// The user dismissed the picker, or it timed out: never retried.
    #[error("the screen picker was dismissed")]
    UserDeclined,
    /// The cancel flag was set.
    #[error("capture cancelled")]
    Cancelled,
    /// Anything else.
    #[error("{0}")]
    Failed(String),
}

/// An established ScreenCast session. Dropping it closes the session.
pub struct ScreenCastSession {
    session: Option<Arc<Session<Screencast>>>,
    proxy: Option<Screencast>,
    connection: Option<zbus::Connection>,
    watcher: Option<tokio::task::JoinHandle<()>>,
    fd: Option<OwnedFd>,
    node_id: u32,
    closed: watch::Receiver<bool>,
    restore_token: Option<String>,
    restored: bool,
}

async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn bounded<T>(
    step: &'static str,
    timeout: Duration,
    cancel: &AtomicBool,
    fut: impl std::future::Future<Output = Result<T, ashpd::Error>>,
) -> Result<T, PortalFailure> {
    tokio::select! {
        _ = cancelled(cancel) => Err(PortalFailure::Cancelled),
        r = tokio::time::timeout(timeout, fut) => match r {
            Err(_) if step == "Start" => Err(PortalFailure::UserDeclined),
            Err(_) => Err(PortalFailure::Failed(format!("{step}: the portal did not answer in time"))),
            Ok(Err(ashpd::Error::Response(ResponseError::Cancelled))) => Err(PortalFailure::UserDeclined),
            Ok(Err(e)) => Err(PortalFailure::Failed(format!("{step}: {e}"))),
            Ok(Ok(v)) => Ok(v),
        },
    }
}

fn response_failure(step: &'static str) -> impl Fn(ashpd::Error) -> PortalFailure {
    move |e| match e {
        ashpd::Error::Response(ResponseError::Cancelled) => PortalFailure::UserDeclined,
        other => PortalFailure::Failed(format!("{step}: {other}")),
    }
}

/// The saved token, if it can be used. A damaged file is removed; one that
/// cannot be read is left for the next start.
fn load_token(path: &Path) -> Option<String> {
    match token::load(path) {
        Ok(t) => t,
        Err(e @ token::TokenError::Damaged) => {
            tracing::warn!("{e}; starting without a restore token");
            let _ = token::save(path, "");
            None
        }
        Err(e) => {
            tracing::warn!("{e}; starting without a restore token");
            None
        }
    }
}

/// Persists the token `Start` returned: it replaces the one presented,
/// which the portal accepts only once. No token means the portal does not
/// persist this session, and the old one must not linger.
pub fn persist_token(path: &Path, presented: Option<&str>, returned: Option<&str>) {
    if returned == presented {
        return;
    }
    if let Err(e) = token::save(path, returned.unwrap_or("")) {
        tracing::warn!("could not save the portal restore token: {e}");
    }
}

impl ScreenCastSession {
    fn empty() -> Self {
        Self {
            session: None,
            proxy: None,
            connection: None,
            watcher: None,
            fd: None,
            node_id: 0,
            closed: watch::channel(false).1,
            restore_token: None,
            restored: false,
        }
    }

    /// Runs the portal flow once, presenting the saved restore token. The
    /// token `Start` returns is saved as soon as it arrives. On any failure
    /// the session is closed and the saved token is left as it was.
    pub async fn start(
        paths: &FthrPaths,
        cancel: Arc<AtomicBool>,
        timeouts: Timeouts,
    ) -> Result<Self, PortalFailure> {
        let token_path = token::path(paths);
        let presented = load_token(&token_path);
        let connection =
            match tokio::time::timeout(timeouts.method, zbus::Connection::session()).await {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => return Err(PortalFailure::Unavailable(format!("session bus: {e}"))),
                Err(_) => {
                    return Err(PortalFailure::Unavailable(
                        "the session bus did not answer".into(),
                    ));
                }
            };
        let proxy = match tokio::time::timeout(
            timeouts.method,
            Screencast::with_connection(connection.clone()),
        )
        .await
        {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => return Err(PortalFailure::Unavailable(e.to_string())),
            Err(_) => {
                return Err(PortalFailure::Unavailable(
                    "the portal did not answer".into(),
                ));
            }
        };
        let session = bounded(
            "CreateSession",
            timeouts.method,
            &cancel,
            proxy.create_session(CreateSessionOptions::default()),
        )
        .await?;
        let mut this = Self::empty();
        this.session = Some(Arc::new(session));
        this.proxy = Some(proxy);
        this.connection = Some(connection);
        this.restored = presented.is_some();
        match this
            .negotiate(presented.as_deref(), &token_path, &cancel, timeouts)
            .await
        {
            Ok(()) => Ok(this),
            Err(e) => {
                this.close().await;
                Err(e)
            }
        }
    }

    async fn negotiate(
        &mut self,
        presented: Option<&str>,
        token_path: &Path,
        cancel: &AtomicBool,
        timeouts: Timeouts,
    ) -> Result<(), PortalFailure> {
        let (Some(proxy), Some(session)) = (self.proxy.as_ref(), self.session.clone()) else {
            return Err(PortalFailure::Failed("the portal session is closed".into()));
        };
        let options = SelectSourcesOptions::default()
            .set_sources(BitFlags::from(SourceType::Monitor))
            .set_multiple(false)
            .set_cursor_mode(CursorMode::Embedded)
            .set_persist_mode(PersistMode::ExplicitlyRevoked)
            .set_restore_token(presented);
        let request = bounded(
            "SelectSources",
            timeouts.method,
            cancel,
            proxy.select_sources(&session, options),
        )
        .await?;
        request
            .response()
            .map_err(response_failure("SelectSources"))?;
        let request = bounded(
            "Start",
            timeouts.dialog,
            cancel,
            proxy.start(&session, None, StartCastOptions::default()),
        )
        .await?;
        let streams = request.response().map_err(response_failure("Start"))?;
        self.restore_token = streams.restore_token().map(str::to_string);
        persist_token(token_path, presented, self.restore_token.as_deref());
        let Some(stream) = streams.streams().first() else {
            return Err(PortalFailure::Failed(
                "the portal started a session without a stream".into(),
            ));
        };
        self.node_id = stream.pipe_wire_node_id();
        let fd = bounded(
            "OpenPipeWireRemote",
            timeouts.method,
            cancel,
            proxy.open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default()),
        )
        .await?;
        self.fd = Some(fd);
        let (closed_tx, closed_rx) = watch::channel(false);
        self.closed = closed_rx;
        self.watcher = Some(tokio::spawn(async move {
            match session.receive_closed().await {
                Ok(mut signal) => {
                    let _ = signal.next().await;
                }
                Err(e) => tracing::warn!("cannot watch the portal session: {e}"),
            }
            let _ = closed_tx.send(true);
        }));
        Ok(())
    }

    /// The PipeWire node of the screen stream.
    pub fn node_id(&self) -> u32 {
        self.node_id
    }

    /// Takes the PipeWire remote fd (once).
    pub fn take_fd(&mut self) -> Option<OwnedFd> {
        self.fd.take()
    }

    /// Flips to `true` when the portal closes the session.
    pub fn closed(&self) -> watch::Receiver<bool> {
        self.closed.clone()
    }

    /// The restore token the portal issued, if any.
    pub fn restore_token(&self) -> Option<&str> {
        self.restore_token.as_deref()
    }

    /// Whether the session was started from a saved restore token.
    pub fn restored(&self) -> bool {
        self.restored
    }

    /// Closes the session (the screen-sharing indicator goes out) and its
    /// D-Bus connection.
    pub async fn close(&mut self) {
        if let Some(watcher) = self.watcher.take() {
            watcher.abort();
        }
        if let Some(session) = self.session.take() {
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, session.close()).await;
        }
        self.proxy = None;
        self.connection = None;
    }

    /// [`close`](Self::close) for synchronous callers. zbus objects must be
    /// dropped inside a runtime, so outside one this blocks on the portal
    /// runtime; inside one the close is handed to the portal runtime.
    pub fn close_blocking(&mut self) {
        if self.session.is_none() && self.connection.is_none() && self.watcher.is_none() {
            return;
        }
        let mut open = std::mem::replace(self, Self::empty());
        let closing = async move { open.close().await };
        let inside = tokio::runtime::Handle::try_current();
        match (crate::runtime(), inside) {
            (Ok(runtime), Ok(_)) => drop(runtime.spawn(closing)),
            (Ok(runtime), Err(_)) => runtime.block_on(closing),
            (Err(_), Ok(handle)) => drop(handle.spawn(closing)),
            // No runtime to drop the connection on: leak it rather than
            // panic in a destructor (the process is shutting down anyway).
            (Err(_), Err(_)) => std::mem::forget(closing),
        }
    }
}

impl Drop for ScreenCastSession {
    fn drop(&mut self) {
        self.close_blocking();
    }
}
