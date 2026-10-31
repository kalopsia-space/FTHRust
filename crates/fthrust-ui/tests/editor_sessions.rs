//! Editor sessions: open, park, restore, close, clip navigation, delete
//! (reference `main.py` editor sessions, notes section 7 and 9).

mod common;
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

/// What the library's rename and delete dialogs answer.
fn library_answers(r: &mut Rig, tag: String) {
    r.s.dialogs
        .results
        .push(DialogResult::Answer { tag, yes: true });
    fthrust_ui::library::page::consume_results(&mut r.s);
}

fn rename_tag(path: &Path, stem: &str) -> String {
    format!("rename-clip:{}\u{1f}{stem}", path.display())
}

fn delete_tag(path: &Path) -> String {
    format!("delete-clip:{}", path.display())
}

#[test]
fn renaming_a_clip_in_the_library_moves_its_parked_session_with_it() {
    let mut r = rig();
    let Some(a) = r.clip("a.mp4") else {
        return;
    };
    sessions::open(&mut r.s, &a);
    r.settle();
    r.park();
    assert_eq!(r.s.editor_slots.len(), 1);
    library_answers(&mut r, rename_tag(&a, "renamed"));
    let renamed = a.with_file_name("renamed.mp4");
    assert!(renamed.exists() && !a.exists());
    assert_eq!(r.s.editor_slots[0].path, renamed, "the session follows");
    assert_eq!(
        r.s.editor_sessions[0].stem, "renamed",
        "and so does its rail button"
    );
    assert!(sessions::restore(&mut r.s, 1), "{:?}", r.messages());
    r.settle();
    assert_eq!(r.s.editor.as_ref().unwrap().path, renamed);
    assert!(r.messages().is_empty(), "{:?}", r.messages());
}

#[test]
fn deleting_a_clip_in_the_library_closes_its_parked_session() {
    let mut r = rig();
    let Some(a) = r.clip("a.mp4") else {
        return;
    };
    sessions::open(&mut r.s, &a);
    r.settle();
    r.park();
    assert_eq!(r.s.editor_sessions.len(), 1);
    library_answers(&mut r, delete_tag(&a));
    assert!(!a.exists());
    assert!(r.s.editor_slots.is_empty(), "no session for a deleted clip");
    assert!(r.s.editor_sessions.is_empty(), "no rail button either");
    let store = fthrust_core::clip_metadata::ClipMetadataStore::open(&r.s.paths).unwrap();
    assert!(store.get(&a).unwrap().editor_draft.is_none());
}

/// A queue whose exports run until they are cancelled.
fn endless_queue() -> fthrust_ui::services::export::ExportQueue {
    use fthrust_media::export::{ExportResult, ExportState};
    fthrust_ui::services::export::ExportQueue::with_runner(Arc::new(move |_job, cancel, _, _| {
        while !cancel.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        ExportResult {
            state: ExportState::Completed,
            detail: String::new(),
        }
    }))
}

#[test]
fn a_clip_under_export_is_neither_renamed_nor_deleted_from_the_library() {
    let mut r = rig();
    let Some(a) = r.clip("a.mp4") else {
        return;
    };
    r.s.export_queue = endless_queue();
    sessions::open(&mut r.s, &a);
    r.settle();
    fthrust_ui::editor::export_ui::export_now(&mut r.s);
    assert!(r.s.export_queue.active_for(&a), "an export is queued");
    r.park();
    // Rename.
    library_answers(&mut r, rename_tag(&a, "renamed"));
    assert!(a.exists(), "the source stays where the export reads it");
    assert!(!a.with_file_name("renamed.mp4").exists());
    assert_eq!(
        r.messages(),
        vec![(
            "Export in progress".to_owned(),
            "Finish or cancel this clip\u{2019}s exports before renaming it.".to_owned()
        )]
    );
    r.s.dialogs.messages.clear();
    // Delete.
    library_answers(&mut r, delete_tag(&a));
    assert!(a.exists(), "the source is kept");
    assert_eq!(r.s.editor_slots.len(), 1, "the session stays");
    assert_eq!(
        r.messages(),
        vec![(
            "Export in progress".to_owned(),
            "Finish or cancel this clip\u{2019}s exports before deleting the source.".to_owned()
        )]
    );
    // A share in progress counts the same.
    r.s.dialogs.messages.clear();
    r.s.export_queue.shutdown(Duration::from_secs(5));
    assert!(!r.s.export_queue.active_for(&a), "the export was cancelled");
    r.s.active_shares.push(a.clone());
    library_answers(&mut r, delete_tag(&a));
    assert!(a.exists(), "a running share keeps the source too");
    r.s.active_shares.clear();
    // Once nothing runs the rename goes through.
    library_answers(&mut r, rename_tag(&a, "renamed"));
    assert!(a.with_file_name("renamed.mp4").exists());
}

/// A fake uploader that records the upload and says the clip may go.
fn deleting_uploader(dir: &Path, history: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let exe = dir.join("uploader");
    let body = format!(
        r#"#!/bin/sh
read -r line
path=$(echo "$line" | sed 's/.*"path":"\([^"]*\)".*/\1/')
mkdir -p "$(dirname "{history}")"
printf '{{"%s": {{"status": "ok", "provider": "custom", "url": "https://cdn.example/x", "raw_url": "https://cdn.example/x"}}}}' "$path" > "{history}"
echo '{{"ok":true,"message":"https://cdn.example/x","delete":true}}'
"#,
        history = history.display()
    );
    std::fs::write(&exe, body).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

/// An upload's auto-delete meets the library's rules when the upload ends,
/// not when it began, and keeps away from what the user has open: a clip that
/// an export still reads is kept, and so is one that any editor session holds,
/// open or parked in the rail, with its draft (the bar says so either way,
/// since the editor draws none). A clip nothing holds is deleted. The bin
/// button, pressed by the user, still closes the sessions of the clip.
#[test]
fn an_uploads_auto_delete_follows_the_librarys_rules_and_keeps_open_clips() {
    use fthrust_core::clip_metadata::{DraftFile, EditorDraft};
    use fthrust_ui::services::upload::UploadService;
    let mut r = rig();
    let (Some(a), Some(p), Some(b), Some(c)) = (
        r.clip("a.mp4"),
        r.clip("p.mp4"),
        r.clip("b.mp4"),
        r.clip("c.mp4"),
    ) else {
        return;
    };
    for (k, v) in [
        ("upload_enabled", serde_json::json!(true)),
        ("upload_provider", serde_json::json!("custom")),
        ("upload_auto_delete", serde_json::json!(true)),
    ] {
        r.s.uploader_settings.set(k, v);
    }
    let exe = deleting_uploader(r._tmp.path(), &r.s.paths.upload_history_file());
    let mut uploads = UploadService::with_exe(Some(exe));
    // `a` is in the rail with an export queued behind another job, `p` is in
    // the rail, `b` is the open editor, with a draft; `c` is open nowhere.
    r.s.export_queue = endless_queue();
    sessions::open(&mut r.s, &a);
    r.settle();
    fthrust_ui::editor::export_ui::export_now(&mut r.s);
    assert!(r.s.export_queue.active_for(&a), "an export is queued");
    r.park();
    sessions::open(&mut r.s, &p);
    r.settle();
    r.park();
    sessions::open(&mut r.s, &b);
    r.settle();
    let identity = fthrust_core::media_identity::file_identity(&b).unwrap();
    sessions::store(&mut r.s)
        .unwrap()
        .set_editor_draft(
            &b,
            EditorDraft {
                version: 1,
                file: DraftFile {
                    mtime_ns: identity.mtime_ns,
                    size: identity.size,
                },
                updated_at: 1.0,
                state: serde_json::json!({"trim_start": 0.25}),
            },
        )
        .unwrap();
    assert_eq!(r.s.editor_slots.len(), 3);
    // Upload is pressed for all four.
    for clip in [&a, &p, &b, &c] {
        uploads.enqueue(&mut r.s, clip.clone());
    }
    let mut now = 0.0;
    while uploads.busy() {
        uploads.tick(&mut r.s, now);
        now += 0.02;
        std::thread::sleep(Duration::from_millis(20));
        assert!(now < 60.0, "the uploads never finished");
    }
    uploads.tick(&mut r.s, now);
    // Everything that was open or exporting is kept, library entry and all.
    for clip in [&a, &p, &b] {
        assert!(clip.exists(), "{} was deleted", clip.display());
        assert!(r.s.library.record(clip).is_some());
    }
    assert_eq!(r.s.editor_slots.len(), 3, "a session was closed");
    assert_eq!(
        r.s.editor.as_ref().map(|e| e.path.clone()),
        Some(b.clone()),
        "the open editor was closed"
    );
    let store = fthrust_core::clip_metadata::ClipMetadataStore::open(&r.s.paths).unwrap();
    assert!(store.get(&b).unwrap().editor_draft.is_some(), "the draft");
    // The bar says why, once for each.
    let details: Vec<String> =
        r.s.error_bar
            .entries()
            .iter()
            .map(|e| {
                assert_eq!(e.title, "UPLOAD WARNING");
                e.detail.clone()
            })
            .collect();
    assert_eq!(
        details,
        vec![
            "a.mp4 was uploaded, but not deleted: an export or share of it is queued or running. Delete it yourself when that has finished.",
            "p.mp4 was uploaded, but not deleted: it is open in the editor. Delete it yourself once you have closed it.",
            "b.mp4 was uploaded, but not deleted: it is open in the editor. Delete it yourself once you have closed it.",
        ]
    );
    // The clip nothing holds is deleted.
    assert!(!c.exists(), "the free clip is still there");
    assert!(r.s.library.record(&c).is_none());
    // The user's own delete closes the sessions of the clip, as ever.
    library_answers(&mut r, delete_tag(&p));
    assert!(!p.exists());
    assert_eq!(r.s.editor_slots.len(), 2, "p's session is still open");
    assert!(r.s.editor_slots.iter().all(|s| s.path != p));
    r.s.export_queue.shutdown(Duration::from_secs(5));
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

// ---------------------------------------------------------------------------
// Keyboard shortcuts (notes section 8 "Keys").

use egui::{Key, Modifiers, Vec2};

/// A harness over the whole shell that also ticks the editor.
fn keys_harness(s: &mut AppState) -> egui_kittest::Harness<'_> {
    let tokens = s.tokens.clone();
    common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, move |ui| {
        s.tick_editor(None);
        fthrust_ui::shell::frame(ui, s);
    })
}

fn press(h: &mut egui_kittest::Harness<'_>, mods: Modifiers, key: Key) {
    h.key_press_modifiers(mods, key);
    h.run_steps(2);
}

/// winit turns Ctrl+C into a Copy event and no key event.
fn copy(h: &mut egui_kittest::Harness<'_>) {
    h.event(egui::Event::Copy);
    h.run_steps(2);
}

/// Moves the playhead to the middle the way the app does (a debounced
/// seek the next ticks apply).
fn seek_half(r: &mut Rig) {
    let now = r.s.editor_now();
    r.s.editor.as_mut().unwrap().scrub_to(0.5, now);
    std::thread::sleep(Duration::from_millis(80));
    r.s.tick_editor(None);
}

fn opened(r: &mut Rig) -> Option<PathBuf> {
    let a = r.clip("a.mp4")?;
    sessions::open(&mut r.s, &a);
    r.settle();
    seek_half(r);
    Some(a)
}

#[test]
fn default_bindings_drive_the_editor() {
    use fthrust_ui::editor::Lifecycle;
    let mut r = rig();
    if opened(&mut r).is_none() {
        return;
    }
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::NONE, Key::Space);
    drop(h);
    assert_eq!(r.s.editor.as_ref().unwrap().lifecycle, Lifecycle::Playing);
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::NONE, Key::Space);
    drop(h);
    assert_eq!(r.s.editor.as_ref().unwrap().lifecycle, Lifecycle::Paused);
    seek_half(&mut r);
    let mut h = keys_harness(&mut r.s);
    copy(&mut h);
    drop(h);
    assert_eq!(
        r.s.editor.as_ref().unwrap().timeline.segments.len(),
        2,
        "Ctrl+C splits"
    );
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::CTRL, Key::D);
    drop(h);
    assert!(
        r.s.editor.as_ref().unwrap().timeline.segments[1].deleted,
        "Ctrl+D deletes"
    );
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::CTRL, Key::W);
    drop(h);
    assert!(
        !r.s.editor.as_ref().unwrap().timeline.segments[1].deleted,
        "Ctrl+W restores"
    );
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::CTRL, Key::Z);
    drop(h);
    assert!(
        r.s.editor.as_ref().unwrap().timeline.segments[1].deleted,
        "Ctrl+Z undoes the restore"
    );
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::CTRL, Key::Y);
    drop(h);
    assert!(
        !r.s.editor.as_ref().unwrap().timeline.segments[1].deleted,
        "Ctrl+Y redoes"
    );
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::CTRL, Key::Z);
    press(&mut h, Modifiers::CTRL | Modifiers::SHIFT, Key::Z);
    drop(h);
    assert!(
        !r.s.editor.as_ref().unwrap().timeline.segments[1].deleted,
        "Ctrl+Shift+Z redoes"
    );
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::SHIFT, Key::Escape);
    drop(h);
    assert!(r.s.editor.is_none(), "Shift+Escape parks");
    assert_eq!(r.s.editor_sessions.len(), 1);
    assert!(sessions::restore(&mut r.s, 1));
    r.settle();
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::NONE, Key::Escape);
    drop(h);
    assert!(r.s.editor.is_none(), "Escape closes");
    assert!(r.s.editor_sessions.is_empty());
}

/// Runs `f` and returns what it logged at WARN or above, one string per event.
fn logged_warnings(f: impl FnOnce()) -> Vec<String> {
    use std::sync::Mutex;
    use tracing_subscriber::fmt::MakeWriter;
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> MakeWriter<'a> for Sink {
        type Writer = Sink;
        fn make_writer(&'a self) -> Sink {
            self.clone()
        }
    }
    let sink = Sink::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(sink.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let text = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
    text.lines().map(str::to_owned).collect()
}

#[test]
fn an_unavailable_clip_metadata_database_is_retried_on_a_timer_and_logged_once() {
    use fthrust_ui::editor::{Editor, Lifecycle};
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.editor = Some(Editor::for_tests(
        &tmp.path().join("clip.mp4"),
        10_000,
        Lifecycle::Paused,
    ));
    // A directory where the database file belongs: it cannot be opened.
    let db = s.paths.clip_metadata_db();
    std::fs::create_dir_all(&db).unwrap();
    let warnings = logged_warnings(|| {
        for _ in 0..120 {
            s.tick_editor(None);
        }
    });
    assert!(s.clip_metadata.is_none());
    assert_eq!(
        warnings.len(),
        1,
        "two seconds of frames, one log line (first: {:?})",
        warnings.first()
    );
    // The cause goes away. The next frame does not look again...
    std::fs::remove_dir(&db).unwrap();
    s.tick_editor(None);
    assert!(s.clip_metadata.is_none(), "not retried every frame");
    // ...the retry does, a few seconds later.
    s.editor_clock = std::time::Instant::now() - Duration::from_secs(30);
    s.tick_editor(None);
    assert!(s.clip_metadata.is_some(), "retried after the interval");
}

#[test]
fn the_same_metadata_failure_is_not_logged_again_by_user_actions() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    std::fs::create_dir_all(s.paths.clip_metadata_db()).unwrap();
    let warnings = logged_warnings(|| {
        for _ in 0..20 {
            assert!(sessions::store(&mut s).is_none());
        }
    });
    assert_eq!(
        warnings.len(),
        1,
        "one log line for 20 attempts (first: {:?})",
        warnings.first()
    );
}

#[test]
fn opening_a_clip_says_so_when_the_clip_details_cannot_be_opened() {
    let mut r = rig();
    let Some(a) = r.clip("a.mp4") else {
        return;
    };
    // A directory where the details database belongs: it cannot be opened.
    let db = r.s.paths.clip_metadata_db();
    std::fs::create_dir_all(&db).unwrap();
    assert!(!sessions::open(&mut r.s, &a));
    let messages = r.messages();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].0, "Could not open clip");
    assert!(
        messages[0].1.contains("cannot start") && messages[0].1.contains("unable to open"),
        "the message says what and why: {}",
        messages[0].1
    );
    assert!(r.s.editor.is_none() && r.s.editor_slots.is_empty());
    // Once the database can be opened, the same click works.
    std::fs::remove_dir(&db).unwrap();
    assert!(sessions::open(&mut r.s, &a), "{:?}", r.messages());
}

#[test]
fn the_details_panel_does_not_open_the_database_every_frame() {
    use fthrust_ui::editor::{Editor, Lifecycle};
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.tokens.reduce_motion = true;
    s.editor = Some(Editor::for_tests(
        &tmp.path().join("clip.mp4"),
        10_000,
        Lifecycle::Paused,
    ));
    // CLIP DETAILS open, and a directory where the database belongs.
    s.editor_panels[4] = true;
    std::fs::create_dir_all(s.paths.clip_metadata_db()).unwrap();
    let tokens = s.tokens.clone();
    let warnings = logged_warnings(|| {
        let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
            fthrust_ui::shell::frame(ui, &mut s);
        });
        h.run_steps(60);
    });
    assert!(s.clip_metadata.is_none());
    // One look, one log line; it used to look on every one of the frames
    // (and say nothing).
    assert_eq!(
        warnings.len(),
        1,
        "one log line for 60 frames (first: {:?})",
        warnings.first()
    );
}

#[test]
fn closing_the_editor_frees_its_frame_even_when_its_details_cannot_be_saved() {
    use fthrust_ui::editor::{Editor, Lifecycle};
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.editor = Some(Editor::for_tests(
        &tmp.path().join("clip.mp4"),
        10_000,
        Lifecycle::Paused,
    ));
    s.editor_video = Some(fthrust_ui::video_texture::VideoTexture::new());
    // A directory where the details database belongs: the editor's state
    // cannot be saved, and the close used to stop there with the frame (and,
    // with Live edit preview on, the preview's bind group on it) still held.
    std::fs::create_dir_all(s.paths.clip_metadata_db()).unwrap();
    sessions::shutdown(&mut s);
    assert!(s.editor.is_none(), "the editor is gone");
    assert!(
        s.editor_video.is_none(),
        "the editor's video frame is still held"
    );
}

/// Ticks the editor with `rs` until its first video frame is a texture.
fn first_video_texture(r: &mut Rig, rs: &egui_wgpu::RenderState) -> Option<egui::TextureId> {
    for _ in 0..500 {
        r.s.tick_editor(Some(rs));
        sessions::service(&mut r.s);
        if let Some(id) = r.s.editor_video.as_ref().and_then(|v| v.id()) {
            return Some(id);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

#[test]
fn closing_or_parking_an_editor_frees_its_video_texture() {
    let Some(rs) = common::headless_render_state() else {
        eprintln!("skipping: no wgpu adapter");
        return;
    };
    let mut r = rig();
    let Some(a) = r.clip("a.mp4") else {
        return;
    };
    sessions::open(&mut r.s, &a);
    let id = first_video_texture(&mut r, &rs).expect("a video frame reached the texture");
    assert!(rs.renderer.read().texture(&id).is_some());
    // Parking releases the player and the texture.
    sessions::park(&mut r.s);
    assert!(r.s.editor_video.is_none());
    assert!(
        rs.renderer.read().texture(&id).is_none(),
        "the parked editor's texture is still registered"
    );
    // Restoring builds a new one; closing frees that.
    assert!(sessions::restore(&mut r.s, 1));
    let id = first_video_texture(&mut r, &rs).expect("a video frame after restoring");
    sessions::close(&mut r.s, 1);
    assert!(r.s.editor_video.is_none());
    assert!(
        rs.renderer.read().texture(&id).is_none(),
        "the closed editor's texture is still registered"
    );
}

#[test]
fn escape_in_the_share_picker_closes_the_picker_not_the_editor() {
    let mut r = rig();
    if opened(&mut r).is_none() {
        return;
    }
    fthrust_ui::editor::share::open_picker(&mut r.s);
    assert!(r.s.share_picker.is_some());
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::NONE, Key::Escape);
    drop(h);
    assert!(r.s.share_picker.is_none(), "Escape closes the picker");
    assert!(r.s.editor.is_some(), "and leaves the editor open");
    // With the picker gone Escape is the editor's again.
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::NONE, Key::Escape);
    drop(h);
    assert!(r.s.editor.is_none(), "the next Escape closes the editor");
}

#[test]
fn editor_shortcuts_wait_while_the_share_picker_is_open() {
    use fthrust_ui::editor::Lifecycle;
    let mut r = rig();
    if opened(&mut r).is_none() {
        return;
    }
    fthrust_ui::editor::share::open_picker(&mut r.s);
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::NONE, Key::Space);
    drop(h);
    assert_ne!(
        r.s.editor.as_ref().unwrap().lifecycle,
        Lifecycle::Playing,
        "Space does not play behind the picker"
    );
}

#[test]
fn escape_leaves_fullscreen_first() {
    let mut r = rig();
    if opened(&mut r).is_none() {
        return;
    }
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::NONE, Key::F11);
    drop(h);
    assert!(r.s.window.fullscreen, "F11 goes fullscreen");
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::NONE, Key::Escape);
    drop(h);
    assert!(
        r.s.editor.is_some(),
        "the first Escape only leaves fullscreen"
    );
    assert!(!r.s.window.fullscreen);
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::NONE, Key::Escape);
    drop(h);
    assert!(r.s.editor.is_none());
}

#[test]
fn shortcuts_ignored_while_typing() {
    let mut r = rig();
    if opened(&mut r).is_none() {
        return;
    }
    {
        let now = r.s.editor_now();
        let e = r.s.editor.as_mut().unwrap();
        e.split(now);
    }
    r.s.editor_panels = [false, false, false, false, true, false];
    let tokens = r.s.tokens.clone();
    let tag = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        fthrust_ui::shell::frame(ui, &mut r.s);
    })
    .marker("detailEdit[e.g. Gaming, Highlight …]")
    .unwrap()
    .rect
    .center();
    let mut h = keys_harness(&mut r.s);
    h.drag_at(tag);
    h.step();
    h.drop_at(tag);
    h.run_steps(2);
    press(&mut h, Modifiers::CTRL, Key::D);
    drop(h);
    assert!(!r.s.editor.as_ref().unwrap().timeline.segments[1].deleted);
}

#[test]
fn disabled_category_disables_keys() {
    use fthrust_ui::editor::Lifecycle;
    let mut r = rig();
    if opened(&mut r).is_none() {
        return;
    }
    r.s.nav.set_category_enabled("Clip editor", false);
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::NONE, Key::Space);
    press(&mut h, Modifiers::NONE, Key::F11);
    drop(h);
    assert_ne!(r.s.editor.as_ref().unwrap().lifecycle, Lifecycle::Playing);
    assert!(!r.s.window.fullscreen);
}

#[test]
fn rebinding_is_respected() {
    let mut r = rig();
    if opened(&mut r).is_none() {
        return;
    }
    r.s.nav.set("editor_split", "Ctrl+K").unwrap();
    let mut h = keys_harness(&mut r.s);
    copy(&mut h);
    drop(h);
    assert_eq!(
        r.s.editor.as_ref().unwrap().timeline.segments.len(),
        1,
        "Ctrl+C is free now"
    );
    let mut h = keys_harness(&mut r.s);
    press(&mut h, Modifiers::CTRL, Key::K);
    drop(h);
    assert_eq!(r.s.editor.as_ref().unwrap().timeline.segments.len(), 2);
}

// ---------------------------------------------------------------------------
// Crop and stretch dialogs.

/// A shell harness that reports, after every frame, whether the open
/// dialog's frame has decoded.
fn dialog_harness<'a>(
    s: &'a mut AppState,
    loaded: Arc<std::sync::atomic::AtomicBool>,
) -> egui_kittest::Harness<'a> {
    let tokens = s.tokens.clone();
    common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, move |ui| {
        s.tick_editor(None);
        fthrust_ui::shell::frame(ui, s);
        loaded.store(
            fthrust_ui::editor::dialogs::frame_loaded(s),
            Ordering::SeqCst,
        );
    })
}

fn wait_loaded(h: &mut egui_kittest::Harness<'_>, loaded: &std::sync::atomic::AtomicBool) {
    for _ in 0..300 {
        h.step();
        if loaded.load(Ordering::SeqCst) {
            h.run_steps(2);
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("dialog frame never decoded");
}

fn drag(h: &mut egui_kittest::Harness<'_>, from: egui::Pos2, to: egui::Pos2) {
    h.drag_at(from);
    h.step();
    for i in 1..=5 {
        h.hover_at(from + (to - from) * (i as f32 / 5.0));
        h.step();
    }
    h.drop_at(to);
    h.run_steps(2);
}

fn click_at(h: &mut egui_kittest::Harness<'_>, at: egui::Pos2) {
    h.drag_at(at);
    h.step();
    h.drop_at(at);
    h.run_steps(2);
}

#[test]
fn crop_accept_is_one_undo_step_and_pauses() {
    use fthrust_ui::editor::Lifecycle;
    let mut r = rig();
    if opened(&mut r).is_none() {
        return;
    }
    let now = r.s.editor_now();
    {
        let e = r.s.editor.as_mut().unwrap();
        e.toggle_play(now);
        assert_eq!(e.lifecycle, Lifecycle::Playing);
        e.requests.push(EditorRequest::OpenCrop);
    }
    let loaded = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut h = dialog_harness(&mut r.s, loaded.clone());
    wait_loaded(&mut h, &loaded);
    // The 900×580 dialog is centred: its frame area spans y 228..692.
    drag(
        &mut h,
        egui::Pos2::new(400.0, 300.0),
        egui::Pos2::new(700.0, 500.0),
    );
    let apply = common::render_harness(&mut h)
        .marker("QPushButton[Apply crop]")
        .expect("apply button")
        .rect
        .center();
    click_at(&mut h, apply);
    drop(h);
    let e = r.s.editor.as_ref().unwrap();
    assert!(e.crop.is_some(), "crop applied");
    assert_eq!(e.undo.undo.len(), 1);
    assert_eq!(e.lifecycle, Lifecycle::Playing, "playback resumes");
    assert!(r.s.editor_dialog.is_none());
    // Escape cancels without an undo step.
    r.s.editor
        .as_mut()
        .unwrap()
        .requests
        .push(EditorRequest::OpenCrop);
    let mut h = dialog_harness(&mut r.s, loaded.clone());
    wait_loaded(&mut h, &loaded);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    drop(h);
    assert!(r.s.editor_dialog.is_none());
    assert!(
        r.s.editor.is_some(),
        "Escape closed the dialog, not the editor"
    );
    assert_eq!(r.s.editor.as_ref().unwrap().undo.undo.len(), 1);
}

#[test]
fn stretch_ratio_clamps() {
    let mut r = rig();
    if opened(&mut r).is_none() {
        return;
    }
    r.s.editor
        .as_mut()
        .unwrap()
        .requests
        .push(EditorRequest::OpenStretch);
    let loaded = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1440.0, 900.0));
    let mut h = dialog_harness(&mut r.s, loaded.clone());
    wait_loaded(&mut h, &loaded);
    drop(h);
    for _ in 0..3 {
        let frame = fthrust_ui::editor::dialogs::stretch_frame_rect(&r.s, screen).unwrap();
        let mut h = dialog_harness(&mut r.s, loaded.clone());
        let y = frame.center().y;
        drag(
            &mut h,
            egui::Pos2::new(frame.max.x, y),
            egui::Pos2::new(1439.0, y),
        );
        drop(h);
    }
    let apply_x = {
        let mut h = dialog_harness(&mut r.s, loaded.clone());
        let c = common::render_harness(&mut h)
            .marker("stretchDialogApply")
            .unwrap()
            .rect
            .center();
        click_at(&mut h, c);
        c
    };
    let _ = apply_x;
    let e = r.s.editor.as_ref().unwrap();
    assert_eq!(e.stretch, 4.0);
    assert_eq!(e.undo.undo.len(), 1);
}

#[test]
fn quitting_flushes_the_active_editor() {
    let mut r = rig();
    let Some(a) = opened(&mut r) else {
        return;
    };
    {
        let now = r.s.editor_now();
        let e = r.s.editor.as_mut().unwrap();
        assert!(e.split(now), "an edit still inside the 450 ms debounce");
        e.tag = "Highlight".into();
        e.details_dirty = true;
    }
    let paths = r.s.paths.clone();
    let ctx = egui::Context::default();
    let mut app = fthrust_ui::App::new(&ctx, r.s);
    app.on_exit();
    drop(app);
    let store = fthrust_core::clip_metadata::ClipMetadataStore::open(&paths).unwrap();
    let entry = store.get(&a).unwrap();
    assert!(
        entry.editor_draft.is_some(),
        "the pending draft was written"
    );
    assert_eq!(entry.tag, "Highlight");
}
