//! Screenshots through the XDG Screenshot portal (`_on_hotkey_save_screenshot`
//! with the reference's `WaylandScreenshotPortal` finally wired in): one
//! request in flight plus up to nine queued, the staged PNG published into
//! `<clips>/Screenshots`, and failures that stay actionable.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fthrust_core::screenshot_save::{
    ScreenshotPaths, publish_staged_png, reserve_screenshot_paths, validate_png_bytes,
};
use serde_json::json;
use time::OffsetDateTime;

use super::CardKind;
use crate::shell::error_bar::{Action, Level};
use crate::state::{AppState, DialogKind};

/// The portal must answer within this long (the compositor dialog).
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
/// Requests queued behind the in-flight one.
pub const MAX_PENDING: u8 = 9;

/// A portal failure with the reference's code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenshotFailure {
    /// `PORTAL_UNAVAILABLE`, `CANCELLED`, `PORTAL_FAILED`, `TIMEOUT`, …
    pub code: &'static str,
    /// Detail.
    pub detail: String,
}

/// Takes one screenshot and returns the local PNG the portal wrote.
pub trait PortalScreenshot: Send + Sync {
    /// Blocks until the portal answers.
    fn take(&self) -> Result<PathBuf, ScreenshotFailure>;
}

/// The real portal on the services runtime.
pub struct AshpdScreenshot {
    handle: tokio::runtime::Handle,
}

impl AshpdScreenshot {
    /// Uses `handle` for the D-Bus calls.
    pub fn new(handle: tokio::runtime::Handle) -> Self {
        Self { handle }
    }
}

impl PortalScreenshot for AshpdScreenshot {
    fn take(&self) -> Result<PathBuf, ScreenshotFailure> {
        self.handle.block_on(async {
            let fut = async {
                let request = ashpd::desktop::screenshot::Screenshot::request()
                    .interactive(false)
                    .modal(false)
                    .send()
                    .await?;
                let shot = request.response()?;
                Ok::<String, ashpd::Error>(shot.uri().as_str().to_owned())
            };
            match tokio::time::timeout(RESPONSE_TIMEOUT, fut).await {
                Ok(Ok(uri)) => local_path_from_uri(&uri),
                Ok(Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled))) => {
                    Err(ScreenshotFailure {
                        code: "CANCELLED",
                        detail: "Screenshot was canceled in the compositor dialog.".into(),
                    })
                }
                Ok(Err(ashpd::Error::Response(_))) => Err(ScreenshotFailure {
                    code: "PORTAL_FAILED",
                    detail: "The compositor could not capture a screenshot.".into(),
                }),
                Ok(Err(e)) => Err(ScreenshotFailure {
                    code: "PORTAL_UNAVAILABLE",
                    detail: format!(
                        "Screenshot portal unavailable: {}",
                        e.to_string().chars().take(400).collect::<String>()
                    ),
                }),
                Err(_) => Err(ScreenshotFailure {
                    code: "TIMEOUT",
                    detail: "The compositor screenshot dialog timed out.".into(),
                }),
            }
        })
    }
}

/// `file:///path/with%20escapes.png` → the local path.
pub fn local_path_from_uri(uri: &str) -> Result<PathBuf, ScreenshotFailure> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| ScreenshotFailure {
            code: "PORTAL_FAILED",
            detail: format!("Invalid screenshot portal result: {uri}"),
        })?;
    let path_part = rest.find('/').map(|i| &rest[i..]).unwrap_or("");
    let bytes = path_part.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&path_part[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    let s = String::from_utf8(out).map_err(|_| ScreenshotFailure {
        code: "PORTAL_FAILED",
        detail: "Invalid screenshot portal result: bad encoding".into(),
    })?;
    if s.is_empty() {
        return Err(ScreenshotFailure {
            code: "PORTAL_FAILED",
            detail: "Invalid screenshot portal result: empty path".into(),
        });
    }
    Ok(PathBuf::from(s))
}

/// What the service wants done.
#[derive(Debug)]
pub enum ShotAction {
    /// Show a card.
    Card(CardKind),
    /// Play a sound.
    Sound(String),
    /// Push an error-bar entry and a warning dialog.
    Failed {
        /// Code.
        code: String,
        /// Detail.
        detail: String,
    },
    /// A diagnostics event.
    Diag {
        /// Name.
        event: String,
        /// Fields.
        fields: serde_json::Value,
    },
}

struct Job {
    paths: ScreenshotPaths,
    rx: Receiver<Result<PathBuf, ScreenshotFailure>>,
}

/// The service.
pub struct ScreenshotService {
    portal: Option<Arc<dyn PortalScreenshot>>,
    job: Option<Job>,
    pending: u8,
    hold: Arc<Mutex<bool>>,
}

impl Default for ScreenshotService {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenshotService {
    /// Idle, without a portal (requests fail as `PORTAL_UNAVAILABLE`).
    pub fn new() -> Self {
        ScreenshotService {
            portal: None,
            job: None,
            pending: 0,
            hold: Arc::new(Mutex::new(false)),
        }
    }

    /// Installs the portal implementation.
    pub fn set_portal(&mut self, portal: Arc<dyn PortalScreenshot>) {
        self.portal = Some(portal);
    }

    /// Pauses portal calls (tests of the queue).
    pub fn set_hold(&self, hold: bool) {
        if let Ok(mut h) = self.hold.lock() {
            *h = hold;
        }
    }

    /// A request is running.
    pub fn inflight(&self) -> bool {
        self.job.is_some()
    }

    /// Requests waiting behind the running one.
    pub fn pending(&self) -> u8 {
        self.pending
    }

    /// The hotkey fired.
    pub fn request(&mut self, state: &mut AppState, wall: OffsetDateTime) -> Vec<ShotAction> {
        let mut out = Vec::new();
        if self.job.is_some() {
            if self.pending < MAX_PENDING {
                self.pending += 1;
            }
            return out;
        }
        let dir = match state.settings.clips_directory() {
            Ok(d) => d.join("Screenshots"),
            Err(e) => {
                out.push(ShotAction::Failed {
                    code: "STORAGE_UNAVAILABLE".into(),
                    detail: e.to_string(),
                });
                return out;
            }
        };
        let paths = match reserve_screenshot_paths(&dir, wall) {
            Ok(p) => p,
            Err(e) => {
                out.push(ShotAction::Failed {
                    code: e.code.into(),
                    detail: e.detail,
                });
                return out;
            }
        };
        let Some(portal) = self.portal.clone() else {
            let _ = std::fs::remove_file(&paths.staged);
            out.push(ShotAction::Failed {
                code: "PORTAL_UNAVAILABLE".into(),
                detail: "The desktop session bus is unavailable.".into(),
            });
            return out;
        };
        out.push(ShotAction::Diag {
            event: "screenshot_requested".into(),
            fields: json!({"final": paths.final_path.display().to_string()}),
        });
        let (tx, rx) = channel();
        let hold = self.hold.clone();
        std::thread::Builder::new()
            .name("fthrust-screenshot".into())
            .spawn(move || {
                while hold.lock().map(|h| *h).unwrap_or(false) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let _ = tx.send(portal.take());
            })
            .ok();
        self.job = Some(Job { paths, rx });
        out
    }

    /// Polls the running request; starts the next queued one when done.
    pub fn on_tick(&mut self, state: &mut AppState, wall: OffsetDateTime) -> Vec<ShotAction> {
        let mut out = Vec::new();
        let result = match &self.job {
            Some(job) => match job.rx.try_recv() {
                Ok(r) => Some(r),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(_) => Some(Err(ScreenshotFailure {
                    code: "PORTAL_FAILED",
                    detail: "The screenshot worker stopped.".into(),
                })),
            },
            None => None,
        };
        let Some(result) = result else {
            return out;
        };
        let job = self.job.take().expect("job");
        match result.and_then(|src| import(&src, &job.paths)) {
            Ok(()) => {
                let path = job.paths.final_path.clone();
                state.readiness.engine_committed(&path, false);
                state.readiness.ready(&path);
                if state.library.upsert(&path, false) {
                    state.library_dirty = true;
                }
                out.push(ShotAction::Card(CardKind::Screenshot));
                out.push(ShotAction::Sound("screenshot_captured".into()));
                out.push(ShotAction::Diag {
                    event: "screenshot_saved".into(),
                    fields: json!({"path": path.display().to_string()}),
                });
            }
            Err(e) => {
                let _ = std::fs::remove_file(&job.paths.staged);
                out.push(ShotAction::Failed {
                    code: e.code.into(),
                    detail: e.detail,
                });
            }
        }
        if self.pending > 0 {
            self.pending -= 1;
            let mut more = self.request(state, wall);
            out.append(&mut more);
        }
        out
    }
}

/// Validates the portal's PNG and publishes it under the reserved name.
fn import(src: &Path, paths: &ScreenshotPaths) -> Result<(), ScreenshotFailure> {
    let bytes = std::fs::read(src).map_err(|e| ScreenshotFailure {
        code: "PORTAL_FAILED",
        detail: format!("Could not import screenshot portal image: {e}"),
    })?;
    validate_png_bytes(&bytes).map_err(|e| ScreenshotFailure {
        code: "INVALID_IMAGE",
        detail: e.detail,
    })?;
    std::fs::write(&paths.staged, &bytes).map_err(|e| ScreenshotFailure {
        code: "WRITE_FAILED",
        detail: format!("Could not write the staged screenshot: {e}"),
    })?;
    publish_staged_png(&paths.staged, &paths.final_path).map_err(|e| ScreenshotFailure {
        code: match e.code {
            "OUTPUT_COLLISION" => "OUTPUT_COLLISION",
            _ => "WRITE_FAILED",
        },
        detail: e.detail,
    })?;
    // The portal's own copy is not needed any more.
    let _ = std::fs::remove_file(src);
    Ok(())
}

/// Applies the service's actions to the state.
pub fn apply(state: &mut AppState, actions: Vec<ShotAction>, cards: &mut Vec<CardKind>) {
    for a in actions {
        match a {
            ShotAction::Card(c) => {
                cards.push(c.clone());
                state.pending_cards.push(c);
            }
            ShotAction::Sound(k) => state.pending_sounds.push(k),
            ShotAction::Failed { code, detail } => {
                state.error_bar.push(
                    "SCREENSHOT FAILED",
                    &format!("{code}: {detail}"),
                    Level::Error,
                    Vec::<Action>::new(),
                    None,
                    true,
                );
                state.dialogs.push_modal(DialogKind::Message {
                    title: "Screenshot Failed".into(),
                    message: format!("{code}\n\n{detail}"),
                });
                super::diagnostics::emit(
                    state,
                    "screenshot",
                    "failed",
                    json!({"code": code, "detail": detail}),
                );
            }
            ShotAction::Diag { event, fields } => {
                super::diagnostics::emit(state, "screenshot", &event, fields)
            }
        }
    }
}
