//! Plan 5 Task 7: the Clips page (`ClipGrid`).

mod common;

use std::path::PathBuf;
use std::time::Instant;

use egui::{Pos2, Rect, Vec2};
use fthrust_core::library_index::{Filter, LibraryRecord, MediaKind, Sort};
use fthrust_ui::fidelity::Marker;
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

/// The fastest of ten settled frames with `n` records.
fn settled_frame_time(n: usize) -> std::time::Duration {
    let (tmp, mut s) = state(serde_json::json!({}));
    s.library.records = synthetic_records(&tmp.path().join("clips"), n);
    s.library_dirty = true;
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    h.run_steps(4);
    (0..10)
        .map(|_| {
            let started = Instant::now();
            h.step();
            started.elapsed()
        })
        .min()
        .unwrap()
}

#[test]
fn a_frame_costs_about_the_same_in_a_big_library_as_in_a_small_one() {
    let small = settled_frame_time(300);
    let large = settled_frame_time(12_000);
    eprintln!("frame with 300 records: {small:?}, with 12000: {large:?}");
    // The same 15 or so cards are built either way; only work proportional to
    // the library (cloning every section, scanning every record for each
    // card, laying out every day's header) tells the two apart.
    assert!(
        large < small * 3 + std::time::Duration::from_millis(4),
        "a frame took {large:?} with 12000 records and {small:?} with 300"
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

/// Answers the rename question of `path` with `stem`, as the dialog does.
fn rename_through_the_dialog(s: &mut AppState, path: &std::path::Path, stem: &str) {
    s.dialogs.results.push(DialogResult::Answer {
        tag: format!("rename-clip:{}\u{1f}{stem}", path.display()),
        yes: true,
    });
    fthrust_ui::library::page::consume_results(s);
}

#[test]
fn renaming_a_clip_keeps_its_tag_description_and_editor_draft() {
    use fthrust_core::clip_metadata::{ClipMetadataStore, DraftFile, EditorDraft};
    let (_tmp, mut s) = state(serde_json::json!({}));
    let root = s.library.clips_root.clone();
    std::fs::create_dir_all(root.join("Desktop")).unwrap();
    let clip = root.join("Desktop/old_name.mp4");
    std::fs::write(&clip, b"video").unwrap();
    s.library.upsert(&clip, false);
    // What the editor saved for the clip.
    let identity = fthrust_core::media_identity::file_identity(&clip).unwrap();
    {
        let mut store = ClipMetadataStore::open(&s.paths).unwrap();
        store
            .update(&clip, Some("clutch"), Some("1v4 on B site"))
            .unwrap();
        let draft = EditorDraft {
            version: 1,
            file: DraftFile {
                mtime_ns: identity.mtime_ns,
                size: identity.size,
            },
            updated_at: 1.0,
            state: serde_json::json!({"trim_start": 0.25}),
        };
        store.set_editor_draft(&clip, draft).unwrap();
    }
    rename_through_the_dialog(&mut s, &clip, "renamed");
    let renamed = root.join("Desktop/renamed.mp4");
    assert!(renamed.exists() && !clip.exists(), "renamed on disk");
    let store = ClipMetadataStore::open(&s.paths).unwrap();
    let entry = store.get(&renamed).unwrap();
    assert_eq!(entry.tag, "clutch");
    assert_eq!(entry.description, "1v4 on B site");
    let renamed_identity = fthrust_core::media_identity::file_identity(&renamed).unwrap();
    assert!(
        store
            .editor_draft_if_current(&renamed, &renamed_identity)
            .unwrap()
            .is_some(),
        "the draft followed the file"
    );
    let old = store.get(&clip).unwrap();
    assert!(
        old.tag.is_empty() && old.description.is_empty() && old.editor_draft.is_none(),
        "nothing is left under the old name: {old:?}"
    );
}

/// `n_new` clips on one day and `n_old` on the day before, all within an hour
/// of noon UTC so that any local offset keeps the two days apart.
fn two_day_records(root: &std::path::Path, n_new: usize, n_old: usize) -> Vec<LibraryRecord> {
    const MINUTE: i128 = 60 * 1_000_000_000;
    const DAY: i128 = 24 * 60 * MINUTE;
    let noon: i128 = 1_800_014_400 * 1_000_000_000; // 2027-01-15 12:00 UTC
    let make = |i: usize, at: i128, tag: &str| {
        let path = root.join(format!("Desktop/{tag}_clip_{i:03}.mp4"));
        LibraryRecord {
            path: path.clone(),
            identity: path,
            root_ids: vec![0],
            imported: false,
            kind: MediaKind::Video,
            size: 1,
            mtime_ns: at - i as i128 * MINUTE,
        }
    };
    (0..n_new)
        .map(|i| make(i, noon, "new"))
        .chain((0..n_old).map(|i| make(i, noon - DAY, "old")))
        .collect()
}

/// The cards and day headers drawn this frame (screen rects).
fn drawn(markers: &[Marker]) -> Vec<(String, Rect)> {
    markers
        .iter()
        .filter(|m| m.name.starts_with("clipCard[") || m.name.starts_with("sectionHeader["))
        .map(|m| (m.name.clone(), m.rect))
        .collect()
}

/// The stretches of `top..bottom` that nothing in `rects` covers and that are
/// taller than the page's own spacing (20 px between sections).
fn blank_bands(rects: &[(String, Rect)], top: f32, bottom: f32) -> Vec<(f32, f32)> {
    const SPACING: f32 = 24.0;
    let mut spans: Vec<(f32, f32)> = rects
        .iter()
        .map(|(_, r)| (r.min.y.max(top), r.max.y.min(bottom)))
        .filter(|(a, b)| a < b)
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut bands = Vec::new();
    let mut reached = top;
    for (a, b) in spans {
        if a - reached > SPACING {
            bands.push((reached, a));
        }
        reached = reached.max(b);
    }
    if bottom - reached > SPACING {
        bands.push((reached, bottom));
    }
    bands
}

fn wheel_down(h: &mut egui_kittest::Harness<'_>, dy: f32) {
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -dy),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: egui::Modifiers::NONE,
    });
}

#[test]
fn scrolling_away_drops_the_queued_thumbnail_jobs_of_the_old_rows() {
    use fthrust_ui::library::thumbs::ThumbPool;
    let (tmp, mut s) = state(serde_json::json!({}));
    s.library.records = two_day_records(&tmp.path().join("clips"), 40, 20);
    s.library_dirty = true;
    // A pool without workers: whatever the page asks for stays queued.
    s.thumb_pool = Some(ThumbPool::start_with(tmp.path().join("cache"), 0));
    let tokens = s.tokens.clone();
    let queued = std::rc::Rc::new(std::cell::Cell::new((0usize, 0usize)));
    let probe = queued.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
        let jobs = s.thumb_pool.as_ref().map_or(0, |p| p.inflight());
        probe.set((jobs, s.visible_cards.len()));
    });
    h.run_steps(4);
    let (first, built) = queued.get();
    assert!(first > 0, "the first screen asked for thumbnails");
    assert!(first <= built, "{first} jobs for {built} built cards");
    h.hover_at(Pos2::new(700.0, 500.0));
    // Down the page, pausing after each step long enough for the page to ask
    // for what it then shows.
    for _ in 0..8 {
        wheel_down(&mut h, 400.0);
        h.run_steps(40);
    }
    let (jobs, built) = queued.get();
    assert!(
        jobs <= built,
        "{jobs} jobs queued for {built} built cards: the old rows' jobs stayed"
    );
}

#[test]
fn a_failed_thumbnail_is_asked_for_again_when_the_clip_changes() {
    let (tmp, mut s) = state(serde_json::json!({}));
    s.library.records = synthetic_records(&tmp.path().join("clips"), 1);
    s.library_dirty = true;
    let clip = s.library.records[0].path.clone();
    let fp = {
        let r = &s.library.records[0];
        format!("{}|{}", r.size, r.mtime_ns)
    };
    s.thumb_failed.insert(clip.clone(), fp, 0.0);
    let tokens = s.tokens.clone();
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    h.run_steps(5);
    drop(h);
    assert_eq!(s.thumb_requests_total, 0, "a fresh failure is left alone");
    // The clip was rewritten.
    s.library.records[0].size += 1;
    s.library_dirty = true;
    let mut h = common::harness(Vec2::new(1440.0, 900.0), 1.0, &tokens, |ui| {
        shell::frame(ui, &mut s);
    });
    h.run_steps(5);
    drop(h);
    assert_eq!(s.thumb_requests_total, 1, "a changed clip is tried again");
    assert!(!s.thumb_failed.contains(&clip));
}

#[test]
fn a_failed_thumbnail_is_asked_for_again_after_a_while() {
    let (tmp, mut s) = state(serde_json::json!({}));
    s.library.records = synthetic_records(&tmp.path().join("clips"), 1);
    s.library_dirty = true;
    let fp = {
        let r = &s.library.records[0];
        format!("{}|{}", r.size, r.mtime_ns)
    };
    s.thumb_failed
        .insert(s.library.records[0].path.clone(), fp, 0.0);
    let tokens = s.tokens.clone();
    // Each step is four seconds on egui's clock; a harness starts with a few.
    let h = common::harness_with_dt(Vec2::new(1440.0, 900.0), 1.0, &tokens, 4.0, |ui| {
        shell::frame(ui, &mut s);
    });
    drop(h);
    assert_eq!(s.thumb_requests_total, 0, "held for the first seconds");
    let mut h = common::harness_with_dt(Vec2::new(1440.0, 900.0), 1.0, &tokens, 4.0, |ui| {
        shell::frame(ui, &mut s);
    });
    h.run_steps(10);
    drop(h);
    assert_eq!(s.thumb_requests_total, 1, "tried again once it is due");
}

/// Scrolls a 60-clip, two-day Clips page from the top to its end and checks
/// at every step that the cards and day headers cover the visible area.
/// `margin` is a content margin on the scroll area's style: the page lays
/// its rows out below it and must draw the same. With `fine`, once the second
/// day's header has shown up the steps shrink to 16 px, so that the header
/// crosses the top edge of the area in several of them.
fn scroll_the_grid_to_the_end(size: Vec2, ppp: f32, margin: egui::Margin, fine: bool) {
    let (tmp, mut s) = state(serde_json::json!({}));
    // 60 clips over two days: 40 on the newer one.
    s.library.records = two_day_records(&tmp.path().join("clips"), 40, 20);
    s.library_dirty = true;
    let tokens = s.tokens.clone();
    let offset = std::rc::Rc::new(std::cell::Cell::new(0.0f32));
    let seen = offset.clone();
    let mut h = common::harness(size, ppp, &tokens, |ui| {
        ui.style_mut().spacing.scroll.content_margin = margin;
        shell::frame(ui, &mut s);
        seen.set(s.library_scroll);
    });
    h.run_steps(3);
    let area_top = {
        let m = markers_after(&mut h);
        m.iter().find(|m| m.name == "filterBar").unwrap().rect.max.y + 8.0
    };
    let area_bottom = size.y;
    h.hover_at(Pos2::new(size.x / 2.0, (area_top + area_bottom) / 2.0));
    let mut headers_seen = std::collections::BTreeSet::new();
    let mut step_px = 400.0;
    let mut last = -1.0f32;
    let mut reached_end = false;
    for step in 0..600 {
        let m = markers_after(&mut h);
        for m in m.iter().filter(|m| m.name.starts_with("sectionHeader[")) {
            headers_seen.insert(m.name.clone());
        }
        if fine && headers_seen.len() == 2 {
            step_px = 16.0;
        }
        // The margin above the first row is empty by design.
        let content_top = area_top + (f32::from(margin.top) - offset.get()).max(0.0);
        let on_screen = drawn(&m);
        let blank = blank_bands(&on_screen, content_top, area_bottom);
        assert!(
            blank.is_empty(),
            "{size:?} at {ppp}, step {step}, scrolled {:.0} px: nothing is drawn at {blank:?} (drawn: {:?})",
            offset.get(),
            on_screen
                .iter()
                .map(|(n, r)| (n.clone(), r.min.y as i32))
                .collect::<Vec<_>>()
        );
        if step > 0 && (offset.get() - last).abs() < 0.5 {
            reached_end = true;
            break;
        }
        last = offset.get();
        wheel_down(&mut h, step_px);
        h.run_steps(if step_px < 100.0 { 6 } else { 10 });
    }
    assert!(
        reached_end && offset.get() > 4000.0,
        "scrolled to the end of the content: {}",
        offset.get()
    );
    assert_eq!(
        headers_seen.len(),
        2,
        "both day headers were drawn: {headers_seen:?}"
    );
}

#[test]
fn the_grid_draws_its_cards_and_headers_at_every_scroll_offset() {
    scroll_the_grid_to_the_end(Vec2::new(1440.0, 900.0), 1.0, egui::Margin::ZERO, false);
}

#[test]
fn the_grid_draws_at_a_small_window_and_a_scaled_screen_too() {
    // Two columns, a 300 px band under the filter bar, 1.5 pixels per point.
    scroll_the_grid_to_the_end(Vec2::new(800.0, 500.0), 1.5, egui::Margin::ZERO, false);
}

#[test]
fn the_grid_does_not_depend_on_the_scroll_areas_content_margin() {
    // egui hands over the visible part relative to the content before its
    // margin, while the rows are laid out inside it: with a margin the page
    // used to cull headers that were still in view at the top edge.
    let margin = egui::Margin {
        left: 0,
        right: 0,
        top: 120,
        bottom: 0,
    };
    scroll_the_grid_to_the_end(Vec2::new(1440.0, 900.0), 1.0, margin, true);
}
