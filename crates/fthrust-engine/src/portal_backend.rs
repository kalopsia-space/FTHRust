//! The portal backend adapter: a ScreenCast session plus its PipeWire
//! stream behind the [`CaptureBackend`] trait.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use fthrust_core::FthrPaths;
use fthrust_core::capture_config::CaptureConfig;
use fthrust_ipc::BackendKind;
use fthrust_portal::screencast::{PortalFailure, ScreenCastSession, Timeouts};
use fthrust_portal::stream::PipeWireVideoStream;
use fthrust_wayland::frame::FrameView;

use crate::backend::{BackendFailure, CaptureBackend, CaptureError, FailureKind};

struct PortalBackend {
    stream: PipeWireVideoStream,
    session: ScreenCastSession,
    runtime: tokio::runtime::Runtime,
}

impl Drop for PortalBackend {
    /// Closes the portal session (the screen-sharing indicator goes out)
    /// while the runtime is still alive.
    fn drop(&mut self) {
        let session = &mut self.session;
        let _ = self.runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(2), session.close()).await
        });
    }
}

impl CaptureBackend for PortalBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::PortalScreenCast
    }
    fn capture(&mut self) -> Result<FrameView<'_>, CaptureError> {
        self.stream.capture()
    }
    fn native_size(&self) -> (u32, u32) {
        self.stream.native_size()
    }
    fn selected_output(&self) -> String {
        String::new()
    }
    fn last_frame_available(&self) -> bool {
        true
    }
}

/// Opens a portal session (the user's dialog) and its stream.
pub fn open(
    paths: Option<&FthrPaths>,
    cfg: &CaptureConfig,
    cancel: Arc<AtomicBool>,
) -> Result<Box<dyn CaptureBackend>, BackendFailure> {
    let paths = match paths {
        Some(p) => p.clone(),
        None => FthrPaths::from_env().map_err(|e| BackendFailure {
            kind: FailureKind::Error,
            reason: e.to_string(),
            retry_pointless: false,
        })?,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| BackendFailure {
            kind: FailureKind::Error,
            reason: format!("tokio: {e}"),
            retry_pointless: false,
        })?;
    let session = runtime.block_on(ScreenCastSession::start(
        &paths,
        cancel.clone(),
        Timeouts::default(),
    ));
    let mut session = match session {
        Ok(s) => s,
        Err(PortalFailure::UserDeclined) => {
            return Err(BackendFailure {
                kind: FailureKind::Error,
                reason: "the screen picker was dismissed".into(),
                retry_pointless: true,
            });
        }
        Err(PortalFailure::Cancelled) => {
            return Err(BackendFailure {
                kind: FailureKind::Cancelled,
                reason: "capture cancelled".into(),
                retry_pointless: false,
            });
        }
        Err(e) => {
            return Err(BackendFailure {
                kind: FailureKind::Error,
                reason: e.to_string(),
                retry_pointless: false,
            });
        }
    };
    let fd = session.take_fd().ok_or_else(|| BackendFailure {
        kind: FailureKind::Error,
        reason: "the portal session has no PipeWire fd".into(),
        retry_pointless: false,
    })?;
    let stream = PipeWireVideoStream::connect(
        fd,
        session.node_id(),
        cfg.fps,
        cancel,
        Some(session.closed()),
    )
    .map_err(|e| BackendFailure {
        kind: FailureKind::Error,
        reason: e.to_string(),
        retry_pointless: false,
    })?;
    Ok(Box::new(PortalBackend {
        stream,
        session,
        runtime,
    }))
}
