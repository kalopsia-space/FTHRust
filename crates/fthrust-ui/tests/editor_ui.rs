//! Editor layer behaviour (kittest).

mod common;

use egui::{Pos2, Vec2};
use fthrust_ui::editor::{Editor, EditorRequest, Lifecycle};
use fthrust_ui::shell;
use fthrust_ui::state::AppState;

fn editor_state(lifecycle: Lifecycle) -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.tokens.reduce_motion = true;
    s.editor = Some(Editor::for_tests(
        &tmp.path().join("Overwatch_clip.mp4"),
        10_000,
        lifecycle,
    ));
    (tmp, s)
}

fn click(h: &mut egui_kittest::Harness<'_>, at: Pos2) {
    h.drag_at(at);
    h.step();
    h.drop_at(at);
    h.run_steps(2);
}

#[test]
fn header_buttons_emit_requests() {
    let (_tmp, mut s) = editor_state(Lifecycle::Paused);
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        // The layer alone: `shell::frame` would also service the requests.
        fthrust_ui::editor::layer::ui(ui, &mut s);
    });
    click(&mut h, Pos2::new(1340.0, 20.0));
    click(&mut h, Pos2::new(1420.0, 20.0));
    drop(h);
    let e = s.editor.as_ref().unwrap();
    assert_eq!(e.requests, vec![EditorRequest::Park, EditorRequest::Close]);
}

#[test]
fn play_button_toggles_the_queue() {
    let (_tmp, mut s) = editor_state(Lifecycle::PlayQueued);
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    click(&mut h, Pos2::new(64.0, 679.0));
    drop(h);
    let e = s.editor.as_ref().unwrap();
    assert_eq!(
        e.lifecycle,
        Lifecycle::Preparing,
        "a click while queued cancels"
    );
    assert_eq!(e.status_text(), "Preparing clip…");
}

// ---------------------------------------------------------------------------
// Timeline (1440×900: the trim bar starts at y 702; the 116 px timeline
// widget at y 772, x 16..1144; its track spans x 30..1130, y 794..864; the
// minimap y 875..882).

const TRACK_X0: f32 = 30.0;
const TRACK_W: f32 = 1100.0;
const TRACK_Y: f32 = 830.0;
const MINIMAP_Y: f32 = 878.0;

fn track_x(pct: f32) -> f32 {
    TRACK_X0 + pct * TRACK_W
}

fn harness_for(s: &mut AppState) -> egui_kittest::Harness<'_> {
    let tokens = s.tokens.clone();
    common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, move |ui| {
        shell::frame(ui, s);
    })
}

#[test]
fn click_scrubs_and_selects() {
    let (_tmp, mut s) = editor_state(Lifecycle::Paused);
    {
        let e = s.editor.as_mut().unwrap();
        e.timeline.split_at(0.5);
        e.timeline.selected = 1;
    }
    let mut h = harness_for(&mut s);
    click(&mut h, Pos2::new(track_x(0.3), TRACK_Y));
    drop(h);
    let e = s.editor.as_ref().unwrap();
    assert_eq!(e.position_ms(), 3000);
    assert_eq!(e.timeline.selected, 0, "the segment under the click");
}

#[test]
fn handle_drag_trims_one_undo_step() {
    let (_tmp, mut s) = editor_state(Lifecycle::Paused);
    let mut h = harness_for(&mut s);
    h.drag_at(Pos2::new(track_x(0.0), TRACK_Y));
    h.step();
    for i in 1..=4 {
        h.hover_at(Pos2::new(track_x(0.05 * i as f32), TRACK_Y));
        h.step();
    }
    h.drop_at(Pos2::new(track_x(0.2), TRACK_Y));
    h.run_steps(2);
    drop(h);
    let e = s.editor.as_ref().unwrap();
    assert!(
        (e.timeline.trim_start - 0.2).abs() < 0.002,
        "{}",
        e.timeline.trim_start
    );
    assert_eq!(e.undo.undo.len(), 1);
    assert_eq!(e.timeline_label(), "0:02 → 0:10");
}

fn wheel(delta_y: f32, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: Vec2::new(0.0, delta_y),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers,
    }
}

#[test]
fn wheel_zooms_and_shift_pans() {
    let (_tmp, mut s) = editor_state(Lifecycle::Paused);
    let mut h = harness_for(&mut s);
    h.hover_at(Pos2::new(track_x(0.5), TRACK_Y));
    h.step();
    h.event(wheel(1.0, egui::Modifiers::NONE));
    h.run_steps(2);
    drop(h);
    let (zoom, start) = {
        let e = s.editor.as_ref().unwrap();
        (e.timeline.zoom, e.timeline.view_start)
    };
    assert!(
        (zoom - 1.25).abs() < 1e-9,
        "wheel up zooms in by 1.25: {zoom}"
    );
    assert!(
        (start - 0.1).abs() < 1e-6,
        "anchored at the cursor: {start}"
    );
    let mut h = harness_for(&mut s);
    h.hover_at(Pos2::new(track_x(0.5), TRACK_Y));
    h.step();
    h.event(wheel(-1.0, egui::Modifiers::SHIFT));
    h.run_steps(2);
    drop(h);
    let e = s.editor.as_ref().unwrap();
    assert!(
        (e.timeline.zoom - 1.25).abs() < 1e-9,
        "Shift+wheel does not zoom"
    );
    assert!(
        (e.timeline.view_start - 0.2).abs() < 1e-9,
        "panned right, clamped to 1 - span"
    );
}

#[test]
fn context_menu_targets_click_position() {
    use egui_kittest::kittest::Queryable;
    let (_tmp, mut s) = editor_state(Lifecycle::Paused);
    let mut h = harness_for(&mut s);
    let at = Pos2::new(track_x(0.5), TRACK_Y);
    h.hover_at(at);
    h.step();
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Secondary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Secondary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(3);
    h.get_by_label("Split here").click();
    h.run_steps(3);
    drop(h);
    let e = s.editor.as_ref().unwrap();
    assert_eq!(e.timeline.segments.len(), 2);
    assert!((e.timeline.segments[0].end - 0.5).abs() < 0.001);
    assert_eq!(e.undo.undo.len(), 1);
}

#[test]
fn minimap_press_centers_view() {
    let (_tmp, mut s) = editor_state(Lifecycle::Paused);
    s.editor.as_mut().unwrap().timeline.set_zoom(4.0, Some(0.0));
    let mut h = harness_for(&mut s);
    click(&mut h, Pos2::new(track_x(0.8), MINIMAP_Y));
    drop(h);
    let e = s.editor.as_ref().unwrap();
    assert!(
        (e.timeline.view_start - (0.8 - 0.125)).abs() < 0.002,
        "{}",
        e.timeline.view_start
    );
    assert_eq!(e.position_ms(), 8000);
}

// ---------------------------------------------------------------------------
// Sidebar (x 1160..1440; content 14 px in, 244 px wide while it scrolls).

fn marker_center(s: &mut AppState, name: &str) -> Pos2 {
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, s);
    });
    r.marker(name)
        .unwrap_or_else(|| panic!("no marker {name}"))
        .rect
        .center()
}

#[test]
fn speed_change_is_one_undo_step_and_applies_rate() {
    let (_tmp, mut s) = editor_state(Lifecycle::Paused);
    let combo = marker_center(&mut s, "speedCombo");
    let mut h = harness_for(&mut s);
    click(&mut h, combo);
    h.run_steps(2);
    let item = common::render_harness(&mut h)
        .marker("speedCombo/item[0.5×  SLOW]")
        .expect("open list")
        .rect
        .center();
    click(&mut h, item);
    h.run_steps(3);
    drop(h);
    let e = s.editor.as_ref().unwrap();
    assert_eq!(e.speed, 0.5);
    assert_eq!(e.undo.undo.len(), 1);
}

#[test]
fn quality_combo_persists() {
    let (_tmp, mut s) = editor_state(Lifecycle::Paused);
    let combo = marker_center(&mut s, "WheelSafeComboBox[High quality]");
    let mut h = harness_for(&mut s);
    click(&mut h, combo);
    h.run_steps(2);
    let item = common::render_harness(&mut h)
        .marker("WheelSafeComboBox[High quality]/item[Lossless · MKV]")
        .expect("open list")
        .rect
        .center();
    click(&mut h, item);
    h.run_steps(3);
    drop(h);
    assert_eq!(s.settings.get_str("export_quality"), Some("lossless"));
}

#[test]
fn details_flush_on_focus_loss() {
    let (tmp, mut s) = editor_state(Lifecycle::Paused);
    // Only CLIP DETAILS open, so its fields are on screen.
    s.editor_panels = [false, false, false, false, true, false];
    let edit = marker_center(&mut s, "detailEdit[e.g. Gaming, Highlight …]");
    let mut h = harness_for(&mut s);
    click(&mut h, edit);
    h.event(egui::Event::Text("Highlight".into()));
    h.run_steps(2);
    click(&mut h, Pos2::new(600.0, 300.0));
    h.run_steps(2);
    drop(h);
    s.tick_editor(None);
    let store = fthrust_core::clip_metadata::ClipMetadataStore::open(&s.paths).unwrap();
    let entry = store.get(&tmp.path().join("Overwatch_clip.mp4")).unwrap();
    assert_eq!(entry.tag, "Highlight");
    assert_eq!(entry.description, "");
}

#[test]
fn linked_import_delete_is_protected() {
    let tmp = tempfile::tempdir().unwrap();
    let imports = tmp.path().join("imports");
    std::fs::create_dir_all(&imports).unwrap();
    let mut s = AppState::for_tests(
        tmp.path(),
        serde_json::json!({"imported_clip_folders": [imports.to_string_lossy()]}),
    );
    s.tokens.reduce_motion = true;
    s.editor = Some(Editor::for_tests(
        &imports.join("linked.mp4"),
        10_000,
        Lifecycle::Paused,
    ));
    let tokens = s.tokens.clone();
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    let m = r.marker("deleteBtn").expect("delete button");
    assert_eq!(
        m.text.as_ref().map(|t| t.content.as_str()),
        Some("Linked original — protected")
    );
}
