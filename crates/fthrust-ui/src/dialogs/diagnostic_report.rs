//! `DiagnosticReportDialog`: the exported ZIP as a draggable file button.

use std::path::{Path, PathBuf};

use egui::Ui;
use fthrust_core::diagnostics::session::DiagnosticsSession;

use crate::fonts::Weight;
use crate::state::AppState;
use crate::tokens::{Color, size};
use crate::widgets::buttons;
use crate::widgets::section;

/// Writes the report ZIP into the profile's diagnostics directory.
pub fn export(state: &mut AppState) -> Result<PathBuf, String> {
    // The live session when the app runs; a fresh one in tests.
    let fresh;
    let session: &DiagnosticsSession = match &state.diagnostics {
        Some(s) => s,
        None => {
            fresh = DiagnosticsSession::start(&state.paths, crate::VERSION)
                .map_err(|e| e.to_string())?;
            &fresh
        }
    };
    let dir = state.paths.diagnostics_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = time::OffsetDateTime::now_utc()
        .format(time::macros::format_description!(
            "[year][month][day]-[hour][minute][second]"
        ))
        .map_err(|e| e.to_string())?;
    let dest = dir.join(format!("fthr-diagnostics-{stamp}.zip"));
    session
        .export_zip(&dest, &state.paths.app_log())
        .map_err(|e| e.to_string())
}

/// Body: the message and the file button (click opens the folder; drag
/// hands the file to the drop target).
pub fn body(ui: &mut Ui, state: &mut AppState, zip: &Path) {
    let t = state.tokens.clone();
    let w = ui.available_width();
    section::label_wrapped(
        ui,
        &t,
        "Your report is ready. Drag the ZIP below into your bug report.\nYou can also drag it later from Settings → Troubleshooting.",
        size::BODY_L,
        Weight::W400,
        Color::Text,
        w,
        0.0,
    );
    let name = zip
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let resp = buttons::outline(ui, &t, &name, "diagnosticReportFile");
    if resp.clicked() {
        crate::shell::open_url(
            &zip.parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        );
    }
    if resp.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), zip.to_path_buf());
    }
}
