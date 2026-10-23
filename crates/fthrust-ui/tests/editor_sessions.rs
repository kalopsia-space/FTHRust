//! Editor sessions: open, park, restore, close, clip navigation, delete
//! (reference `main.py` editor sessions, notes section 7 and 9).

#[path = "../../fthrust-media/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fthrust_ui::editor::sessions;
use fthrust_ui::editor::{EditorRequest, PlayerFactory};
use fthrust_ui::services::playback::audio_out::{AudioOutput, NullOutput, PcmQueue};
use fthrust_ui::services::playback::{PlaybackService, real_clock};
use fthrust_ui::state::{AppState, DialogKind, DialogResult};

/// A null output that counts live players.
struct Counting {
    inner: NullOutput,
    alive: Arc<AtomicUsize>,
}

impl Drop for Counting {
    fn drop(&mut self) {
        self.alive.fetch_sub(1, Ordering::SeqCst);
    }
}

impl AudioOutput for Counting {
    fn start(&mut self, queue: Arc<PcmQueue>) -> Result<(), String> {
        self.inner.start(queue)
    }
    fn suspend(&mut self) {
        self.inner.suspend();
    }
    fn resume(&mut self) {
        self.inner.resume();
    }
    fn buffered_frames(&self) -> usize {
        self.inner.buffered_frames()
    }
    fn stop(&mut self) {
        self.inner.stop();
    }
    fn as_null_mut(&mut self) -> Option<&mut NullOutput> {
        Some(&mut self.inner)
    }
}

struct Rig {
    _tmp: tempfile::TempDir,
    s: AppState,
    alive: Arc<AtomicUsize>,
}

fn rig() -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.tokens.reduce_motion = true;
    let alive = Arc::new(AtomicUsize::new(0));
    let counter = alive.clone();
    let home = tmp.path().to_path_buf();
    let factory: PlayerFactory = Arc::new(move |path: &Path| {
        let clock = real_clock();
        counter.fetch_add(1, Ordering::SeqCst);
        let out = Counting {
            inner: NullOutput::new(clock.clone()),
            alive: counter.clone(),
        };
        PlaybackService::open(path, &home, Box::new(out), clock)
    });
    s.editor_factory = Some(factory);
    Rig {
        _tmp: tmp,
        s,
        alive,
    }
}

impl Rig {
    /// Writes a clip into the library (newest last by mtime).
    fn clip(&mut self, name: &str) -> Option<PathBuf> {
        let dir = self.s.library.clips_root.join("Desktop");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        support::write_test_clip(&p, 2, true)?;
        self.s.library.upsert(&p, false);
        self.s.library.rebuild("F12");
        std::thread::sleep(Duration::from_millis(15));
        Some(p)
    }

    fn settle(&mut self) {
        for _ in 0..300 {
            self.s.tick_editor(None);
            sessions::service(&mut self.s);
            if self.s.editor.as_ref().is_none_or(|e| e.player_loaded()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn messages(&self) -> Vec<(String, String)> {
        self.s
            .dialogs
            .messages
            .iter()
            .chain(self.s.dialogs.modal.iter())
            .filter_map(|d| match d {
                DialogKind::Message { title, message }
                | DialogKind::Question { title, message, .. } => {
                    Some((title.clone(), message.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn park(&mut self) {
        self.s
            .editor
            .as_mut()
            .unwrap()
            .requests
            .push(EditorRequest::Park);
        sessions::service(&mut self.s);
    }
}

#[test]
fn fourth_session_is_refused() {
    let mut r = rig();
    let mut clips = Vec::new();
    for n in ["a.mp4", "b.mp4", "c.mp4", "d.mp4"] {
        let Some(p) = r.clip(n) else {
            return;
        };
        clips.push(p);
    }
    for p in &clips[..3] {
        assert!(sessions::open(&mut r.s, p));
        r.settle();
        r.park();
    }
    assert_eq!(r.s.editor_sessions.len(), 3);
    assert!(!sessions::open(&mut r.s, &clips[3]));
    assert!(r.messages().contains(&(
        "Three editors are open".into(),
        "Close a sidebar editor before opening another clip.".into()
    )));
    assert_eq!(r.s.editor_sessions.len(), 3);
    assert!(r.s.editor.is_none());
}

#[test]
fn reopening_a_parked_clip_restores_it() {
    let mut r = rig();
    let (Some(a), Some(b)) = (r.clip("a.mp4"), r.clip("b.mp4")) else {
        return;
    };
    sessions::open(&mut r.s, &a);
    r.settle();
    {
        let now = r.s.editor_now();
        let e = r.s.editor.as_mut().unwrap();
        e.timeline.playhead = 0.5;
        assert!(e.split(now));
    }
    r.park();
    sessions::open(&mut r.s, &b);
    r.settle();
    r.park();
    assert!(sessions::open(&mut r.s, &a));
    r.settle();
    let e = r.s.editor.as_ref().unwrap();
    assert_eq!(e.path, a);
    assert_eq!(e.timeline.segments.len(), 2);
    assert_eq!(e.undo.undo.len(), 1, "undo history survives parking");
    assert_eq!(r.s.editor_sessions.len(), 2);
}

#[test]
fn restore_of_deleted_clip_removes_session() {
    let mut r = rig();
    let Some(a) = r.clip("a.mp4") else {
        return;
    };
    sessions::open(&mut r.s, &a);
    r.settle();
    r.park();
    std::fs::remove_file(&a).unwrap();
    assert!(!sessions::restore(&mut r.s, 1));
    assert!(r.messages().contains(&(
        "CLIP NOT FOUND".into(),
        "The editor clip is no longer available.".into()
    )));
    assert!(r.s.editor_sessions.is_empty());
}

#[test]
fn opening_a_missing_file_reports_and_rescans() {
    let mut r = rig();
    let missing = r.s.library.clips_root.join("gone.mp4");
    assert!(!sessions::open(&mut r.s, &missing));
    assert!(r.messages().contains(&(
        "CLIP NOT FOUND".into(),
        "gone.mp4 was moved or deleted outside the app.".into()
    )));
    assert!(r.s.scan_clips_requested);
}

#[test]
fn navigation_moves_within_library_without_wrap() {
    let mut r = rig();
    let (Some(a), Some(b), Some(c)) = (r.clip("a.mp4"), r.clip("b.mp4"), r.clip("c.mp4")) else {
        return;
    };
    let order = r.s.library.navigation_paths(false);
    assert_eq!(order.len(), 3);
    let last = order[2].clone();
    sessions::open(&mut r.s, &last);
    r.settle();
    sessions::navigate(&mut r.s, 1);
    assert_eq!(
        r.s.editor.as_ref().unwrap().path,
        last,
        "no wrap at the end"
    );
    sessions::navigate(&mut r.s, -1);
    r.settle();
    assert_eq!(r.s.editor.as_ref().unwrap().path, order[1]);
    assert_eq!(
        r.s.editor_sessions.len(),
        1,
        "the same session follows the clip"
    );
    let _ = (a, b, c);
}

#[test]
fn rapid_clip_navigation_keeps_one_player() {
    let mut r = rig();
    let mut clips = Vec::new();
    for n in ["a.mp4", "b.mp4", "c.mp4", "d.mp4", "e.mp4", "f.mp4"] {
        let Some(p) = r.clip(n) else {
            return;
        };
        clips.push(p);
    }
    let order = r.s.library.navigation_paths(false);
    sessions::open(&mut r.s, &order[0]);
    for _ in 0..5 {
        sessions::navigate(&mut r.s, 1);
    }
    r.settle();
    std::thread::sleep(Duration::from_millis(300));
    r.settle();
    assert_eq!(r.s.editor.as_ref().unwrap().path, order[5]);
    assert_eq!(r.alive.load(Ordering::SeqCst), 1, "players alive");
}

#[test]
fn delete_owned_clip_confirms_and_clears_draft() {
    let mut r = rig();
    let Some(a) = r.clip("a.mp4") else {
        return;
    };
    sessions::open(&mut r.s, &a);
    r.settle();
    {
        let now = r.s.editor_now();
        let e = r.s.editor.as_mut().unwrap();
        e.timeline.playhead = 0.5;
        e.split(now);
        e.requests.push(EditorRequest::DeleteClip);
    }
    sessions::service(&mut r.s);
    let question = r.s.dialogs.modal.iter().find_map(|d| match d {
        DialogKind::Question {
            title,
            message,
            tag,
        } => Some((title.clone(), message.clone(), tag.clone())),
        _ => None,
    });
    let (title, message, tag) = question.expect("a confirmation");
    assert_eq!(title, "Delete Clip");
    assert_eq!(message, "Delete a.mp4?\nThis cannot be undone.");
    r.s.dialogs.modal.clear();
    r.s.dialogs
        .results
        .push(DialogResult::Answer { tag, yes: true });
    sessions::service(&mut r.s);
    assert!(!a.exists());
    assert!(r.s.editor.is_none());
    assert!(r.s.editor_sessions.is_empty());
    let store = fthrust_core::clip_metadata::ClipMetadataStore::open(&r.s.paths).unwrap();
    assert!(store.get(&a).unwrap().editor_draft.is_none());
}

#[test]
fn delete_of_linked_import_is_refused() {
    let mut r = rig();
    let imports = r.s.paths.root().parent().unwrap().join("imports");
    std::fs::create_dir_all(&imports).unwrap();
    r.s.settings.set(
        "imported_clip_folders",
        serde_json::json!([imports.to_string_lossy()]),
    );
    let p = imports.join("linked.mp4");
    if support::write_test_clip(&p, 2, true).is_none() {
        return;
    }
    sessions::open(&mut r.s, &p);
    r.settle();
    r.s.editor
        .as_mut()
        .unwrap()
        .requests
        .push(EditorRequest::DeleteClip);
    sessions::service(&mut r.s);
    assert!(r.messages().contains(&(
        "Linked Original Protected".into(),
        "This clip is linked from another folder. FTHR will not delete the original with the generic Delete action.".into()
    )));
    assert!(p.exists());
}
