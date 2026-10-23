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
        shell::frame(ui, &mut s);
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
