//! The webcam lifecycle (`_on_camera_toggled`, `_populate_camera_devices`,
//! `_start_camera_open`): devices are scanned on a thread, the recorder
//! opens on a thread whenever `camera_enabled` and the selected device
//! change, an open failure turns the overlay off with `CAMERA
//! UNAVAILABLE`, and the Visuals page gets a 10 fps preview frame.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use fthrust_media::camera::{CameraRecorder, Device, Source, list_devices};
use serde_json::json;

use super::finalize::CameraUse;
use crate::shell::error_bar::{Action, Level};
use crate::state::{AppState, Page, Tab};

/// Lists cameras.
pub type Lister = Arc<dyn Fn() -> Vec<Device> + Send + Sync>;
/// Maps a device to its capture source.
pub type Sourcer = Arc<dyn Fn(&Path) -> Source + Send + Sync>;

/// Preview interval (`_camera_preview_timer`, 100 ms).
const PREVIEW_S: f64 = 0.1;

type Opening = (PathBuf, usize, Receiver<Result<CameraRecorder, String>>);

/// The service.
pub struct CameraService {
    lister: Lister,
    sourcer: Sourcer,
    devices: Vec<Device>,
    scanned: bool,
    scan: Option<Receiver<Vec<Device>>>,
    recorder: Option<(PathBuf, Arc<CameraRecorder>)>,
    opening: Option<Opening>,
    /// A device that failed to open while enabled stays failed until the
    /// selection or the switch changes.
    failed: Option<PathBuf>,
    preview_seq: u64,
    preview_at: f64,
}

impl Default for CameraService {
    fn default() -> Self {
        Self::new()
    }
}

impl CameraService {
    /// V4L2 cameras.
    pub fn new() -> CameraService {
        CameraService::for_tests(Arc::new(list_devices), Arc::new(Source::v4l2))
    }

    /// Another lister and source mapping (tests: the synthetic camera).
    pub fn for_tests(lister: Lister, sourcer: Sourcer) -> CameraService {
        CameraService {
            lister,
            sourcer,
            devices: Vec::new(),
            scanned: false,
            scan: None,
            recorder: None,
            opening: None,
            failed: None,
            preview_seq: 0,
            preview_at: f64::NEG_INFINITY,
        }
    }

    /// Re-scans devices on a thread; a scan in progress is shared. The
    /// running camera keeps recording.
    pub fn refresh(&mut self) {
        if self.scan.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let lister = self.lister.clone();
        if std::thread::Builder::new()
            .name("fthr-camera-device-scan".into())
            .spawn(move || {
                let _ = tx.send(lister());
            })
            .is_ok()
        {
            self.scan = Some(rx);
        }
    }

    /// The first scan (app start; the reference scans when Visuals opens
    /// and opens the saved camera at launch).
    pub fn ensure_scanned(&mut self) {
        if !self.scanned && self.scan.is_none() {
            self.refresh();
        }
    }

    /// The Visuals preview is live (the UI repaints for it).
    pub fn preview_active(&self, state: &AppState) -> bool {
        state.page == Page::Settings
            && state.settings_tab == Tab::Visuals
            && self.recorder.as_ref().is_some_and(|(_, r)| r.is_running())
    }

    /// The camera for a save ending at `end`, `seconds` long: its frames
    /// are taken now, so the burn does not depend on the live ring.
    pub fn for_save(&self, state: &AppState, end: std::time::Instant, seconds: f64) -> CameraUse {
        if !state.flag("camera_enabled", false) {
            return CameraUse::Off;
        }
        match &self.recorder {
            Some((_, r)) if r.is_running() => CameraUse::Frames(r.snapshot(end, seconds)),
            _ => CameraUse::Unavailable,
        }
    }

    /// The running recorder (tests).
    pub fn recorder(&self) -> Option<Arc<CameraRecorder>> {
        self.recorder.as_ref().map(|(_, r)| r.clone())
    }

    fn selected(&self, state: &AppState) -> Option<(usize, PathBuf)> {
        let i = state
            .settings
            .get_i64("camera_device_index")
            .unwrap_or(0)
            .max(0) as usize;
        let i = if i < self.devices.len() { i } else { 0 };
        self.devices.get(i).map(|d| (i, d.path.clone()))
    }

    fn stop(&mut self) {
        if let Some((_, r)) = self.recorder.take() {
            r.stop();
        }
        self.opening = None;
    }

    /// Applies scans, open results, the switch and the selection, and the
    /// preview frame.
    pub fn tick(&mut self, state: &mut AppState, now: f64) {
        if let Some(rx) = &self.scan
            && let Ok(list) = rx.try_recv()
        {
            self.devices = list;
            self.scanned = true;
            self.scan = None;
            state.camera_devices = self.devices.iter().map(|d| d.name.clone()).collect();
        }
        let enabled = state.flag("camera_enabled", false);
        if !enabled {
            self.failed = None;
            self.stop();
            state.camera_preview = None;
            return;
        }
        if let Some((path, index, rx)) = &self.opening
            && let Ok(result) = rx.try_recv()
        {
            let (path, index) = (path.clone(), *index);
            self.opening = None;
            match result {
                Ok(r) => self.recorder = Some((path, Arc::new(r))),
                Err(diag) => {
                    self.failed = Some(path);
                    unavailable(state, index, &diag);
                    return;
                }
            }
        }
        let Some((index, path)) = self.selected(state) else {
            return;
        };
        let current = self
            .recorder
            .as_ref()
            .map(|(p, _)| p.clone())
            .or_else(|| self.opening.as_ref().map(|(p, _, _)| p.clone()));
        if current.as_ref() != Some(&path) && self.failed.as_ref() != Some(&path) {
            self.stop();
            let (tx, rx) = channel();
            let source = (self.sourcer)(&path);
            if std::thread::Builder::new()
                .name("fthr-camera-open".into())
                .spawn(move || {
                    let _ = tx.send(CameraRecorder::start(source));
                })
                .is_ok()
            {
                self.opening = Some((path, index, rx));
            }
        }
        self.preview(state, now);
    }

    fn preview(&mut self, state: &mut AppState, now: f64) {
        let visible = state.page == Page::Settings && state.settings_tab == Tab::Visuals;
        let previews = state
            .settings
            .get_bool("settings_visual_previews_enabled")
            .unwrap_or(true);
        if !visible || !previews || now - self.preview_at < PREVIEW_S {
            return;
        }
        self.preview_at = now;
        let Some((_, r)) = &self.recorder else {
            return;
        };
        let Some((seq, jpeg)) = r.latest_jpeg() else {
            return;
        };
        if seq == self.preview_seq {
            return;
        }
        self.preview_seq = seq;
        if let Some(img) = decode_preview(&jpeg) {
            state.camera_preview = Some((seq, img));
        }
    }

    /// Stops the camera (app exit).
    pub fn shutdown(&mut self) {
        self.stop();
    }
}

impl Drop for CameraService {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The JPEG at most 640×360, never upscaled.
fn decode_preview(jpeg: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).ok()?;
    let (w, h) = (img.width(), img.height());
    let scale = (640.0 / w as f32).min(360.0 / h as f32).min(1.0);
    let img = if scale < 1.0 {
        img.resize(
            (w as f32 * scale) as u32,
            (h as f32 * scale) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

/// `camera_enabled` off, saved, and the reference's failure text.
fn unavailable(state: &mut AppState, index: usize, diag: &str) {
    state.set_setting("camera_enabled", json!(false));
    state.camera_preview = None;
    let diag: String = diag.split_whitespace().collect::<Vec<_>>().join(" ");
    let diag: String = diag.chars().take(80).collect();
    let mut detail = format!(
        "Could not open camera device {index}. Confirm it is connected and not in use by another app. The camera overlay was turned off; choose another camera or try again."
    );
    if !diag.is_empty() {
        detail.push_str(&format!(" ({diag})"));
    }
    let detail: String = detail.chars().take(240).collect();
    if state.settings.get_bool("error_notifications_enabled") == Some(false) {
        return;
    }
    state.error_bar.push(
        "CAMERA UNAVAILABLE",
        &detail,
        Level::Warning,
        vec![Action {
            label: "OPEN CAMERA SETTINGS".into(),
            id: "open-camera-settings".into(),
        }],
        None,
        true,
    );
}
