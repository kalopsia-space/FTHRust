//! A running ScreenCast capture: the portal session plus its negotiated
//! PipeWire stream, opened with the reference's restore-token rule
//! (`RunPortalStartupWithRetry`).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use fthrust_core::FthrPaths;
use fthrust_wayland::backend::CaptureFailure;
use fthrust_wayland::frame::FrameView;

use crate::screencast::{PortalFailure, ScreenCastSession, Timeouts, should_retry_startup};
use crate::stream::{PipeWireVideoStream, StreamError};
use crate::token;

/// A portal session and its stream. Dropping it stops the stream, then
/// closes the session.
pub struct PortalCapture {
    // Field order is drop order.
    stream: PipeWireVideoStream,
    session: ScreenCastSession,
}

impl PortalCapture {
    /// Runs the portal flow (the user's dialog unless the saved token
    /// restores the last choice) on the portal runtime and connects the
    /// stream. A restored session whose stream never settles a format is
    /// closed, its token is forgotten and the flow runs once more without
    /// it. Nothing else is retried and nothing else forgets the token: a
    /// missing or silent portal, or a PipeWire error, leaves it for the
    /// next start.
    pub fn open(
        paths: &FthrPaths,
        fps: u32,
        cancel: Arc<AtomicBool>,
        timeouts: Timeouts,
    ) -> Result<Self, PortalFailure> {
        let runtime = crate::runtime()?;
        let mut attempt = 0;
        loop {
            let mut session =
                runtime.block_on(ScreenCastSession::start(paths, cancel.clone(), timeouts))?;
            let Some(fd) = session.take_fd() else {
                return Err(PortalFailure::Failed(
                    "the portal session has no PipeWire fd".into(),
                ));
            };
            let connected = PipeWireVideoStream::connect(
                fd,
                session.node_id(),
                fps,
                cancel.clone(),
                Some(session.closed()),
            );
            match connected {
                Ok(stream) => return Ok(Self { stream, session }),
                Err(StreamError::Cancelled) => return Err(PortalFailure::Cancelled),
                Err(e) => {
                    let restored = session.restored();
                    session.close_blocking();
                    if e.negotiation_timed_out() && should_retry_startup(attempt, restored) {
                        tracing::warn!(
                            "the restored portal session did not negotiate ({e}); retrying without its restore token"
                        );
                        if let Err(err) = token::save(&token::path(paths), "") {
                            tracing::warn!("could not remove the portal restore token: {err}");
                        }
                        attempt += 1;
                        continue;
                    }
                    return Err(PortalFailure::Failed(e.to_string()));
                }
            }
        }
    }

    /// The next frame (see [`PipeWireVideoStream::capture`]).
    pub fn capture(&mut self) -> Result<FrameView<'_>, CaptureFailure> {
        self.stream.capture()
    }

    /// Negotiated size.
    pub fn native_size(&self) -> (u32, u32) {
        self.stream.native_size()
    }

    /// The portal session.
    pub fn session(&self) -> &ScreenCastSession {
        &self.session
    }
}
