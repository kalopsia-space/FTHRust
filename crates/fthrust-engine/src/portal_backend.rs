//! The portal backend adapter: a ScreenCast session plus its PipeWire
//! stream behind the [`CaptureBackend`] trait.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use fthrust_core::FthrPaths;
use fthrust_core::capture_config::CaptureConfig;
use fthrust_ipc::BackendKind;
use fthrust_portal::capture::{Dialog, PortalCapture};
use fthrust_portal::screencast::{PortalFailure, Timeouts};
use fthrust_wayland::frame::FrameView;

use crate::backend::{BackendFailure, CaptureBackend, CaptureError, FailureKind, OpenReason};

/// Dropping it stops the stream and closes the portal session (the
/// screen-sharing indicator goes out).
struct PortalBackend(PortalCapture);

impl CaptureBackend for PortalBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::PortalScreenCast
    }
    fn capture(&mut self) -> Result<FrameView<'_>, CaptureError> {
        self.0.capture()
    }
    fn native_size(&self) -> (u32, u32) {
        self.0.native_size()
    }
    fn selected_output(&self) -> String {
        String::new()
    }
    fn last_frame_available(&self) -> bool {
        true
    }
}

fn failure(e: PortalFailure) -> BackendFailure {
    match e {
        PortalFailure::UserDeclined => BackendFailure {
            kind: FailureKind::Error,
            reason: "the screen picker was dismissed".into(),
            retry_pointless: true,
        },
        // Only the user can bring the dialog back (by restarting capture).
        e @ PortalFailure::NeedsDialog => BackendFailure {
            kind: FailureKind::Error,
            reason: e.to_string(),
            retry_pointless: true,
        },
        PortalFailure::Cancelled => BackendFailure {
            kind: FailureKind::Cancelled,
            reason: "capture cancelled".into(),
            retry_pointless: false,
        },
        e => BackendFailure {
            kind: FailureKind::Error,
            reason: e.to_string(),
            retry_pointless: false,
        },
    }
}

/// Opens a portal session and its stream: with the user's dialog when the
/// user started capture, restore-only when recovery re-opens it.
pub fn open(
    paths: Option<&FthrPaths>,
    cfg: &CaptureConfig,
    reason: OpenReason,
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
    let dialog = match reason {
        OpenReason::Start => Dialog::Allowed,
        OpenReason::Recovery => Dialog::Forbidden,
    };
    let capture = PortalCapture::open(&paths, cfg.fps, cancel, Timeouts::default(), dialog)
        .map_err(failure)?;
    Ok(Box::new(PortalBackend(capture)))
}
