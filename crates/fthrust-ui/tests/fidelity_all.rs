//! The fidelity gate: every Qt reference screen this plan owns is rendered
//! from the same state the Qt renderer used and compared marker for marker
//! (see "UI fidelity tests" in `docs/development.md`). Screens owned by later
//! plans are ignored with the owning plan named.

mod common;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::dialogs;
use fthrust_ui::popups::{capture_settings, game_detection, hotkeys, source};
use fthrust_ui::shell;
use fthrust_ui::shell::error_bar::{Action, Level};
use fthrust_ui::state::{AppState, DialogKind, Page, StatusStyle, StatusText, Tab};
use fthrust_ui::widgets::popup;

fn screen_size(screen: &str) -> Vec2 {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/reference/qt-screens/"
    );
    let json = std::fs::read_to_string(format!("{path}{screen}.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    Vec2::new(
        v["size"][0].as_f64().unwrap() as f32,
        v["size"][1].as_f64().unwrap() as f32,
    )
}

fn state() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.tokens.reduce_motion = true;
    (tmp, s)
}

fn check(screen: &str, mut s: AppState, draw: impl FnMut(&mut egui::Ui, &mut AppState)) {
    let mut draw = draw;
    let tokens = s.tokens.clone();
    let size = screen_size(screen);
    let r = common::render(size, 1.0, &tokens, |ui| draw(ui, &mut s));
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, &tokens);
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    assert_fidelity!(r, screen, &tokens, &ctx);
}

fn settings_tab(screen: &str, tab: Tab, open: &[usize]) {
    let (_tmp, mut s) = state();
    s.page = Page::Settings;
    s.settings_tab = tab;
    s.status = StatusText {
        text: "STARTING CAPTURE".into(),
        style: StatusStyle::Idle,
    };
    for i in open {
        s.customize_open[*i] = true;
    }
    check(screen, s, |ui, s| {
        shell::frame(ui, s);
    });
}

fn popup_screen(
    screen: &str,
    name: &str,
    s: AppState,
    mut content: impl FnMut(&mut egui::Ui, &mut AppState),
) {
    let width = screen_size(screen).x;
    check(screen, s, |ui, s| {
        let t = s.tokens.clone();
        popup::draw_frame(ui, &t, name, Pos2::ZERO, width, |ui| content(ui, s));
    });
}

fn dialog_screen(screen: &str, kind: DialogKind) {
    let (_tmp, s) = state();
    check(screen, s, |ui, s| {
        dialogs::draw_at(ui, s, &kind, Pos2::ZERO);
    });
}

#[test]
fn settings() {
    settings_tab("settings", Tab::General, &[]);
}

#[test]
fn clip_settings() {
    settings_tab("clip-settings", Tab::Clip, &[]);
}

#[test]
fn audio() {
    settings_tab("audio", Tab::Audio, &[]);
}

#[test]
fn visuals() {
    settings_tab("visuals", Tab::Visuals, &[]);
}

#[test]
fn customize() {
    for screen in [
        "customize",
        "customize-1920",
        "customize-2560",
        "customize-3840",
    ] {
        settings_tab(screen, Tab::Customize, &[]);
    }
}

#[test]
fn typography() {
    settings_tab("typography", Tab::Customize, &[0]);
}

#[test]
fn colors() {
    settings_tab("colors", Tab::Customize, &[1]);
}

#[test]
fn performance() {
    settings_tab("performance", Tab::Performance, &[]);
}

#[test]
fn shortcuts() {
    settings_tab("shortcuts", Tab::Shortcuts, &[]);
}

#[test]
fn library() {
    let (_tmp, mut s) = state();
    s.page = Page::Clips;
    s.status = StatusText {
        text: "CONNECTING".into(),
        style: StatusStyle::Idle,
    };
    check("library", s, |ui, s| {
        shell::frame(ui, s);
    });
}

#[test]
fn capture_popup() {
    let (_tmp, s) = state();
    popup_screen("capture-popup", "capture", s, capture_settings::content);
}

#[test]
fn source_popup() {
    let (_tmp, mut s) = state();
    s.monitors = vec![fthrust_ui::state::MonitorInfo {
        name: "DP-1".into(),
        width: 2560,
        height: 1400,
        refresh_hz: 165,
    }];
    popup_screen("source-popup", "source", s, source::content);
}

#[test]
fn game_detection_popup() {
    let (_tmp, mut s) = state();
    s.running_windows = vec!["Steam · Steam".into()];
    popup_screen("game-detection-popup", "game", s, game_detection::content);
}

#[test]
fn hotkey_popup() {
    let (_tmp, s) = state();
    popup_screen("hotkey-popup", "hotkeys", s, |ui, s| {
        hotkeys::content(ui, s)
    });
}

#[test]
fn error_bars() {
    let (_tmp, mut s) = state();
    let bar = Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 36.0));
    s.error_bar.push(
        "CAPTURE FAILED",
        "The compositor stopped answering",
        Level::Error,
        vec![Action {
            label: "RESTART ENGINE".into(),
            id: "restart".into(),
        }],
        None,
        true,
    );
    let tokens = s.tokens.clone();
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, &tokens);
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let r = common::render(screen_size("error-bar-error"), 1.0, &tokens, |ui| {
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
            shell::error_bar::ui(ui, &mut s)
        });
    });
    assert_fidelity!(r, "error-bar-error", &tokens, &ctx);
    s.error_bar.push(
        "LIMITED FEATURE SUPPORT",
        "Game detection is unavailable on this compositor",
        Level::Warning,
        vec![Action {
            label: "LEARN MORE".into(),
            id: "learn".into(),
        }],
        Some("compositor".into()),
        true,
    );
    s.error_bar.push(
        "MIC NOT FOUND",
        "No microphone is connected",
        Level::Warning,
        vec![],
        None,
        true,
    );
    let r = common::render(screen_size("error-bar-warning"), 1.0, &tokens, |ui| {
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
            shell::error_bar::ui(ui, &mut s)
        });
    });
    assert_fidelity!(r, "error-bar-warning", &tokens, &ctx);
}

#[test]
fn dialog_message() {
    dialog_screen(
        "dialog-message",
        DialogKind::Message {
            title: "Clip not found".into(),
            message: "The clip is no longer on disk.".into(),
        },
    );
}

#[test]
fn dialog_question() {
    dialog_screen(
        "dialog-question",
        DialogKind::Question {
            title: "Delete clip".into(),
            message: "Delete Overwatch_clip.mp4?\nThis cannot be undone.".into(),
            tag: "t".into(),
        },
    );
}

#[test]
fn dialog_input() {
    dialog_screen(
        "dialog-input",
        DialogKind::Input {
            title: "Rename clip".into(),
            label: "New name".into(),
            text: "Overwatch_clip".into(),
            tag: "rename".into(),
        },
    );
}

#[test]
fn custom_clip_length() {
    dialog_screen(
        "custom-clip-length",
        DialogKind::CustomClipLength { seconds: 30 },
    );
}

#[test]
fn export_preset() {
    dialog_screen("export-preset", DialogKind::ExportPreset { preset: None });
}

#[test]
fn library_small() {
    let (_tmp, mut s) = state();
    s.page = Page::Clips;
    s.status = StatusText {
        text: "CONNECTING".into(),
        style: StatusStyle::Idle,
    };
    check("library-small", s, |ui, s| {
        shell::frame(ui, s);
    });
}

fn card_data(feedback: Option<&str>) -> fthrust_ui::library::card::CardData {
    fthrust_ui::library::card::CardData {
        path: std::path::PathBuf::from(
            "/clips/Overwatch/Overwatch_clip_from_04Oct2025_12-00-00.mp4",
        ),
        title: "Overwatch_clip_from_04Oct2025_12-00-00".into(),
        metadata: "Screenshot".into(),
        duration_s: Some(0.0),
        is_video: true,
        imported: false,
        uploaded: false,
        upload_link: None,
        upload_enabled: true,
        uploading: false,
        ready: true,
        feedback: feedback.map(str::to_owned),
        thumbnail: None,
    }
}

fn check_card(screen: &str, data: fthrust_ui::library::card::CardData, hover: Option<Pos2>) {
    let (_tmp, s) = state();
    let tokens = s.tokens.clone();
    let mut icons = fthrust_ui::icons::IconCache::default();
    let mut editing = None;
    let size = screen_size(screen);
    let tc = tokens.clone();
    let mut h = common::harness(size, 1.0, &tokens, move |ui| {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(Pos2::ZERO, size)),
            |ui| {
                fthrust_ui::library::card::card(
                    ui,
                    &tc,
                    &mut icons,
                    size.x,
                    &data,
                    &mut editing,
                    "card",
                );
            },
        );
    });
    if let Some(p) = hover {
        h.hover_at(p);
    }
    h.run_steps(2);
    let r = common::render_harness(&mut h);
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, &tokens);
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    assert_fidelity!(r, screen, &tokens, &ctx);
}

#[test]
fn clip_card() {
    check_card("clip-card", card_data(None), None);
}

#[test]
fn clip_card_hover() {
    check_card(
        "clip-card-hover",
        card_data(None),
        Some(Pos2::new(120.0, 60.0)),
    );
}

#[test]
fn clip_card_feedback() {
    check_card(
        "clip-card-feedback",
        card_data(Some("Upload requested")),
        None,
    );
}

#[test]
fn record_hover() {
    let (_tmp, mut s) = state();
    s.page = Page::Clips;
    s.status = StatusText {
        text: "CONNECTING".into(),
        style: StatusStyle::Idle,
    };
    let tokens = s.tokens.clone();
    let mut h = common::harness(screen_size("record-hover"), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    h.hover_at(Pos2::new(84.0, 68.0));
    h.run_steps(2);
    let r = common::render_harness(&mut h);
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, &tokens);
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    assert_fidelity!(r, "record-hover", &tokens, &ctx);
}

/// The editor as the Qt render shows it: a 10 s clip, play queued.
fn editor_state() -> (tempfile::TempDir, AppState) {
    let (tmp, mut s) = state();
    s.editor = Some(fthrust_ui::editor::Editor::for_tests(
        &tmp.path().join("Overwatch_clip.mp4"),
        10_000,
        fthrust_ui::editor::Lifecycle::PlayQueued,
    ));
    (tmp, s)
}

#[test]
fn clip_editor_covers_every_widget() {
    let (_tmp, mut s) = editor_state();
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    let qt = fthrust_ui::fidelity::QtScreen::load("clip-editor").unwrap();
    let missing: Vec<&str> = qt
        .widgets
        .iter()
        .map(|w| w.name.as_str())
        .filter(|n| !n.starts_with("qt_scrollarea"))
        .filter(|n| r.marker(n).is_none())
        .collect();
    assert!(missing.is_empty(), "no marker for {missing:?}");
}

#[test]
fn clip_editor_sidebar() {
    let (_tmp, mut s) = editor_state();
    {
        let e = s.editor.as_mut().unwrap();
        e.set_source_rows_for_tests(vec![
            ("desktop".into(), "Desktop".into()),
            ("mic".into(), "Mic".into()),
        ]);
    }
    s.editor_panels = [true; 6];
    check("clip-editor-sidebar", s, |ui, s| {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(280.0, 2160.0));
        fthrust_ui::editor::sidebar::ui(ui, rect, s);
    });
}

#[test]
fn clip_editor_rail() {
    let (tmp, mut s) = state();
    s.status = StatusText {
        text: "Connecting".into(),
        style: StatusStyle::Idle,
    };
    // The Qt render had the engine's failure notice showing.
    s.error_bar.push(
        "CAPTURE FAILED",
        "The compositor stopped answering",
        Level::Error,
        vec![Action {
            label: "RESTART ENGINE".into(),
            id: "restart".into(),
        }],
        None,
        true,
    );
    for (n, name) in ["Overwatch_clip.mp4", "Valorant_clip.mp4"]
        .iter()
        .enumerate()
    {
        let path = tmp.path().join(name);
        s.editor_slots.push(fthrust_ui::editor::sessions::Session {
            path: path.clone(),
            state: None,
        });
        s.editor_sessions.push(fthrust_ui::state::SessionSummary {
            number: n + 1,
            stem: name.trim_end_matches(".mp4").into(),
            file_name: (*name).into(),
        });
    }
    check("clip-editor-rail", s, |ui, s| {
        shell::frame(ui, s);
    });
}

/// The editor with a dialog open on a clip whose frame cannot be decoded
/// (the Qt renders show that state).
fn editor_dialog(request: fthrust_ui::editor::EditorRequest) -> (tempfile::TempDir, AppState) {
    let (tmp, mut s) = editor_state();
    s.editor.as_mut().unwrap().requests.push(request);
    fthrust_ui::editor::dialogs::service(&mut s);
    let ctx = egui::Context::default();
    fthrust_ui::editor::dialogs::wait_frame(&ctx, &mut s);
    (tmp, s)
}

#[test]
fn clip_editor_crop() {
    let (_tmp, s) = editor_dialog(fthrust_ui::editor::EditorRequest::OpenCrop);
    check("clip-editor-crop", s, |ui, s| {
        let rect = Rect::from_min_size(Pos2::ZERO, fthrust_ui::editor::dialogs::CROP_SIZE);
        fthrust_ui::editor::dialogs::draw_crop(ui, s, rect);
    });
}

#[test]
fn clip_editor_stretch() {
    let (_tmp, s) = editor_dialog(fthrust_ui::editor::EditorRequest::OpenStretch);
    check("clip-editor-stretch", s, |ui, s| {
        let rect = Rect::from_min_size(Pos2::ZERO, fthrust_ui::editor::dialogs::STRETCH_SIZE);
        fthrust_ui::editor::dialogs::draw_stretch(ui, s, rect);
    });
}

#[test]
fn clip_editor_timeline() {
    let (_tmp, mut s) = editor_state();
    {
        let e = s.editor.as_mut().unwrap();
        e.timeline.split_at(0.3);
        e.timeline.split_at(0.6);
        e.timeline.selected = 1;
        e.timeline.toggle_selected();
        e.timeline.playhead = 0.45;
        e.timeline.set_zoom(1.25, None);
        e.segments_changed();
    }
    check("clip-editor-timeline", s, |ui, s| {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1160.0, 198.0));
        fthrust_ui::editor::timeline_ui::ui(ui, rect, s);
    });
}

#[test]
fn clip_editor() {
    let (_tmp, s) = editor_state();
    check("clip-editor", s, |ui, s| {
        shell::frame(ui, s);
    });
}

// ---------------------------------------------------------------------------
// Export jobs window (`ui/export_jobs.py`): the render script's two tasks.

fn export_jobs_state(with_tasks: bool) -> (tempfile::TempDir, AppState) {
    use fthrust_media::export::Quality;
    use fthrust_ui::services::export::TaskView;
    let (tmp, s) = state();
    if with_tasks {
        let row = |id, name: &str, quality, status: &str, progress, done| TaskView {
            id,
            source: tmp.path().join(name),
            output: tmp.path().join("Exported").join(name),
            quality,
            status: status.into(),
            progress,
            detail: String::new(),
            done,
        };
        s.export_queue.insert_for_tests(row(
            1,
            "Overwatch_clip.mp4",
            Quality::Quality,
            "Exporting · software",
            42.0,
            false,
        ));
        s.export_queue.insert_for_tests(row(
            2,
            "Valorant_clip.mp4",
            Quality::Fast,
            "Completed",
            100.0,
            true,
        ));
    }
    (tmp, s)
}

#[test]
fn export_jobs() {
    let (_tmp, s) = export_jobs_state(true);
    check("export-jobs", s, |ui, s| {
        let rect = Rect::from_min_size(Pos2::ZERO, fthrust_ui::dialogs::export_jobs::SIZE);
        fthrust_ui::dialogs::export_jobs::draw(ui, s, rect);
    });
}

#[test]
fn export_jobs_empty() {
    let (_tmp, s) = export_jobs_state(false);
    check("export-jobs-empty", s, |ui, s| {
        let rect = Rect::from_min_size(Pos2::ZERO, fthrust_ui::dialogs::export_jobs::SIZE);
        fthrust_ui::dialogs::export_jobs::draw(ui, s, rect);
    });
}

// ---------------------------------------------------------------------------
// Share picker and windows (`ShareModeDialog`, `ShareWindow`).

#[test]
fn share_picker() {
    let (_tmp, mut s) = editor_state();
    let mut p = fthrust_core::export_presets::ExportPreset::builtin_discord();
    p.preset_id = "clip-25-mb".into();
    p.name = "Clip 25 MB".into();
    p.target_size_mb = 25.0;
    p.built_in = false;
    fthrust_core::export_presets::ExportPresetStore::new(&s.paths)
        .save(&p)
        .unwrap();
    fthrust_ui::editor::share::open_picker(&mut s);
    check("share-picker", s, |ui, s| {
        let rect = Rect::from_min_size(Pos2::ZERO, fthrust_ui::editor::share::PICKER_SIZE);
        fthrust_ui::editor::share::draw_picker(ui, s, rect);
    });
}

fn share_window(
    status: fthrust_ui::editor::share::ShareStatus,
    line: &str,
) -> (tempfile::TempDir, AppState) {
    let (tmp, mut s) = editor_state();
    s.share_windows
        .push(fthrust_ui::editor::share::ShareWindow::for_tests(
            "SHARE CLIP",
            status,
            line,
        ));
    (tmp, s)
}

fn check_share_window(screen: &str, status: fthrust_ui::editor::share::ShareStatus, line: &str) {
    let (_tmp, s) = share_window(status, line);
    check(screen, s, |ui, s| {
        let rect = Rect::from_min_size(Pos2::ZERO, fthrust_ui::editor::share::WINDOW_SIZE);
        fthrust_ui::editor::share::draw_window(ui, s, 0, rect);
    });
}

#[test]
fn share_window_exporting() {
    use fthrust_ui::editor::share::ShareStatus;
    check_share_window(
        "share-window-exporting",
        ShareStatus::Exporting,
        "Preparing...",
    );
}

#[test]
fn share_window_ready() {
    use fthrust_ui::editor::share::ShareStatus;
    check_share_window(
        "share-window-ready",
        ShareStatus::Ready,
        "Overwatch_clip_share_12-00-00_0123abcd.mp4",
    );
}

#[test]
fn share_window_failed() {
    use fthrust_ui::editor::share::ShareStatus;
    check_share_window(
        "share-window-failed",
        ShareStatus::Failed,
        "FFmpeg exited with code 1",
    );
}
