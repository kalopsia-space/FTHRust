//! Editor sessions (`main.py` `_add_editor_session`, `_park_active_editor`,
//! `_restore_editor_session`, `_navigate_editor_clip`; notes section 7):
//! one active editor plus parked ones, at most three in all, each shown as
//! a rail button. Parking keeps the session state and releases the player.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fthrust_core::clip_metadata::ClipMetadataStore;
use fthrust_core::library_ownership::{Ownership, classify_media_path};

use super::{Editor, EditorRequest, PlayerFactory, SessionState};
use crate::services::playback::audio_out::PipeWireOutput;
use crate::services::playback::{PlaybackService, real_clock};
use crate::state::{AppState, DialogKind, DialogResult, Page, SessionSummary};

/// `MAX_EDITOR_SESSIONS`.
pub const MAX_SESSIONS: usize = 3;
/// Dialog tag prefix of the delete confirmation.
const DELETE_TAG: &str = "editor-delete:";
/// Title of the export guards.
const EXPORT_BUSY: &str = "Export in progress";

/// One editor session.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    /// The clip.
    pub path: PathBuf,
    /// Parked state (`None` while active).
    pub state: Option<SessionState>,
}

/// The real player: PipeWire output, wall clock, proxy cache under `home`.
pub fn default_factory(home: PathBuf) -> PlayerFactory {
    Arc::new(move |path: &Path| {
        PlaybackService::open(path, &home, Box::new(PipeWireOutput::new()), real_clock())
    })
}

fn factory(state: &mut AppState) -> PlayerFactory {
    state
        .editor_factory
        .get_or_insert_with(|| {
            // `~/.fthr`'s parent: the playback proxy cache lives under it.
            let root = state.paths.root();
            default_factory(root.parent().map_or(root.clone(), Path::to_path_buf))
        })
        .clone()
}

fn store(state: &mut AppState) -> Option<&mut ClipMetadataStore> {
    if state.clip_metadata.is_none() {
        match ClipMetadataStore::open(&state.paths) {
            Ok(s) => state.clip_metadata = Some(s),
            Err(e) => {
                tracing::warn!(error = %e, "clip metadata store unavailable");
                return None;
            }
        }
    }
    state.clip_metadata.as_mut()
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b || (a.canonicalize().ok().is_some() && a.canonicalize().ok() == b.canonicalize().ok())
}

/// Refreshes the rail buttons.
fn sync(state: &mut AppState) {
    state.editor_sessions = state
        .editor_slots
        .iter()
        .enumerate()
        .map(|(i, s)| SessionSummary {
            number: i + 1,
            stem: s
                .path
                .file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default(),
            file_name: s
                .path
                .file_name()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default(),
        })
        .collect();
}

/// Builds the editor for `path` and makes it active.
fn activate(state: &mut AppState, index: usize, restore: Option<SessionState>) -> bool {
    let path = state.editor_slots[index].path.clone();
    let f = factory(state);
    if store(state).is_none() {
        return false;
    }
    let Some(st) = state.clip_metadata.as_ref() else {
        return false;
    };
    match Editor::open(&path, st, &state.settings, f) {
        Ok(mut e) => {
            if let Some(r) = restore {
                e.restore(r);
            }
            state.editor = Some(e);
            state.editor_video = None;
            state.editor_filmstrip.clear();
            state.editor_sidebar_scroll = 0.0;
            state.active_session = Some(index);
            state.editor_slots[index].state = None;
            true
        }
        Err(err) => {
            state.dialogs.push_message("Could not open clip", err);
            false
        }
    }
}

/// Releases the active editor: its state goes to its session when `keep`.
fn release_active(state: &mut AppState, keep: bool) {
    let Some(mut e) = state.editor.take() else {
        return;
    };
    let Some(st) = store(state) else {
        return;
    };
    let captured = e.capture(st);
    e.close(st);
    if let Some(i) = state.active_session.take()
        && let Some(slot) = state.editor_slots.get_mut(i)
    {
        slot.state = keep.then_some(captured);
    }
    state.editor_video = None;
    state.editor_filmstrip.clear();
}

/// App exit: the active editor flushes its details, gesture and draft
/// (the reference's `closeEvent`) and releases its player.
pub fn shutdown(state: &mut AppState) {
    release_active(state, false);
}

/// Opens `path` (library double-click, `OpenClip`).
pub fn open(state: &mut AppState, path: &Path) -> bool {
    if !path.is_file() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        state.dialogs.push_message(
            "CLIP NOT FOUND",
            format!("{name} was moved or deleted outside the app."),
        );
        state.scan_clips_requested = true;
        return false;
    }
    if !state.flag("clip_editor_enabled", true) {
        tracing::info!("clip editor disabled; the basic viewer is not ported");
        return false;
    }
    if let Some(i) = state
        .editor_slots
        .iter()
        .position(|s| same_path(&s.path, path))
    {
        return restore(state, i + 1);
    }
    // The reference closes the active viewer before opening another.
    if let Some(i) = state.active_session {
        release_active(state, false);
        state.editor_slots.remove(i);
    }
    if state.editor_slots.len() >= MAX_SESSIONS {
        sync(state);
        state.dialogs.push_message(
            "Three editors are open",
            "Close a sidebar editor before opening another clip.",
        );
        return false;
    }
    state.editor_slots.push(Session {
        path: path.to_path_buf(),
        state: None,
    });
    let i = state.editor_slots.len() - 1;
    let ok = activate(state, i, None);
    if !ok {
        state.editor_slots.remove(i);
    }
    sync(state);
    ok
}

/// Parks the active editor in the rail.
pub fn park(state: &mut AppState) {
    release_active(state, true);
    sync(state);
}

/// Restores session `number` (1-based).
pub fn restore(state: &mut AppState, number: usize) -> bool {
    let i = number.saturating_sub(1);
    if i >= state.editor_slots.len() {
        return false;
    }
    if state.active_session == Some(i) && state.editor.is_some() {
        return true;
    }
    if !state.editor_slots[i].path.is_file() {
        state
            .dialogs
            .push_message("CLIP NOT FOUND", "The editor clip is no longer available.");
        state.editor_slots.remove(i);
        if let Some(a) = state.active_session
            && a > i
        {
            state.active_session = Some(a - 1);
        }
        sync(state);
        return false;
    }
    release_active(state, true);
    let parked = state.editor_slots[i].state.take();
    let ok = activate(state, i, parked);
    sync(state);
    ok
}

/// Closes session `number` (1-based; rail "Close editor").
pub fn close(state: &mut AppState, number: usize) {
    let i = number.saturating_sub(1);
    if i >= state.editor_slots.len() {
        return;
    }
    if state.active_session == Some(i) {
        release_active(state, false);
    }
    state.editor_slots.remove(i);
    if let Some(a) = state.active_session
        && a > i
    {
        state.active_session = Some(a - 1);
    }
    sync(state);
}

/// Tab+Right / Tab+Left: the adjacent library clip in the same editor.
pub fn navigate(state: &mut AppState, step: i32) {
    let (Some(e), Some(i)) = (state.editor.as_ref(), state.active_session) else {
        return;
    };
    let Some(target) = state.library.adjacent(&e.path, step, &state.readiness) else {
        return;
    };
    let other = state
        .editor_slots
        .iter()
        .position(|s| same_path(&s.path, &target));
    release_active(state, true);
    match other {
        Some(j) => {
            // Another session holds the destination: it takes over and the
            // session we leave is dropped (its draft is already flushed).
            state.editor_slots.remove(i);
            let j = if j > i { j - 1 } else { j };
            let parked = state.editor_slots[j].state.take();
            activate(state, j, parked);
        }
        None => {
            state.editor_slots[i] = Session {
                path: target,
                state: None,
            };
            activate(state, i, None);
        }
    }
    sync(state);
}

/// Shift+Down / Shift+Up over `[Clips, Settings] + sessions`, wrapping.
pub fn sidebar_step(state: &mut AppState, step: i32) {
    let n = 2 + state.editor_slots.len() as i32;
    let current = match (state.editor.is_some(), state.active_session) {
        (true, Some(i)) => 2 + i as i32,
        _ if state.page == Page::Settings => 1,
        _ => 0,
    };
    let next = (current + step).rem_euclid(n);
    match next {
        0 | 1 => {
            park(state);
            state.page = if next == 0 {
                Page::Clips
            } else {
                Page::Settings
            };
        }
        k => {
            restore(state, (k - 1) as usize);
        }
    }
}

fn imported_roots(state: &AppState) -> Vec<PathBuf> {
    state
        .settings
        .get("imported_clip_folders")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// A share of the open clip is running (its window holds the editor).
fn sharing_open_clip(state: &AppState) -> bool {
    state
        .editor
        .as_ref()
        .is_some_and(|e| state.active_shares.iter().any(|p| same_path(p, &e.path)))
}

/// Refuses leaving the editor while one of its shares runs. The
/// reference's guard watched the viewer's own export thread; queued
/// exports keep running while the user browses.
fn refuse_while_sharing(state: &mut AppState, message: &str) -> bool {
    if sharing_open_clip(state) {
        state.dialogs.push_message(EXPORT_BUSY, message);
        return true;
    }
    false
}

/// The Delete button: exports of this clip, ownership, then a
/// confirmation.
fn request_delete(state: &mut AppState) {
    let Some(path) = state.editor.as_ref().map(|e| e.path.clone()) else {
        return;
    };
    if state.export_queue.active_for(&path) || sharing_open_clip(state) {
        state.dialogs.push_message(
            EXPORT_BUSY,
            "Finish or cancel this clip\u{2019}s exports before deleting the source.",
        );
        return;
    }
    let owned = state.library.clips_root.clone();
    if classify_media_path(&path, &owned, &imported_roots(state)) != Ownership::FthrOwned {
        state.dialogs.push_message(
            "Linked Original Protected",
            "This clip is linked from another folder. FTHR will not delete the original with the generic Delete action.",
        );
        return;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    state.dialogs.push_modal(DialogKind::Question {
        title: "Delete Clip".into(),
        message: format!("Delete {name}?\nThis cannot be undone."),
        tag: format!("{DELETE_TAG}{}", path.display()),
    });
}

fn delete(state: &mut AppState, path: &Path) {
    let Some(i) = state.active_session.filter(|_| {
        state
            .editor
            .as_ref()
            .is_some_and(|e| same_path(&e.path, path))
    }) else {
        return;
    };
    // Release the player before the file goes.
    release_active(state, false);
    state.editor_slots.remove(i);
    let owned = state.library.clips_root.clone();
    let imports = imported_roots(state);
    match fthrust_core::library_ownership::delete_owned_media(path, &owned, &imports) {
        Ok(()) => {
            let _ = std::fs::remove_file(fthrust_core::audio_manifest::manifest_path_for(path));
            if let Some(st) = store(state)
                && let Err(e) = st.clear_editor_draft(path)
            {
                tracing::warn!(error = %e, "editor draft not cleared");
            }
            state.library.remove(path);
            state.thumb_textures.remove(path);
            state.thumb_meta.remove(path);
            state.library_dirty = true;
        }
        Err(e) => state.dialogs.push_message("Delete Failed", e.to_string()),
    }
    sync(state);
}

/// Runs every frame: library open requests, rail clicks, editor requests
/// and this module's dialog answers.
pub fn service(state: &mut AppState) {
    if let Some(p) = state.open_clip_requested.take() {
        open(state, &p);
    }
    if let Some(n) = state.editor_requested.take() {
        restore(state, n);
    }
    if let Some(n) = state.editor_close_requested.take() {
        close(state, n);
    }
    let requests = state
        .editor
        .as_mut()
        .map(|e| std::mem::take(&mut e.requests))
        .unwrap_or_default();
    let mut keep = Vec::new();
    for r in requests {
        match r {
            EditorRequest::Park => {
                if !refuse_while_sharing(
                    state,
                    "Finish or cancel the export before minimizing this editor.",
                ) {
                    park(state);
                }
            }
            EditorRequest::Close => {
                if let Some(i) = state.active_session {
                    close(state, i + 1);
                }
            }
            EditorRequest::Navigate(step) => {
                if !refuse_while_sharing(
                    state,
                    "Finish or cancel the export before switching clips.",
                ) {
                    navigate(state, step);
                }
            }
            EditorRequest::Export => super::export_ui::export_now(state),
            EditorRequest::Share => super::share::open_picker(state),
            EditorRequest::DeleteClip => request_delete(state),
            other => keep.push(other),
        }
    }
    if let Some(e) = state.editor.as_mut() {
        e.requests.extend(keep);
    }
    let results = std::mem::take(&mut state.dialogs.results);
    let mut rest = Vec::new();
    for r in results {
        match &r {
            DialogResult::Answer { tag, yes } if tag.starts_with(DELETE_TAG) => {
                if *yes {
                    delete(state, &PathBuf::from(&tag[DELETE_TAG.len()..]));
                }
            }
            _ => rest.push(r),
        }
    }
    state.dialogs.results = rest;
}
