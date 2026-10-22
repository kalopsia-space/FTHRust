//! The ScreenCast portal session (ports the D-Bus flow of
//! `portal_screencast.cpp` onto ashpd): create session, select a monitor
//! with the cursor embedded, start (the user's dialog), open the PipeWire
//! remote, persist the restore token.

use std::os::fd::OwnedFd;
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

/// A restored session that fails to negotiate is retried once without
/// its token; nothing else is retried automatically.
pub fn should_retry_startup(attempt: u32, had_token: bool) -> bool {
    attempt == 0 && had_token
}

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

/// An established ScreenCast session.
pub struct ScreenCastSession {
    session: Option<Arc<Session<Screencast>>>,
    fd: Option<OwnedFd>,
    node_id: u32,
    closed: watch::Receiver<bool>,
    restore_token: Option<String>,
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

impl ScreenCastSession {
    /// Runs the portal flow, retrying once without a restore token when a
    /// restored session fails to negotiate.
    pub async fn start(
        paths: &FthrPaths,
        cancel: Arc<AtomicBool>,
        timeouts: Timeouts,
    ) -> Result<Self, PortalFailure> {
        let token_path = token::path(paths);
        let mut restore_token = match token::load(&token_path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("{e}; starting without a restore token");
                let _ = token::save(&token_path, "");
                None
            }
        };
        let mut attempt = 0;
        loop {
            let had_token = restore_token.is_some();
            match Self::start_once(restore_token.as_deref(), &cancel, timeouts).await {
                Ok(session) => {
                    if let Some(t) = &session.restore_token
                        && let Err(e) = token::save(&token_path, t)
                    {
                        tracing::warn!("could not save the portal restore token: {e}");
                    }
                    return Ok(session);
                }
                Err(e @ (PortalFailure::UserDeclined | PortalFailure::Cancelled)) => return Err(e),
                Err(e) if should_retry_startup(attempt, had_token) => {
                    tracing::warn!(
                        "restored portal session failed ({e}); retrying without the token"
                    );
                    let _ = token::save(&token_path, "");
                    restore_token = None;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn start_once(
        restore_token: Option<&str>,
        cancel: &AtomicBool,
        timeouts: Timeouts,
    ) -> Result<Self, PortalFailure> {
        let proxy = match tokio::time::timeout(timeouts.method, Screencast::new()).await {
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
            cancel,
            proxy.create_session(CreateSessionOptions::default()),
        )
        .await?;
        let options = SelectSourcesOptions::default()
            .set_sources(BitFlags::from(SourceType::Monitor))
            .set_multiple(false)
            .set_cursor_mode(CursorMode::Embedded)
            .set_persist_mode(PersistMode::ExplicitlyRevoked)
            .set_restore_token(restore_token);
        let request = bounded(
            "SelectSources",
            timeouts.method,
            cancel,
            proxy.select_sources(&session, options),
        )
        .await?;
        request.response().map_err(|e| match e {
            ashpd::Error::Response(ResponseError::Cancelled) => PortalFailure::UserDeclined,
            other => PortalFailure::Failed(format!("SelectSources: {other}")),
        })?;
        let request = bounded(
            "Start",
            timeouts.dialog,
            cancel,
            proxy.start(&session, None, StartCastOptions::default()),
        )
        .await?;
        let streams = request.response().map_err(|e| match e {
            ashpd::Error::Response(ResponseError::Cancelled) => PortalFailure::UserDeclined,
            other => PortalFailure::Failed(format!("Start: {other}")),
        })?;
        let Some(stream) = streams.streams().first() else {
            return Err(PortalFailure::Failed(
                "the portal started a session without a stream".into(),
            ));
        };
        let node_id = stream.pipe_wire_node_id();
        let restore_token = streams.restore_token().map(str::to_string);
        let fd = bounded(
            "OpenPipeWireRemote",
            timeouts.method,
            cancel,
            proxy.open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default()),
        )
        .await?;
        let (closed_tx, closed_rx) = watch::channel(false);
        let session = Arc::new(session);
        let watched = session.clone();
        tokio::spawn(async move {
            match watched.receive_closed().await {
                Ok(mut signal) => {
                    let _ = signal.next().await;
                }
                Err(e) => tracing::warn!("cannot watch the portal session: {e}"),
            }
            let _ = closed_tx.send(true);
        });
        Ok(Self {
            session: Some(session),
            fd: Some(fd),
            node_id,
            closed: closed_rx,
            restore_token,
        })
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

    /// Closes the session.
    pub async fn close(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = session.close().await;
        }
    }
}
