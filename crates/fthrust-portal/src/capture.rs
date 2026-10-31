//! A running ScreenCast capture: the portal session plus its negotiated
//! PipeWire stream, opened with the reference's restore-token rule
//! (`RunPortalStartupWithRetry`) for a start the user asked for, and
//! restore-only for an automatic open.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use fthrust_core::FthrPaths;
use fthrust_wayland::backend::CaptureFailure;
use fthrust_wayland::frame::FrameView;

use crate::screencast::{PortalFailure, ScreenCastSession, Timeouts, should_retry_startup};
use crate::stream::{PipeWireVideoStream, StreamError};
use crate::token;

/// How long `Start` may take when only the saved session may be restored.
/// A portal that restores answers at once; one that does not is showing
/// its dialog, which is then dismissed.
pub const RESTORE_TIMEOUT: Duration = Duration::from_secs(3);

/// Whether an open may bring up the portal's dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    /// A start the user asked for (the app starting, a settings change,
    /// RESTART ENGINE): the dialog comes up when there is no saved choice
    /// or the portal rejects it, and a restored session whose stream never
    /// settles a format is retried once without its token.
    Allowed,
    /// An automatic open (recovery, or an engine the app restarted on its
    /// own after a crash or hang): only the saved session is restored. Without a saved token, or when the portal does not restore
    /// it within [`RESTORE_TIMEOUT`], the open fails with
    /// [`PortalFailure::NeedsDialog`]; the token is never forgotten.
    Forbidden,
}

/// A portal session and its stream. Dropping it stops the stream, then
/// closes the session.
pub struct PortalCapture {
    // Field order is drop order.
    stream: PipeWireVideoStream,
    session: ScreenCastSession,
}

impl PortalCapture {
    /// Runs the portal flow (the user's dialog unless the saved token
    /// restores the last choice, see [`Dialog`]) on the portal runtime and
    /// connects the stream. Only a restored session whose stream never
    /// settles a format, on a start that allows the dialog, forgets its
    /// token and runs once more without it. Nothing else is retried and
    /// nothing else forgets the token: a missing or silent portal, or a
    /// PipeWire error, leaves it for the next start.
    pub fn open(
        paths: &FthrPaths,
        fps: u32,
        cancel: Arc<AtomicBool>,
        mut timeouts: Timeouts,
        dialog: Dialog,
    ) -> Result<Self, PortalFailure> {
        if dialog == Dialog::Forbidden {
            if !matches!(token::load(&token::path(paths)), Ok(Some(_))) {
                // Without a saved choice the portal can only ask.
                return Err(PortalFailure::NeedsDialog);
            }
            timeouts.dialog = timeouts.dialog.min(RESTORE_TIMEOUT);
        }
        let runtime = crate::runtime()?;
        let mut attempt = 0;
        loop {
            let started =
                runtime.block_on(ScreenCastSession::start(paths, cancel.clone(), timeouts));
            let mut session = match started {
                Ok(session) => session,
                // In restore-only mode an unanswered Start is the dialog
                // waiting (the session close dismissed it).
                Err(PortalFailure::UserDeclined) if dialog == Dialog::Forbidden => {
                    return Err(PortalFailure::NeedsDialog);
                }
                Err(e) => return Err(e),
            };
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
                    if dialog == Dialog::Allowed
                        && e.negotiation_timed_out()
                        && should_retry_startup(attempt, restored)
                    {
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
