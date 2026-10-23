//! Plan 5 Task 6: the clip card (`ClipThumbnail`).

mod common;

use std::path::PathBuf;

use egui::{Pos2, Rect, Vec2};
use fthrust_ui::fonts::Weight;
use fthrust_ui::library::card::{CardData, CardResponse, card};
use fthrust_ui::library::thumbs::{MAX_LIVE_TEXTURES, ThumbTextures};
use fthrust_ui::tokens::{Color, Tokens};

fn tokens() -> Tokens {
    let mut t = Tokens::default_palette();
    t.reduce_motion = true;
    t
}

fn ctx_for(tokens: &Tokens) -> egui::Context {
    let ctx = egui::Context::default();
    fthrust_ui::fonts::install(&ctx, tokens);
    let mut out = ctx.run_pass(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    ctx
}

fn data() -> CardData {
    CardData {
        path: PathBuf::from("/clips/Overwatch/Overwatch_clip_from_04Oct2025_12-00-00.mp4"),
        title: "Overwatch_clip_from_04Oct2025_12-00-00".into(),
        metadata: "Screenshot".into(),
        duration_s: Some(0.0),
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

fn render_card(t: &Tokens, d: &CardData, hover: Option<Pos2>) -> common::Rendered {
    let mut icons = fthrust_ui::icons::IconCache::default();
    let mut editing: Option<String> = None;
    let mut h = common::harness(Vec2::new(320.0, 268.0), 1.0, t, |ui| {
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 268.0))),
            |ui| {
                card(ui, t, &mut icons, 320.0, d, &mut editing, "card");
            },
        );
    });
    if let Some(p) = hover {
        h.hover_at(p);
    }
    h.run_steps(2);
    common::render_harness(&mut h)
}

#[test]
fn clip_card_matches_qt() {
    let t = tokens();
    let r = render_card(&t, &data(), None);
    assert_eq!(
        r.marker("cardThumb").unwrap().rect,
        Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 180.0))
    );
    assert_eq!(
        r.marker("cardDurationBadge").unwrap().rect,
        Rect::from_min_size(Pos2::new(262.0, 147.0), Vec2::new(48.0, 25.0))
    );
    assert_eq!(
        r.marker("cardBody").unwrap().rect,
        Rect::from_min_size(Pos2::new(0.0, 180.0), Vec2::new(320.0, 88.0))
    );
    assert_eq!(
        r.marker("cardTitle").unwrap().rect,
        Rect::from_min_size(Pos2::new(10.0, 190.0), Vec2::new(188.0, 32.0))
    );
    assert_eq!(
        r.marker("cardMetadata").unwrap().rect,
        Rect::from_min_size(Pos2::new(10.0, 225.0), Vec2::new(188.0, 33.0))
    );
    assert_eq!(
        r.marker("cardShareBtn").unwrap().rect,
        Rect::from_min_size(Pos2::new(206.0, 198.0), Vec2::new(48.0, 52.0))
    );
    assert_eq!(
        r.marker("cardDeleteBtn").unwrap().rect,
        Rect::from_min_size(Pos2::new(262.0, 198.0), Vec2::new(48.0, 52.0))
    );
    assert_eq!(
        r.pixel(Pos2::new(206.0, 224.0)),
        Some(t.color(Color::Accent)),
        "share button edge"
    );
    assert_eq!(
        r.pixel(Pos2::new(262.0, 224.0)),
        Some(t.color(Color::Delete)),
        "delete button edge"
    );
    assert_eq!(
        r.pixel(Pos2::new(160.0, 90.0)),
        Some(t.color(Color::CardBg)),
        "no thumbnail: card background"
    );
    assert_ne!(
        r.pixel(Pos2::new(160.0, 90.0)),
        r.pixel(Pos2::new(160.0, 2.0)).map(|_| t.color(Color::Text)),
        "play icon hidden when not hovered"
    );
    let ctx = ctx_for(&t);
    assert_fidelity!(r, "clip-card", &t, &ctx);
}

#[test]
fn clip_card_hover_shows_play_icon() {
    let t = tokens();
    // The harness paints a pointer marker under the cursor, so hover away
    // from the icon and sample the icon centre.
    let r = render_card(&t, &data(), Some(Pos2::new(120.0, 60.0)));
    let icon_pixels = |r: &common::Rendered| {
        let bg = t.color(Color::CardBg);
        (72..108)
            .flat_map(|y| (142..178).map(move |x| (x, y)))
            .filter(|(x, y)| r.pixel(Pos2::new(*x as f32, *y as f32)) != Some(bg))
            .count()
    };
    assert!(
        icon_pixels(&r) > 100,
        "a 36 px play icon is painted at the thumb centre on hover"
    );
    let ctx = ctx_for(&t);
    assert_fidelity!(r, "clip-card-hover", &t, &ctx);
    // Not ready: no play icon on hover.
    let mut d = data();
    d.ready = false;
    let r = render_card(&t, &d, Some(Pos2::new(120.0, 60.0)));
    assert_eq!(icon_pixels(&r), 0);
    assert!(
        r.marker("cardFinalizingBadge").is_some(),
        "Finalizing badge while not ready"
    );
}

#[test]
fn clip_card_feedback_label() {
    let t = tokens();
    let mut d = data();
    d.feedback = Some("Upload requested".into());
    let r = render_card(&t, &d, None);
    assert_eq!(
        r.marker("cardActionFeedback").unwrap().rect,
        Rect::from_min_size(Pos2::new(8.0, 8.0), Vec2::new(112.0, 31.0))
    );
    let ctx = ctx_for(&t);
    assert_fidelity!(r, "clip-card-feedback", &t, &ctx);
    // Badges stack bottom-left: Imported then Uploaded above it.
    let mut d = data();
    d.imported = true;
    d.uploaded = true;
    let r = render_card(&t, &d, None);
    let imported = r.marker("cardImportedBadge").unwrap().rect;
    let uploaded = r.marker("cardUploadedBadge").unwrap().rect;
    assert_eq!(imported.min.x, 8.0);
    assert_eq!(imported.max.y, 172.0, "8 px above the thumb bottom");
    assert_eq!(uploaded.max.y, imported.min.y - 4.0);
    assert_eq!(
        r.pixel(Pos2::new(imported.min.x + 2.0, imported.center().y)),
        Some(t.color(Color::Accent)),
        "ACCENT badge background"
    );
}

#[test]
fn rename_enter_commits_escape_cancels() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let t = tokens();
    let d = data();
    let mut icons = fthrust_ui::icons::IconCache::default();
    let editing: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    // The response is per frame: keep the last committed name.
    let last: Rc<RefCell<CardResponse>> = Rc::new(RefCell::new(CardResponse::default()));
    let (e2, l2) = (editing.clone(), last.clone());
    let tc = t.clone();
    let mut h = common::harness(Vec2::new(320.0, 268.0), 1.0, &t, move |ui| {
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 268.0))),
            |ui| {
                let mut e = e2.borrow_mut();
                let r = card(ui, &tc, &mut icons, 320.0, &d, &mut e, "card");
                if r.rename_committed.is_some() {
                    *l2.borrow_mut() = r;
                }
            },
        );
    });
    h.run_steps(2);
    h.hover_at(Pos2::new(100.0, 206.0));
    h.step();
    h.key_press(egui::Key::F2);
    h.run_steps(2);
    assert_eq!(
        editing.borrow().as_deref(),
        Some("Overwatch_clip_from_04Oct2025_12-00-00"),
        "F2 starts editing with the stem"
    );
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.step();
    for c in "new name".chars() {
        h.event(egui::Event::Text(c.to_string()));
        h.step();
    }
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(last.borrow().rename_committed.as_deref(), Some("new name"));
    assert!(editing.borrow().is_none());
    // Escape cancels.
    last.borrow_mut().rename_committed = None;
    h.hover_at(Pos2::new(100.0, 206.0));
    h.step();
    h.key_press(egui::Key::F2);
    h.run_steps(2);
    assert!(editing.borrow().is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(editing.borrow().is_none());
    assert!(last.borrow().rename_committed.is_none());
    drop(h);
    let _ = Weight::W400;
}

#[test]
fn thumb_pool_caps_live_textures_at_96() {
    let ctx = egui::Context::default();
    let mut textures = ThumbTextures::default();
    for i in 0..100 {
        let img = egui::ColorImage::filled([2, 2], egui::Color32::RED);
        textures.insert(
            PathBuf::from(format!("/c/{i}.mp4")),
            ctx.load_texture(format!("t{i}"), img, egui::TextureOptions::LINEAR),
            None,
        );
    }
    assert_eq!(textures.len(), MAX_LIVE_TEXTURES);
    assert_eq!(MAX_LIVE_TEXTURES, 96);
    assert!(
        textures.get(&PathBuf::from("/c/0.mp4")).is_none(),
        "the oldest was evicted"
    );
    assert!(textures.get(&PathBuf::from("/c/99.mp4")).is_some());
    // Touching an entry keeps it alive.
    let _ = textures.get(&PathBuf::from("/c/4.mp4"));
    let img = egui::ColorImage::filled([2, 2], egui::Color32::RED);
    textures.insert(
        PathBuf::from("/c/100.mp4"),
        ctx.load_texture("t100", img, egui::TextureOptions::LINEAR),
        None,
    );
    assert!(textures.get(&PathBuf::from("/c/4.mp4")).is_some());
    assert!(textures.get(&PathBuf::from("/c/5.mp4")).is_none());
}

#[test]
fn rename_commits_on_focus_loss() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let t = tokens();
    let d = data();
    let mut icons = fthrust_ui::icons::IconCache::default();
    let editing: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let last: Rc<RefCell<CardResponse>> = Rc::new(RefCell::new(CardResponse::default()));
    let (e2, l2) = (editing.clone(), last.clone());
    let tc = t.clone();
    let mut h = common::harness(Vec2::new(320.0, 268.0), 1.0, &t, move |ui| {
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 268.0))),
            |ui| {
                let mut e = e2.borrow_mut();
                let r = card(ui, &tc, &mut icons, 320.0, &d, &mut e, "card");
                if r.rename_committed.is_some() {
                    *l2.borrow_mut() = r;
                }
            },
        );
    });
    h.run_steps(2);
    h.hover_at(Pos2::new(100.0, 206.0));
    h.step();
    h.key_press(egui::Key::F2);
    h.run_steps(2);
    assert!(editing.borrow().is_some());
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.step();
    for c in "blurred".chars() {
        h.event(egui::Event::Text(c.to_string()));
        h.step();
    }
    // Clicking elsewhere ends the edit like Qt's editingFinished.
    h.hover_at(Pos2::new(160.0, 60.0));
    h.step();
    h.drag_at(Pos2::new(160.0, 60.0));
    h.step();
    h.drop_at(Pos2::new(160.0, 60.0));
    h.run_steps(3);
    assert_eq!(last.borrow().rename_committed.as_deref(), Some("blurred"));
    assert!(editing.borrow().is_none());
    drop(h);
}
