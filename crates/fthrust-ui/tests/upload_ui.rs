//! Upload actions on cards, the card menu and the editor
//! (notes-uploader-reference 6.3).

mod common;

use std::path::{Path, PathBuf};

use fthrust_core::upload::history::{Entry, History};
use fthrust_ui::library::card::{CardData, MenuItem, menu_items, share_action};
use fthrust_ui::library::page;
use fthrust_ui::state::{AppState, DialogKind, DialogResult};
use serde_json::json;

fn data() -> CardData {
    CardData {
        path: PathBuf::from("/clips/Desktop/a.mp4"),
        title: "a".into(),
        metadata: String::new(),
        duration_s: Some(1.0),
        is_video: true,
        imported: false,
        uploaded: false,
        upload_link: None,
        upload_enabled: true,
        uploading: false,
        ready: true,
        feedback: None,
        thumbnail: None,
    }
}

#[test]
fn card_states() {
    let a = share_action(&data()).unwrap();
    assert_eq!(
        (a.label, a.tooltip, a.enabled, a.link),
        ("Upload clip", "Upload clip", true, false)
    );

    let mut d = data();
    d.upload_enabled = false;
    let a = share_action(&d).unwrap();
    assert!(!a.enabled);
    assert_eq!(
        a.tooltip,
        "Enable the uploader in Settings to upload this clip"
    );

    let mut d = data();
    d.uploading = true;
    assert!(!share_action(&d).unwrap().enabled);

    let mut d = data();
    d.upload_link = Some("https://files.catbox.moe/x.mp4".into());
    d.upload_enabled = false;
    let a = share_action(&d).unwrap();
    assert_eq!(
        (a.label, a.tooltip, a.enabled, a.link),
        ("Copy link", "Copy link", true, true)
    );

    d.upload_link = Some("sftp://h:22/up/x.mp4".into());
    let a = share_action(&d).unwrap();
    assert_eq!(a.label, "Copy SFTP location");
    assert_eq!(
        a.tooltip,
        "Copy SFTP location (requires an SFTP client and login)"
    );

    // Screenshots without a link have no share button; not ready disables it.
    let mut d = data();
    d.is_video = false;
    assert!(share_action(&d).is_none());
    let mut d = data();
    d.ready = false;
    assert!(!share_action(&d).unwrap().enabled);
}

#[test]
fn an_uploaded_clip_without_a_link_keeps_its_upload_button_and_says_what_a_press_does() {
    let mut d = data();
    d.uploaded = true;
    let a = share_action(&d).unwrap();
    assert!(
        a.enabled,
        "the press has to reach the 'already uploaded' notice"
    );
    assert!(!a.link);
    assert_eq!(
        a.tooltip,
        "Uploaded, but no link came back. Press for the option to upload again."
    );
    assert!(
        menu_items(&d).contains(&MenuItem::Upload),
        "the menu keeps its Upload entry"
    );
    // The other states of the button are unchanged.
    d.uploading = true;
    assert!(!share_action(&d).unwrap().enabled, "busy uploading");
    d.uploading = false;
    d.upload_enabled = false;
    assert_eq!(
        share_action(&d).unwrap().tooltip,
        "Enable the uploader in Settings to upload this clip"
    );
    d.upload_link = Some("https://files.catbox.moe/x.mp4".into());
    let a = share_action(&d).unwrap();
    assert_eq!((a.label, a.enabled, a.link), ("Copy link", true, true));
}

#[test]
fn card_context_menu() {
    let labels =
        |d: &CardData| -> Vec<&'static str> { menu_items(d).iter().map(|i| i.label()).collect() };
    assert_eq!(
        labels(&data()),
        vec![
            "Open in editor",
            "Rename",
            "Show in file manager",
            "Copy path",
            "Upload",
            "Delete clip"
        ]
    );
    let mut d = data();
    d.upload_link = Some("https://x/y".into());
    assert_eq!(
        labels(&d),
        vec![
            "Open in editor",
            "Rename",
            "Show in file manager",
            "Copy path",
            "Open uploaded link",
            "Delete clip"
        ]
    );
    d.upload_link = Some("sftp://h/x/y".into());
    assert!(labels(&d).contains(&"Open SFTP location"));
    let mut d = data();
    d.upload_enabled = false;
    assert!(!labels(&d).contains(&"Upload"));
    let mut d = data();
    d.uploading = true;
    assert!(!labels(&d).contains(&"Upload"));
    assert!(menu_items(&data()).contains(&MenuItem::Upload));
}

struct Rig {
    _dir: tempfile::TempDir,
    state: AppState,
    clip: PathBuf,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let clips = dir.path().join("clips");
    std::fs::create_dir_all(clips.join("Desktop")).unwrap();
    let clip = clips.join("Desktop").join("a.mp4");
    std::fs::write(&clip, b"x").unwrap();
    let mut state = AppState::for_tests(
        dir.path(),
        json!({"clips_directory": clips.to_string_lossy()}),
    );
    state.library.clips_root = clips.clone();
    state.library.upsert(&clip, false);
    Rig {
        _dir: dir,
        state,
        clip,
    }
}

fn answer(state: &mut AppState, tag: String) {
    state
        .dialogs
        .results
        .push(DialogResult::Answer { tag, yes: true });
    page::consume_results(state);
}

fn last_message(state: &AppState) -> Option<(String, String)> {
    state.dialogs.messages.last().map(|m| match m {
        DialogKind::Message { title, message } => (title.clone(), message.clone()),
        other => panic!("{other:?}"),
    })
}

#[test]
fn rename_and_delete_blocked_while_uploading() {
    let mut r = rig();
    r.state
        .upload_status
        .insert(r.clip.clone(), "Uploading…".into());
    answer(&mut r.state, format!("delete-clip:{}", r.clip.display()));
    assert!(r.clip.exists());
    assert_eq!(
        last_message(&r.state),
        Some((
            "Clip is busy".into(),
            "This clip is uploading. Try again when the upload finishes.".into()
        ))
    );
    answer(
        &mut r.state,
        format!("rename-clip:{}\u{1f}b", r.clip.display()),
    );
    assert!(r.clip.exists());
    assert_eq!(r.state.dialogs.messages.len(), 2);
}

#[test]
fn rename_moves_history() {
    let mut r = rig();
    let real = r.clip.canonicalize().unwrap();
    let mut h = History::default();
    h.record(
        &real,
        Entry(
            json!({"status": "ok", "url": "https://x/a"})
                .as_object()
                .unwrap()
                .clone(),
        ),
    );
    h.save(&r.state.paths).unwrap();
    r.state.upload_history = History::load(&r.state.paths);
    answer(
        &mut r.state,
        format!("rename-clip:{}\u{1f}b", r.clip.display()),
    );
    let renamed = r.clip.with_file_name("b.mp4");
    assert!(renamed.exists());
    let h = History::load(&r.state.paths);
    assert!(h.is_uploaded(&renamed.canonicalize().unwrap()));
    assert!(!h.is_uploaded(&real));
    assert!(
        r.state
            .upload_history
            .is_uploaded(&renamed.canonicalize().unwrap())
    );
}

#[test]
fn share_click_copies_or_requests() {
    let mut r = rig();
    let ctx = egui::Context::default();
    page::on_share(&mut r.state, &ctx, &r.clip.clone(), 10.0, None);
    assert_eq!(r.state.upload_requested.as_deref(), Some(r.clip.as_path()));
    assert_eq!(r.state.card_feedback[&r.clip].0, "Upload requested");
    page::on_share(
        &mut r.state,
        &ctx,
        &r.clip.clone(),
        10.0,
        Some("https://x/a"),
    );
    assert_eq!(r.state.card_feedback[&r.clip].0, "Link copied");
    page::on_share(
        &mut r.state,
        &ctx,
        &r.clip.clone(),
        10.0,
        Some("sftp://h/x/a"),
    );
    assert_eq!(r.state.card_feedback[&r.clip].0, "SFTP location copied");
}

/// The history entry of a server that took the clip and sent no link back.
fn record_without_a_link(state: &mut AppState, clip: &Path) {
    state.upload_history.record(
        &clip.canonicalize().unwrap(),
        Entry(
            json!({"status": "ok", "provider": "custom", "url": ""})
                .as_object()
                .unwrap()
                .clone(),
        ),
    );
}

/// The card's tooltip promises the option to upload again: the press must not
/// claim an upload was requested when the clip is uploaded already.
#[test]
fn pressing_upload_on_a_clip_uploaded_without_a_link_says_already_uploaded() {
    let mut r = rig();
    let ctx = egui::Context::default();
    record_without_a_link(&mut r.state, &r.clip.clone());
    page::on_share(&mut r.state, &ctx, &r.clip.clone(), 10.0, None);
    assert_eq!(r.state.card_feedback[&r.clip].0, "Already uploaded");
    // The request still goes out: the upload service answers it with the
    // notice that offers UPLOAD AGAIN.
    assert_eq!(r.state.upload_requested.as_deref(), Some(r.clip.as_path()));
}

#[test]
fn editor_upload_button() {
    use fthrust_ui::editor::sidebar::upload_button;
    assert_eq!(
        upload_button(false, None, 0.0, false),
        None,
        "hidden while uploads are off"
    );
    assert_eq!(
        upload_button(true, None, 0.0, false),
        Some(("Upload", true))
    );
    assert_eq!(
        upload_button(true, Some(12.5), 10.0, false),
        Some(("Queued", false))
    );
    assert_eq!(
        upload_button(true, Some(12.5), 12.6, false),
        Some(("Upload", true))
    );
    // A clip that was uploaded without a link says so on the button: the
    // editor draws no error bar to explain it or to offer the way out.
    assert_eq!(
        upload_button(true, None, 0.0, true),
        Some(("Upload again", true))
    );
    assert_eq!(
        upload_button(true, Some(12.5), 10.0, true),
        Some(("Queued", false))
    );
    assert_eq!(upload_button(false, None, 0.0, true), None);
}

/// Pressing the editor's button uploads a clip again when it was uploaded
/// without a link (the same request as the bar's UPLOAD AGAIN), and asks for a
/// plain upload otherwise.
#[test]
fn the_editors_upload_button_uploads_again_a_clip_that_came_back_without_a_link() {
    use fthrust_ui::editor::sidebar::press_upload;
    use fthrust_ui::services::upload::uploaded_without_link;
    let mut r = rig();
    let clip = r.clip.clone();
    assert!(!uploaded_without_link(&r.state, &clip));
    press_upload(&mut r.state, &clip, false, 5.0);
    assert_eq!(r.state.upload_requested.as_deref(), Some(clip.as_path()));
    assert_eq!(r.state.error_action_requested, None);
    assert_eq!(r.state.editor_upload_queued_until, Some(7.5));

    r.state.upload_requested = None;
    record_without_a_link(&mut r.state, &clip);
    assert!(uploaded_without_link(&r.state, &clip));
    press_upload(&mut r.state, &clip, true, 5.0);
    assert_eq!(r.state.upload_requested, None);
    assert_eq!(
        r.state.error_action_requested,
        Some(format!("upload-again:{}", clip.display()))
    );
    // With a link there is nothing to upload again.
    r.state.upload_history.record(
        &clip.canonicalize().unwrap(),
        Entry(
            json!({"status": "ok", "url": "https://x/a"})
                .as_object()
                .unwrap()
                .clone(),
        ),
    );
    assert!(!uploaded_without_link(&r.state, &clip));
}

/// What the open editor shows on its Upload button.
#[test]
fn the_open_editor_labels_its_upload_button_after_what_became_of_the_clip() {
    use fthrust_ui::editor::{Editor, Lifecycle};
    let mut r = rig();
    r.state.tokens.reduce_motion = true;
    r.state.uploader_settings.set("upload_enabled", json!(true));
    r.state.editor = Some(Editor::for_tests(&r.clip, 10_000, Lifecycle::Paused));
    let label = |state: &mut AppState| {
        let tokens = state.tokens.clone();
        let rendered = common::render(egui::Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
            fthrust_ui::editor::layer::ui(ui, state);
        });
        rendered
            .marker("uploadBtn")
            .and_then(|m| m.text.as_ref().map(|t| t.content.clone()))
    };
    assert_eq!(label(&mut r.state).as_deref(), Some("Upload"));
    let clip = r.clip.clone();
    record_without_a_link(&mut r.state, &clip);
    assert_eq!(label(&mut r.state).as_deref(), Some("Upload again"));
}

#[allow(dead_code)]
fn unused(_: &Path) {}
