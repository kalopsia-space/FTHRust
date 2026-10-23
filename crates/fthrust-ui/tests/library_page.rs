//! Plan 5 Task 7: the Clips page (`ClipGrid`).

mod common;

use std::path::PathBuf;
use std::time::Instant;

use egui::{Pos2, Rect, Vec2};
use fthrust_core::library_index::{Filter, LibraryRecord, MediaKind, Sort};
use fthrust_ui::shell;
use fthrust_ui::state::{AppState, DialogKind, DialogResult, Page, StatusStyle, StatusText};
use fthrust_ui::tokens::Color;

fn state(settings_json: serde_json::Value) -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), settings_json);
    s.tokens.reduce_motion = true;
    s.page = Page::Clips;
    s.status = StatusText {
        text: "CONNECTING".into(),
        style: StatusStyle::Idle,
    };
    (tmp, s)
}

fn ctx_for(tokens: &fthrust_ui::tokens::Tokens) -> egui::Context {
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, tokens);
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    ctx
}

fn markers_after(h: &mut egui_kittest::Harness<'_>) -> Vec<fthrust_ui::fidelity::Marker> {
    h.run_steps(2);
    let m = fthrust_ui::fidelity::take(&h.ctx);
    if m.is_empty() {
        h.step();
        fthrust_ui::fidelity::take(&h.ctx)
    } else {
        m
    }
}

fn click(h: &mut egui_kittest::Harness<'_>, at: Pos2) {
    h.hover_at(at);
    h.step();
    h.drag_at(at);
    h.step();
    h.drop_at(at);
    h.run_steps(2);
}

#[test]
fn library_page_matches_qt() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let tokens = s.tokens.clone();
    let ctx = ctx_for(&tokens);
    let r = common::render(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    assert_eq!(
        r.marker("filterBar").unwrap().rect,
        Rect::from_min_size(Pos2::new(118.0, 119.0), Vec2::new(1298.0, 60.0))
    );
    assert_eq!(
        r.marker("allCountLabel").unwrap().rect.min,
        Pos2::new(130.0, 119.0)
    );
    assert_eq!(
        r.marker("_DropdownCombo[All clips]").unwrap().rect,
        Rect::from_min_size(Pos2::new(1112.0, 129.0), Vec2::new(140.0, 38.0))
    );
    assert_eq!(
        r.marker("_DropdownCombo[Newest]").unwrap().rect.min.x,
        1264.0
    );
    assert_eq!(
        r.marker("QLabel[No clips yet]").unwrap().rect,
        Rect::from_min_size(Pos2::new(118.0, 283.0), Vec2::new(1298.0, 39.0))
    );
    assert_eq!(
        r.pixel(Pos2::new(767.0, 149.0)),
        Some(tokens.color(Color::ShellBg)),
        "filter bar fill"
    );
    assert_fidelity!(r, "library", &tokens, &ctx);
    let r = common::render(Vec2::new(1100.0, 760.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    assert_eq!(r.marker("filterBar").unwrap().rect.width(), 958.0);
    assert_fidelity!(r, "library-small", &tokens, &ctx);
    // Record hovered.
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    h.hover_at(Pos2::new(84.0, 68.0));
    h.run_steps(2);
    let r = common::render_harness(&mut h);
    assert_fidelity!(r, "record-hover", &tokens, &ctx);
}

fn synthetic_records(root: &std::path::Path, n: usize) -> Vec<LibraryRecord> {
    let base: i128 = 1_800_000_000 * 1_000_000_000;
    (0..n)
        .map(|i| {
            let path = root.join(format!("Desktop/desktop_clip_from_{i:05}.mp4"));
            LibraryRecord {
                path: path.clone(),
                identity: path,
                root_ids: vec![0],
                imported: false,
                kind: MediaKind::Video,
                size: 1,
                mtime_ns: base - (i as i128) * 3_600_000_000_000,
            }
        })
        .collect()
}

#[test]
fn only_visible_rows_request_thumbnails() {
    let (tmp, mut s) = state(serde_json::json!({}));
    let root = tmp.path().join("clips");
    s.library.records = synthetic_records(&root, 5000);
    s.library_dirty = true;
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    h.run_steps(2);
    let started = Instant::now();
    h.step();
    let frame = started.elapsed();
    drop(h);
    eprintln!("frame with 5000 records: {frame:?}");
    // 16 ms in release terms; tests run unoptimised, so allow 50 ms.
    assert!(
        frame.as_millis() < 50,
        "a frame with 5000 records took {frame:?}"
    );
    assert_eq!(s.library.count, 5000);
    assert!(!s.visible_cards.is_empty());
    assert!(
        s.visible_cards.len() <= 3 * 5,
        "only the visible rows (plus 2 overscan) are built: {}",
        s.visible_cards.len()
    );
    assert!(
        s.thumb_requests_total <= 3 * 5,
        "thumbnails are requested only for visible cards: {}",
        s.thumb_requests_total
    );
}

#[test]
fn filter_and_sort_rebuild_sections() {
    let (tmp, mut s) = state(serde_json::json!({}));
    let root = tmp.path().join("clips");
    let mut records = synthetic_records(&root, 3);
    records.push(LibraryRecord {
        path: root.join("Screenshots/shot.png"),
        identity: root.join("Screenshots/shot.png"),
        root_ids: vec![0],
        imported: false,
        kind: MediaKind::Image,
        size: 1,
        mtime_ns: 1_700_000_000 * 1_000_000_000,
    });
    s.library.records = records;
    s.library_dirty = true;
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    let m = markers_after(&mut h);
    assert!(
        m.iter().any(|m| m.name == "clipCard[0]"),
        "cards are built: {:?}",
        m.iter()
            .map(|m| m.name.clone())
            .filter(|n| n.starts_with("clip"))
            .collect::<Vec<_>>()
    );
    let combo = m
        .iter()
        .find(|m| m.name == "_DropdownCombo[All clips]")
        .unwrap()
        .rect;
    click(&mut h, combo.center());
    let m = markers_after(&mut h);
    let item = m
        .iter()
        .find(|m| m.name == "_DropdownCombo[All clips]/item[Screenshots]")
        .unwrap()
        .rect;
    click(&mut h, item.center());
    let m = markers_after(&mut h);
    assert!(m.iter().any(|m| m.name == "_DropdownCombo[Screenshots]"));
    let sort = m
        .iter()
        .find(|m| m.name == "_DropdownCombo[Newest]")
        .unwrap()
        .rect;
    click(&mut h, sort.center());
    let m = markers_after(&mut h);
    let item = m
        .iter()
        .find(|m| m.name == "_DropdownCombo[Newest]/item[Oldest]")
        .unwrap()
        .rect;
    click(&mut h, item.center());
    h.run_steps(2);
    drop(h);
    assert_eq!(s.library.filter, Filter::Screenshots);
    assert_eq!(s.library.sort, Sort::Oldest);
    assert_eq!(s.library.count, 1);
    assert_eq!(s.library.heading_text(), "SCREENSHOTS  (1)");
}

#[test]
fn delete_question_then_file_removed() {
    let (tmp, mut s) = state(serde_json::json!({}));
    let root = s.library.clips_root.clone();
    std::fs::create_dir_all(root.join("Desktop")).unwrap();
    let clip = root.join("Desktop/desktop_clip_from_01Jan2027_10-00-00.mp4");
    std::fs::write(&clip, b"video").unwrap();
    assert!(s.library.upsert(&clip, false));
    s.library_dirty = true;
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    let m = markers_after(&mut h);
    let del = m.iter().find(|m| m.name == "cardDeleteBtn").unwrap().rect;
    click(&mut h, del.center());
    h.run_steps(2);
    drop(h);
    let tag = format!("delete-clip:{}", clip.display());
    assert!(
        matches!(s.dialogs.modal.front(), Some(DialogKind::Question { tag: t, message, .. }) if *t == tag && message.starts_with("Delete desktop_clip_from_01Jan2027_10-00-00.mp4?\nThis cannot be undone.")),
        "{:?}",
        s.dialogs.modal
    );
    s.dialogs.modal.clear();
    s.dialogs
        .results
        .push(DialogResult::Answer { tag, yes: true });
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    h.run_steps(3);
    drop(h);
    assert!(!clip.exists(), "the file is deleted");
    assert!(s.library.records.is_empty());
    let _ = tmp;
}

#[test]
fn rename_updates_card_and_manifest() {
    let (tmp, mut s) = state(serde_json::json!({}));
    let root = s.library.clips_root.clone();
    std::fs::create_dir_all(root.join("Desktop")).unwrap();
    let clip = root.join("Desktop/old_name.mp4");
    std::fs::write(&clip, b"video").unwrap();
    s.library.upsert(&clip, false);
    s.library_dirty = true;
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    let m = markers_after(&mut h);
    let title = m.iter().find(|m| m.name == "cardTitle").unwrap().rect;
    h.hover_at(title.center());
    h.step();
    h.key_press(egui::Key::F2);
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.step();
    for c in "renamed".chars() {
        h.event(egui::Event::Text(c.to_string()));
        h.step();
    }
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    drop(h);
    let renamed = root.join("Desktop/renamed.mp4");
    assert!(renamed.exists() && !clip.exists(), "renamed on disk");
    assert_eq!(s.library.records[0].path, renamed);
    assert!(s.rename_draft.is_none());
    let _ = tmp;
    let _: PathBuf = renamed;
}

#[test]
fn up_and_down_scroll_the_grid() {
    let (_tmp, mut s) = state(serde_json::json!({}));
    let root = s.library.clips_root.clone();
    std::fs::create_dir_all(root.join("Desktop")).unwrap();
    for i in 0..18 {
        let clip = root.join(format!("Desktop/clip_{i:02}.mp4"));
        std::fs::write(&clip, b"video").unwrap();
        s.library.upsert(&clip, false);
    }
    s.library_dirty = true;
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    h.run_steps(3);
    h.hover_at(Pos2::new(700.0, 500.0));
    h.step();
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(4);
    drop(h);
    assert!(
        s.library_scroll > 0.0,
        "Down scrolls one row: {}",
        s.library_scroll
    );
    let after_down = s.library_scroll;
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    h.run_steps(3);
    h.hover_at(Pos2::new(700.0, 500.0));
    h.step();
    h.key_press(egui::Key::ArrowUp);
    h.run_steps(4);
    drop(h);
    assert!(
        s.library_scroll < after_down,
        "Up scrolls back: {}",
        s.library_scroll
    );
}

#[test]
fn renaming_a_linked_import_asks_first() {
    let tmp = tempfile::tempdir().unwrap();
    let import_dir = tmp.path().join("imports");
    std::fs::create_dir_all(&import_dir).unwrap();
    let (_tmp2, mut s) =
        state(serde_json::json!({"imported_clip_folders": [import_dir.to_string_lossy()]}));
    let clip = import_dir.join("original.mp4");
    std::fs::write(&clip, b"video").unwrap();
    s.library.upsert(&clip, true);
    s.library_dirty = true;
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    let m = markers_after(&mut h);
    let title = m.iter().find(|m| m.name == "cardTitle").unwrap().rect;
    h.hover_at(title.center());
    h.step();
    h.key_press(egui::Key::F2);
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.step();
    for c in "moved".chars() {
        h.event(egui::Event::Text(c.to_string()));
        h.step();
    }
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    drop(h);
    assert!(clip.exists(), "the original is untouched until confirmed");
    let tag = match s.dialogs.modal.front() {
        Some(DialogKind::Question { tag, title, .. }) if tag.starts_with("rename-clip:") => {
            assert!(title.to_lowercase().contains("rename"), "{title}");
            tag.clone()
        }
        other => panic!("expected a rename question, got {other:?}"),
    };
    s.dialogs.modal.pop_front();
    s.dialogs
        .results
        .push(DialogResult::Answer { tag, yes: true });
    fthrust_ui::library::page::consume_results(&mut s);
    assert!(
        import_dir.join("moved.mp4").exists() && !clip.exists(),
        "renamed after yes"
    );
}
