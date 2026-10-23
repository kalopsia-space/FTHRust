//! Editor export: the spec it submits, the empty-edit error, the jobs
//! label, the export guards and Ctrl+E (notes sections 1, 9 and 10).

mod common;
#[path = "../../fthrust-media/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{Key, Modifiers, Vec2};
use fthrust_media::export::{ExportResult, ExportState, Job, Quality};
use fthrust_ui::editor::export_ui;
use fthrust_ui::editor::sessions;
use fthrust_ui::editor::{Editor, EditorRequest, Lifecycle, PlayerFactory};
use fthrust_ui::services::export::{ExportQueue, Runner};
use fthrust_ui::services::playback::audio_out::NullOutput;
use fthrust_ui::services::playback::{PlaybackService, real_clock};
use fthrust_ui::state::{AppState, DialogKind};

/// A runner that records jobs and blocks until released or cancelled.
#[derive(Clone)]
struct Capture {
    jobs: Arc<Mutex<Vec<Job>>>,
    release: Arc<AtomicBool>,
}

impl Capture {
    fn new() -> Capture {
        Capture {
            jobs: Arc::new(Mutex::new(Vec::new())),
            release: Arc::new(AtomicBool::new(false)),
        }
    }

    fn runner(&self) -> Runner {
        let c = self.clone();
        Arc::new(move |job, cancel, _, _| {
            c.jobs.lock().unwrap().push(job.clone());
            while !c.release.load(Ordering::SeqCst) && !cancel.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            ExportResult {
                state: ExportState::Completed,
                detail: String::new(),
            }
        })
    }

    fn wait_job(&self) -> Option<Job> {
        for _ in 0..400 {
            if let Some(j) = self.jobs.lock().unwrap().first() {
                return Some(j.clone());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.release.store(true, Ordering::SeqCst);
    }
}

fn editor_state() -> (tempfile::TempDir, AppState, Capture) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.tokens.reduce_motion = true;
    s.editor = Some(Editor::for_tests(
        &tmp.path().join("Overwatch_clip.mp4"),
        10_000,
        Lifecycle::Paused,
    ));
    let cap = Capture::new();
    s.export_queue = ExportQueue::with_runner(cap.runner());
    (tmp, s, cap)
}

#[test]
fn export_button_submits_kept_ranges() {
    let (_tmp, mut s, cap) = editor_state();
    {
        let e = s.editor.as_mut().unwrap();
        e.timeline.split_at(0.3);
        e.timeline.split_at(0.6);
        e.timeline.selected = 1;
        assert!(e.timeline.toggle_selected());
        e.segments_changed();
    }
    let spec = export_ui::spec_for(&s).unwrap();
    assert_eq!(spec.ranges_s.len(), 2);
    assert!((spec.ranges_s[0].0 - 0.0).abs() < 1e-6 && (spec.ranges_s[0].1 - 3.0).abs() < 1e-6);
    assert!((spec.ranges_s[1].0 - 6.0).abs() < 1e-6 && (spec.ranges_s[1].1 - 10.0).abs() < 1e-6);
    assert_eq!(spec.audio_bitrate_kbps, 320);
    assert!(spec.watermark.is_none(), "watermark_enabled defaults off");
    // The sidebar button's request goes through the session service.
    s.editor
        .as_mut()
        .unwrap()
        .requests
        .push(EditorRequest::Export);
    sessions::service(&mut s);
    let tasks = s.export_queue.tasks();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].quality, Quality::Quality);
    assert_eq!(
        tasks[0].output.parent(),
        Some(s.library.clips_root.join("Exported").as_path())
    );
    let job = cap.wait_job().expect("the job ran");
    let g = job.plan.filter_graph.unwrap();
    assert!(g.contains("trim=start=0.000000:end=3.000000"), "{g}");
    assert!(g.contains("trim=start=6.000000:end=10.000000"), "{g}");
    assert!((job.expected_duration_s - 7.0).abs() < 1e-6);
}

#[test]
fn watermark_and_quality_follow_settings() {
    let (_tmp, mut s, _cap) = editor_state();
    s.settings.set("watermark_enabled", serde_json::json!(true));
    s.settings
        .set("export_quality", serde_json::json!("lossless"));
    let spec = export_ui::spec_for(&s).unwrap();
    let w = spec.watermark.expect("watermark on");
    assert!(w.font_path.ends_with("Oswald-Bold.ttf"), "{}", w.font_path);
    assert!(w.icon_path.ends_with("favicon.ico"), "{}", w.icon_path);
    assert!(w.bg.starts_with("0x"), "{}", w.bg);
    export_ui::export_now(&mut s);
    let t = s.export_queue.tasks();
    assert_eq!(t[0].quality, Quality::Lossless);
    assert!(t[0].output.to_string_lossy().ends_with(".mkv"));
}

#[test]
fn empty_edit_shows_error() {
    let (_tmp, mut s, _cap) = editor_state();
    {
        let e = s.editor.as_mut().unwrap();
        e.timeline.split_at(0.5);
        e.timeline.set_trim_end(0.5);
        e.timeline.selected = 0;
        assert!(e.timeline.toggle_selected());
        e.segments_changed();
        assert!(e.timeline.kept_ranges().is_empty());
    }
    export_ui::export_now(&mut s);
    assert!(s.export_queue.tasks().is_empty());
    let bar = s.error_bar.current().expect("error shown");
    assert!(bar.title.contains("EXPORT FAILED"), "{}", bar.title);
    assert_eq!(bar.detail, "The edit has no kept segments.");
}

#[test]
fn jobs_label_counts() {
    let (_tmp, mut s, cap) = editor_state();
    assert_eq!(export_ui::jobs_label(&s.export_queue), "Export jobs");
    export_ui::export_now(&mut s);
    assert_eq!(
        export_ui::jobs_label(&s.export_queue),
        "Export jobs · 1 pending"
    );
    cap.release.store(true, Ordering::SeqCst);
    for _ in 0..400 {
        if s.export_queue.counts().0 == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        export_ui::jobs_label(&s.export_queue),
        "Export jobs · 1 completed"
    );
}

// ---------------------------------------------------------------------------
// Guards over real sessions.

fn messages(s: &AppState) -> Vec<(String, String)> {
    s.dialogs
        .messages
        .iter()
        .chain(s.dialogs.modal.iter())
        .filter_map(|d| match d {
            DialogKind::Message { title, message }
            | DialogKind::Question { title, message, .. } => Some((title.clone(), message.clone())),
            _ => None,
        })
        .collect()
}

fn session_rig() -> Option<(tempfile::TempDir, AppState, Capture, PathBuf)> {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.tokens.reduce_motion = true;
    let home = tmp.path().to_path_buf();
    let factory: PlayerFactory = Arc::new(move |path: &Path| {
        let clock = real_clock();
        PlaybackService::open(path, &home, Box::new(NullOutput::new(clock.clone())), clock)
    });
    s.editor_factory = Some(factory);
    let dir = s.library.clips_root.join("Desktop");
    std::fs::create_dir_all(&dir).unwrap();
    let mut clips = Vec::new();
    for name in ["a.mp4", "b.mp4", "c.mp4"] {
        let p = dir.join(name);
        support::write_test_clip(&p, 2, true)?;
        s.library.upsert(&p, false);
        std::thread::sleep(Duration::from_millis(15));
        clips.push(p);
    }
    s.library.rebuild("F12");
    let cap = Capture::new();
    s.export_queue = ExportQueue::with_runner(cap.runner());
    let a = clips[1].clone();
    sessions::open(&mut s, &a);
    for _ in 0..300 {
        s.tick_editor(None);
        sessions::service(&mut s);
        if s.editor.as_ref().is_none_or(|e| e.player_loaded()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Some((tmp, s, cap, a))
}

fn request(s: &mut AppState, r: EditorRequest) {
    s.editor.as_mut().unwrap().requests.push(r);
    sessions::service(s);
}

#[test]
fn delete_park_navigate_blocked_while_exporting() {
    let Some((_tmp, mut s, _cap, a)) = session_rig() else {
        return;
    };
    // A queued export of this clip blocks deleting it.
    export_ui::export_now(&mut s);
    assert!(s.export_queue.active_for(&a));
    request(&mut s, EditorRequest::DeleteClip);
    assert_eq!(
        messages(&s),
        vec![(
            "Export in progress".to_owned(),
            "Finish or cancel this clip\u{2019}s exports before deleting the source.".to_owned()
        )]
    );
    s.dialogs.messages.clear();
    // A share in progress for the open clip keeps the editor in place.
    s.active_shares.push(a.clone());
    request(&mut s, EditorRequest::Park);
    assert_eq!(
        messages(&s),
        vec![(
            "Export in progress".to_owned(),
            "Finish or cancel the export before minimizing this editor.".to_owned()
        )]
    );
    assert!(s.editor.is_some(), "not parked");
    s.dialogs.messages.clear();
    for step in [1, -1] {
        request(&mut s, EditorRequest::Navigate(step));
        assert_eq!(s.editor.as_ref().unwrap().path, a);
    }
    assert_eq!(
        messages(&s)[0],
        (
            "Export in progress".to_owned(),
            "Finish or cancel the export before switching clips.".to_owned()
        )
    );
    // Without a share, a queued export does not keep the user in the
    // editor ("Keep browsing while your clips export").
    s.dialogs.messages.clear();
    s.active_shares.clear();
    request(&mut s, EditorRequest::Park);
    assert!(s.editor.is_none(), "parked");
    assert!(messages(&s).is_empty());
}

#[test]
fn ctrl_e_submits() {
    let (_tmp, mut s, _cap) = editor_state();
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        fthrust_ui::shell::frame(ui, &mut s);
    });
    h.key_press_modifiers(Modifiers::CTRL, Key::E);
    h.run_steps(3);
    drop(h);
    assert_eq!(s.export_queue.tasks().len(), 1);
}
