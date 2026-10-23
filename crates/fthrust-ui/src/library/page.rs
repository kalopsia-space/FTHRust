//! The Clips page (`ClipGrid`): filter bar, day sections of cards in a
//! virtualised grid, the empty state, thumbnails for visible cards only,
//! rename, delete and share actions, and keyboard navigation.

use std::path::{Path, PathBuf};

use egui::{Align, Color32, CornerRadius, Pos2, Rect, Sense, Ui, Vec2};
use fthrust_core::library_index::{Filter, MediaKind, Sort};
use fthrust_core::library_ownership::{Ownership, classify_media_path};

use super::card::{CardData, card};
use super::layout::{GRID_SPACING, H_MARGIN, SECTION_SPACING, card_height, metrics};
use super::thumbs::ThumbImage;
use crate::fidelity;
use crate::fonts::Weight;
use crate::state::{AppState, DialogKind, DialogResult};
use crate::tokens::{Color, geom, size};
use crate::widgets::combo::Combo;
use crate::widgets::{galley, paint_text, scroll};

/// Filter bar height (`Sizes.FILTER_H`).
pub const FILTER_H: f32 = 60.0;
/// Page margins (left, top, right, bottom).
pub const MARGINS: (f32, f32, f32, f32) = (24.0, 18.0, 24.0, 24.0);
/// Rows above and below the viewport that are also built.
pub const OVERSCAN_ROWS: usize = 2;
/// Thumbnail requests pause this long after the last scroll input.
pub const SCROLL_SETTLE_S: f64 = 0.09;

const FILTERS: [(&str, Filter); 4] = [
    ("All clips", Filter::All),
    ("Clips only", Filter::Clips),
    ("Screenshots", Filter::Screenshots),
    ("Imported clips", Filter::Imported),
];
const SORTS: [(&str, Sort); 2] = [("Newest", Sort::Newest), ("Oldest", Sort::Oldest)];

/// The screenshot hotkey shown in the empty state.
fn screenshot_key(state: &AppState) -> String {
    let k = state.hotkeys.get("save_screenshot").unwrap_or("");
    if k.is_empty() {
        "F12".into()
    } else {
        k.to_owned()
    }
}

/// `size|mtime_ns` of a record (invalidates thumbnails on rewrite).
fn fingerprint(state: &AppState, path: &Path) -> String {
    state
        .library
        .records
        .iter()
        .find(|r| r.path == path)
        .map(|r| format!("{}|{}", r.size, r.mtime_ns))
        .unwrap_or_default()
}

fn drain_thumbnails(ui: &Ui, state: &mut AppState) {
    let mut results = Vec::new();
    if let Some(pool) = &state.thumb_pool {
        while let Ok(r) = pool.results().try_recv() {
            results.push(r);
        }
    }
    for r in results {
        state.thumb_requested.remove(&r.path);
        if Some(r.generation) != state.thumb_pool.as_ref().map(|p| p.generation()) {
            continue;
        }
        match r.image {
            Some(ThumbImage::Video(cached)) => {
                let meta = cached.meta;
                let mut details = vec![format!("{}p", meta.height)];
                if meta.fps > 0.0 {
                    details.push(format!("{} FPS", trim_float(meta.fps)));
                }
                if meta.video_bitrate > 0 {
                    details.push(format!(
                        "{} Mbps",
                        trim_float(meta.video_bitrate as f64 / 1_000_000.0)
                    ));
                }
                state
                    .thumb_meta
                    .insert(r.path.clone(), (details.join(" · "), Some(meta.duration)));
                if let Ok(img) = image::open(&cached.jpg) {
                    let rgba = img.to_rgba8();
                    let ci = egui::ColorImage::from_rgba_unmultiplied(
                        [rgba.width() as usize, rgba.height() as usize],
                        rgba.as_raw(),
                    );
                    let tex = ui.ctx().load_texture(
                        format!("thumb:{}", r.path.display()),
                        ci,
                        egui::TextureOptions::LINEAR,
                    );
                    state.thumb_textures.insert(r.path.clone(), tex, Some(meta));
                }
            }
            Some(ThumbImage::Image(rgba)) => {
                state
                    .thumb_meta
                    .insert(r.path.clone(), ("Screenshot".into(), None));
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [rgba.width() as usize, rgba.height() as usize],
                    rgba.as_raw(),
                );
                let tex = ui.ctx().load_texture(
                    format!("thumb:{}", r.path.display()),
                    ci,
                    egui::TextureOptions::LINEAR,
                );
                state.thumb_textures.insert(r.path.clone(), tex, None);
            }
            None => {
                state.thumb_failed.insert(r.path.clone());
                state
                    .thumb_meta
                    .entry(r.path.clone())
                    .or_insert_with(|| ("Video clip".into(), None));
            }
        }
    }
}

fn trim_float(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        "0".into()
    } else {
        s.to_owned()
    }
}

/// Handles the dialog answers this page asked for (`delete-clip:<path>`).
pub fn consume_results(state: &mut AppState) {
    let results = std::mem::take(&mut state.dialogs.results);
    let mut keep = Vec::new();
    for r in results {
        match &r {
            DialogResult::Answer { tag, yes } if tag.starts_with("rename-clip:") => {
                if *yes && let Some((path, stem)) = tag["rename-clip:".len()..].split_once('\u{1f}')
                {
                    rename_clip(state, &PathBuf::from(path), stem);
                }
            }
            DialogResult::Answer { tag, yes } if tag.starts_with("delete-clip:") => {
                if *yes {
                    let path = PathBuf::from(&tag["delete-clip:".len()..]);
                    delete_clip(state, &path);
                }
            }
            _ => keep.push(r),
        }
    }
    state.dialogs.results = keep;
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

fn delete_clip(state: &mut AppState, path: &Path) {
    let imports = imported_roots(state);
    let owned = state.library.clips_root.clone();
    let result = match classify_media_path(path, &owned, &imports) {
        Ownership::FthrOwned => {
            fthrust_core::library_ownership::delete_owned_media(path, &owned, &imports)
                .map_err(|e| e.to_string())
        }
        Ownership::LinkedImported => std::fs::remove_file(path).map_err(|e| e.to_string()),
        Ownership::External => Err("Linked or external media is protected".into()),
    };
    match result {
        Ok(()) => {
            let _ = std::fs::remove_file(fthrust_core::audio_manifest::manifest_path_for(path));
            state.library.remove(path);
            state.thumb_textures.remove(path);
            state.thumb_meta.remove(path);
            state.readiness.forget(path);
            state.library_dirty = true;
        }
        Err(e) => state.dialogs.push_message("File change failed", e),
    }
}

fn rename_clip(state: &mut AppState, path: &Path, stem: &str) {
    match fthrust_core::audio_manifest::rename_clip(path, stem) {
        Ok(new_path) => {
            if new_path != path {
                state.library.rename(path, &new_path);
                if let Some((m, d)) = state.thumb_meta.remove(path) {
                    state.thumb_meta.insert(new_path.clone(), (m, d));
                }
                state.thumb_textures.remove(path);
                state.library_dirty = true;
            }
        }
        Err(e) => state.dialogs.push_message("Rename failed", e.to_string()),
    }
}

/// Keyboard navigation chord of a key event (`Shift+Down`, `Tab+S`).
pub fn chord(modifiers: egui::Modifiers, key: egui::Key, tab_held: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    if modifiers.ctrl || modifiers.command {
        parts.push("Ctrl".into());
    }
    if modifiers.alt {
        parts.push("Alt".into());
    }
    if modifiers.shift {
        parts.push("Shift".into());
    }
    if tab_held && key != egui::Key::Tab {
        parts.push("Tab".into());
    }
    let name = match key {
        egui::Key::ArrowUp => "Up".to_owned(),
        egui::Key::ArrowDown => "Down".to_owned(),
        egui::Key::ArrowLeft => "Left".to_owned(),
        egui::Key::ArrowRight => "Right".to_owned(),
        egui::Key::Enter => "Return".to_owned(),
        egui::Key::Escape => "Escape".to_owned(),
        other => other.name().to_owned(),
    };
    parts.push(name);
    parts.join("+")
}

/// The navigation actions whose chord was pressed this frame (bindings on,
/// category on, no text field focused).
pub fn triggered_actions(
    ctx: &egui::Context,
    state: &AppState,
    category: &str,
) -> Vec<&'static str> {
    if !state.nav.is_enabled()
        || !state.nav.is_category_enabled(category)
        || ctx.egui_wants_keyboard_input()
    {
        return Vec::new();
    }
    let tab_held = ctx.input(|i| i.key_down(egui::Key::Tab));
    let pressed: Vec<String> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => Some(chord(*modifiers, *key, tab_held)),
                // egui-winit turns the clipboard chords into these events and
                // sends no key event for them.
                egui::Event::Copy => Some("Ctrl+C".to_owned()),
                egui::Event::Cut => Some("Ctrl+X".to_owned()),
                egui::Event::Paste(_) => Some("Ctrl+V".to_owned()),
                _ => None,
            })
            .collect()
    });
    fthrust_core::navigation::BINDING_DEFINITIONS
        .iter()
        .filter(|d| d.category == category)
        .filter(|d| {
            state
                .nav
                .get(d.action)
                .is_some_and(|c| !c.is_empty() && pressed.iter().any(|p| p == c))
        })
        .map(|d| d.action)
        .collect()
}

/// The page.
pub fn ui(ui: &mut Ui, state: &mut AppState) {
    let t = state.tokens.clone();
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, t.color(Color::Bg));
    drain_thumbnails(ui, state);
    consume_results(state);
    if state.library_dirty {
        let key = screenshot_key(state);
        state.library.rebuild(&key);
        state.library_dirty = false;
    }
    let now = ui.ctx().input(|i| i.time);
    state.card_feedback.retain(|_, (_, until)| *until > now);
    // Filter bar.
    let inner_w = rect.width() - MARGINS.0 - MARGINS.2;
    let bar = Rect::from_min_size(
        Pos2::new(rect.min.x + MARGINS.0, rect.min.y + MARGINS.1),
        Vec2::new(inner_w, FILTER_H),
    );
    ui.painter().rect_filled(
        bar,
        CornerRadius::same(geom::RADIUS as u8),
        t.color(Color::ShellBg),
    );
    crate::widgets::rule_h(
        ui,
        bar.min.x,
        bar.max.x,
        bar.max.y - 1.0,
        t.color(Color::ShellDivider),
    );
    fidelity::mark(ui, "filterBar", bar)
        .fill(Color::ShellBg)
        .edge(Color::ShellBg)
        .top(Color::ShellBg)
        .done();
    let heading = state.library.heading_text();
    let hg = galley(
        ui,
        &heading,
        size::LABEL,
        Weight::W600,
        t.color(Color::Text),
    );
    let hrect = Rect::from_min_size(
        Pos2::new(bar.min.x + 12.0, bar.min.y),
        Vec2::new(hg.size().x.ceil(), FILTER_H - 1.0),
    );
    paint_text(ui, hg, hrect, Align::Min, 0.0, t.color(Color::Text));
    fidelity::mark(ui, "allCountLabel", hrect)
        .edge(Color::ShellBg)
        .top(Color::ShellBg)
        .text(&heading, size::LABEL, Weight::W600)
        .done();
    let combo_w = 140.0;
    let sort_x = bar.max.x - 12.0 - combo_w;
    let filter_x = sort_x - 12.0 - combo_w;
    let filter_idx = FILTERS
        .iter()
        .position(|(_, f)| *f == state.library.filter)
        .unwrap_or(0);
    let mut fc = Combo::new(
        FILTERS.iter().map(|(l, _)| l.to_string()).collect(),
        filter_idx,
    )
    .with_width(combo_w);
    fc.background = Color::CardBgHi;
    let name = format!("_DropdownCombo[{}]", fc.current());
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(filter_x, bar.min.y + 10.0),
        Vec2::new(combo_w, 38.0),
    )));
    if fc.ui(&mut c, &t, &mut state.icons, &name).changed() {
        state.library.filter = FILTERS[fc.selected].1;
        state.library_dirty = true;
        if let Some(p) = &state.thumb_pool {
            p.cancel_generation();
        }
        state.thumb_requested.clear();
    }
    let sort_idx = SORTS
        .iter()
        .position(|(_, s)| *s == state.library.sort)
        .unwrap_or(0);
    let mut sc = Combo::new(SORTS.iter().map(|(l, _)| l.to_string()).collect(), sort_idx)
        .with_width(combo_w);
    sc.background = Color::CardBgHi;
    let name = format!("_DropdownCombo[{}]", sc.current());
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
        Pos2::new(sort_x, bar.min.y + 10.0),
        Vec2::new(combo_w, 38.0),
    )));
    if sc.ui(&mut c, &t, &mut state.icons, &name).changed() {
        state.library.sort = SORTS[sc.selected].1;
        state.library_dirty = true;
    }
    if state.library_dirty {
        let key = screenshot_key(state);
        state.library.rebuild(&key);
        state.library_dirty = false;
    }
    // Scroll area below the bar (8 px gap); the 8 px scrollbar shows only when the content overflows.
    let area_rect = Rect::from_min_max(
        Pos2::new(rect.min.x + MARGINS.0, bar.max.y + 8.0),
        Pos2::new(rect.max.x - MARGINS.2, rect.max.y),
    );
    let height_id = egui::Id::unique("library-content-height");
    let last_h: f32 = ui.ctx().data_mut(|d| d.get_temp(height_id)).unwrap_or(0.0);
    let overflow = last_h > area_rect.height();
    let content_w = if overflow {
        area_rect.width() - 8.0
    } else {
        area_rect.width()
    };
    let scrolled_now = ui.ctx().input(|i| i.smooth_scroll_delta() != Vec2::ZERO);
    if scrolled_now {
        state.library_last_scroll = now;
    }
    let settling = now - state.library_last_scroll < SCROLL_SETTLE_S;
    let mut area = ui.new_child(egui::UiBuilder::new().max_rect(area_rect));
    let mut visible: Vec<PathBuf> = Vec::new();
    let mut content_h = 0.0f32;
    let mut rename_commit: Option<(PathBuf, String)> = None;
    let mut delete_req: Option<PathBuf> = None;
    let mut share_req: Option<PathBuf> = None;
    let mut open_req: Option<PathBuf> = None;
    // Up/Down (keyboard navigation) queued a one-row step last frame.
    let kb_id = egui::Id::unique("library-kb-scroll");
    let kb_step: Option<f32> = ui.ctx().data_mut(|d| d.remove_temp(kb_id));
    let mut scroll_area = scroll::vertical(&area, &t, "library");
    if let Some(step) = kb_step {
        scroll_area = scroll_area.vertical_scroll_offset((state.library_scroll + step).max(0.0));
    }
    let output = scroll_area.show_viewport(&mut area, |ui, viewport| {
        ui.set_width(area_rect.width());
        let origin = ui.cursor().min;
        let x0 = origin.x;
        let mut y = origin.y;
        if let Some(empty) = state.library.empty.clone() {
            // Empty state: 96 px top margin, title, 4 px, detail, 12 px, 56×2 accent rule.
            y += 96.0;
            let th = crate::fonts::qt_wrapped_line_height(26.0, Weight::W600);
            let trect = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(content_w, th));
            let g = galley(ui, &empty.title, 26.0, Weight::W600, t.color(Color::Text));
            paint_text(ui, g, trect, Align::Center, 0.0, t.color(Color::Text));
            fidelity::mark(ui, format!("QLabel[{}]", empty.title), trect)
                .edge(Color::Bg)
                .top(Color::Bg)
                .text(&empty.title, 26.0, Weight::W600)
                .done();
            y += th;
            if let Some(detail) = &empty.detail {
                y += 4.0;
                let dh = crate::fonts::qt_wrapped_line_height(size::LABEL, Weight::W400);
                let drect = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(content_w, dh));
                let g = galley(
                    ui,
                    detail,
                    size::LABEL,
                    Weight::W400,
                    t.color(Color::TextDim),
                );
                paint_text(ui, g, drect, Align::Center, 0.0, t.color(Color::TextDim));
                fidelity::mark(
                    ui,
                    format!("QLabel[{}]", fidelity::marker_label(detail)),
                    drect,
                )
                .text(detail, size::LABEL, Weight::W400)
                .done();
                y += dh;
            }
            y += 12.0;
            let rule = Rect::from_min_size(
                Pos2::new(x0 + (content_w - 56.0) / 2.0, y),
                Vec2::new(56.0, 2.0),
            );
            ui.painter().rect_filled(rule, 0.0, t.color(Color::Accent));
            y += 2.0;
            content_h = y - origin.y + MARGINS.3;
            ui.allocate_rect(
                Rect::from_min_max(origin, Pos2::new(x0 + content_w, origin.y + content_h)),
                Sense::hover(),
            );
            return;
        }
        let (cols, widths) = metrics(content_w + H_MARGIN);
        let card_h = card_height(widths[0]);
        let row_h = card_h + GRID_SPACING;
        let mut card_index = 0usize;
        let sections = state.library.sections.clone();
        for (si, section) in sections.iter().enumerate() {
            if si > 0 {
                y += SECTION_SPACING;
            }
            // Header row: date (17/600) + "·  game" (15 TEXT_DIM), spacing 10, then 8 px.
            let hh = crate::fonts::qt_wrapped_line_height(size::BODY_L, Weight::W600);
            let dg = galley(
                ui,
                &section.label,
                size::BODY_L,
                Weight::W600,
                t.color(Color::Text),
            );
            let drect = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(dg.size().x.ceil(), hh));
            if drect.intersects(viewport) {
                paint_text(ui, dg, drect, Align::Min, 0.0, t.color(Color::Text));
                fidelity::mark(ui, format!("sectionHeader[{}]", section.label), drect)
                    .text(&section.label, size::BODY_L, Weight::W600)
                    .done();
                if let Some(game) = &section.game {
                    let text = format!("·  {game}");
                    let gg = galley(ui, &text, size::BODY, Weight::W400, t.color(Color::TextDim));
                    let grect = Rect::from_min_size(
                        Pos2::new(drect.max.x + 10.0, y),
                        Vec2::new(gg.size().x.ceil(), hh),
                    );
                    paint_text(ui, gg, grect, Align::Min, 0.0, t.color(Color::TextDim));
                }
            }
            y += hh + 8.0;
            let rows = section.paths.len().div_ceil(cols);
            for row in 0..rows {
                let row_top = y + row as f32 * row_h;
                let row_rect =
                    Rect::from_min_size(Pos2::new(x0, row_top), Vec2::new(content_w, card_h));
                let near = row_rect.min.y < viewport.max.y + OVERSCAN_ROWS as f32 * row_h
                    && row_rect.max.y > viewport.min.y - OVERSCAN_ROWS as f32 * row_h;
                if near {
                    let mut x = x0;
                    for (col, &w) in widths.iter().enumerate().take(cols) {
                        let Some(path) = section.paths.get(row * cols + col) else {
                            break;
                        };
                        let crect =
                            Rect::from_min_size(Pos2::new(x, row_top), Vec2::new(w, card_h));
                        let on_screen = crect.intersects(viewport);
                        visible.push(path.clone());
                        let is_video = fthrust_core::clip_files::is_completed_video_path(path);
                        let (metadata, duration) =
                            state.thumb_meta.get(path).cloned().unwrap_or_else(|| {
                                (
                                    if is_video {
                                        "Reading clip details…".to_owned()
                                    } else {
                                        "Screenshot".to_owned()
                                    },
                                    None,
                                )
                            });
                        let thumbnail = state.thumb_textures.get(path).map(|e| {
                            (
                                e.texture.id(),
                                Vec2::new(e.texture.size()[0] as f32, e.texture.size()[1] as f32),
                            )
                        });
                        if thumbnail.is_none()
                            && on_screen
                            && !settling
                            && !state.thumb_failed.contains(path)
                            && !state.thumb_requested.contains(path)
                        {
                            let fp = fingerprint(state, path);
                            let queued = match &state.thumb_pool {
                                Some(pool) => pool.request(path, &fp, is_video),
                                None => true,
                            };
                            if queued {
                                state.thumb_requested.insert(path.clone());
                                state.thumb_requests_total += 1;
                            }
                        }
                        let imported = state
                            .library
                            .records
                            .iter()
                            .any(|r| r.path == *path && r.imported);
                        let ready = state.readiness.can_access(path);
                        let data = CardData {
                            path: path.clone(),
                            title: super::model::title_from_path(path),
                            metadata,
                            duration_s: if is_video {
                                Some(duration.unwrap_or(0.0))
                            } else {
                                None
                            },
                            is_video,
                            imported,
                            uploaded: false,
                            upload_link: None,
                            upload_enabled: state.flag("upload_enabled", false),
                            ready,
                            feedback: state.card_feedback.get(path).map(|(f, _)| f.clone()),
                            thumbnail,
                        };
                        let mut editing = match &state.rename_draft {
                            Some((p, draft)) if p == path => Some(draft.clone()),
                            _ => None,
                        };
                        let was_editing = editing.is_some();
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(crect));
                        let resp = card(
                            &mut child,
                            &t,
                            &mut state.icons,
                            w,
                            &data,
                            &mut editing,
                            &format!("clipCard[{card_index}]"),
                        );
                        match (was_editing, editing) {
                            (_, Some(draft)) => state.rename_draft = Some((path.clone(), draft)),
                            (true, None) => state.rename_draft = None,
                            _ => {}
                        }
                        if let Some(stem) = resp.rename_committed {
                            rename_commit = Some((path.clone(), stem));
                        }
                        if resp.delete_requested {
                            delete_req = Some(path.clone());
                        }
                        if resp.share_clicked {
                            share_req = Some(path.clone());
                        }
                        if resp.opened {
                            open_req = Some(path.clone());
                        }
                        card_index += 1;
                        x += w + GRID_SPACING;
                    }
                } else {
                    card_index += (section.paths.len() - row * cols).min(cols);
                }
            }
            y += rows as f32 * row_h - GRID_SPACING;
        }
        content_h = y - origin.y + MARGINS.3;
        ui.allocate_rect(
            Rect::from_min_max(origin, Pos2::new(x0 + content_w, origin.y + content_h)),
            Sense::hover(),
        );
    });
    state.library_scroll = output.state.clamped_offset().y;
    if (content_h - last_h).abs() > 0.5 {
        ui.ctx().data_mut(|d| d.insert_temp(height_id, content_h));
        ui.ctx().request_repaint();
    }
    state.visible_cards = visible;
    if let Some((path, stem)) = rename_commit {
        state.rename_draft = None;
        let imports = imported_roots(state);
        let owned = state.library.clips_root.clone();
        if classify_media_path(&path, &owned, &imports) == Ownership::LinkedImported {
            // The reference confirms before touching a linked original.
            let name = path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            state.dialogs.push_modal(DialogKind::Question {
                title: "Rename linked original".into(),
                message: format!(
                    "Rename {name} to {stem}?\nThe original file outside the library is renamed too."
                ),
                tag: format!("rename-clip:{}\u{1f}{stem}", path.display()),
            });
        } else {
            rename_clip(state, &path, &stem);
        }
    }
    if settling {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(SCROLL_SETTLE_S));
    }
    if let Some(path) = delete_req {
        let imports = imported_roots(state);
        let owned = state.library.clips_root.clone();
        let imported = classify_media_path(&path, &owned, &imports) == Ownership::LinkedImported;
        if imported {
            state.dialogs.push_message(
                "Linked Original Protected",
                "Use the dedicated Delete button to review deletion of this linked original.",
            );
        } else {
            let name = path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            state.dialogs.push_modal(DialogKind::Question {
                title: "Delete clip".into(),
                message: format!("Delete {name}?\nThis cannot be undone."),
                tag: format!("delete-clip:{}", path.display()),
            });
        }
    }
    if let Some(path) = share_req {
        state
            .card_feedback
            .insert(path.clone(), ("Upload requested".into(), now + 2.0));
        state.upload_requested = Some(path);
    }
    if let Some(path) = open_req {
        if fthrust_core::clip_files::is_completed_video_path(&path) {
            state.open_clip_requested = Some(path);
        } else {
            state.open_screenshot_requested = Some(path);
        }
    }
    // Keyboard: Up/Down scroll one row (Clips category).
    for action in triggered_actions(ui.ctx(), state, "Clips") {
        let (_, widths) = metrics(content_w + H_MARGIN);
        let step = card_height(widths[0]) + GRID_SPACING;
        let delta = match action {
            "library_scroll_up" => -step,
            "library_scroll_down" => step,
            _ => 0.0,
        };
        if delta != 0.0 {
            let id = egui::Id::unique("library-kb-scroll");
            ui.ctx().data_mut(|d| d.insert_temp(id, delta));
            ui.ctx().request_repaint();
        }
    }
    let _ = MediaKind::Video;
    let _ = Color32::WHITE;
}
