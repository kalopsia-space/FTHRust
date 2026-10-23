//! Upload actions on cards, the card menu and the editor
//! (notes-uploader-reference 6.3).

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

#[test]
fn editor_upload_button() {
    use fthrust_ui::editor::sidebar::upload_button;
    assert_eq!(
        upload_button(false, None, 0.0),
        None,
        "hidden while uploads are off"
    );
    assert_eq!(upload_button(true, None, 0.0), Some(("Upload", true)));
    assert_eq!(
        upload_button(true, Some(12.5), 10.0),
        Some(("Queued", false))
    );
    assert_eq!(
        upload_button(true, Some(12.5), 12.6),
        Some(("Upload", true))
    );
}

#[allow(dead_code)]
fn unused(_: &Path) {}
