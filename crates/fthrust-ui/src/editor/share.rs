//! Share (`ShareModeDialog` cv:3356, `ShareWindow` cv:3524): a picker
//! chooses full quality, Discord or a custom size preset; each share then
//! runs in its own small window through the export queue's slots (not
//! listed in the jobs window) into `{clips root}/Shared`.
//!
//! The reference's ready window is a drag source for the file; winit
//! cannot start a file drag on Wayland, so the ready window offers "Copy
//! path" and "Open folder" instead.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use egui::{
    Align, Area, Color32, ColorImage, CornerRadius, Id, Order, Pos2, Rect, Sense, Stroke,
    StrokeKind, TextureHandle, Ui, Vec2,
};
use fthrust_core::editor::snapshot::Effects;
use fthrust_core::export::graph::{ExportSpec, plan};
use fthrust_core::export::naming::{share_name, speed_adjusted_duration, unique};
use fthrust_core::export::plan::plan_export;
use fthrust_core::export::{QUALITY_OPTIONS, normalize_quality};
use fthrust_core::export_presets::{ExportPreset, ExportPresetStore};
use fthrust_media::export::{ExportState, Job, Quality, random_hex};
use serde_json::json;

use super::export_ui;
use crate::fidelity;
use crate::fonts::Weight;
use crate::icons::{Icon, IconCache};
use crate::services::export::ExportQueue;
use crate::shell::error_bar::Level;
use crate::state::AppState;
use crate::tokens::{Color, Tokens, geom};
use crate::widgets::buttons::{IconButtonStyle, icon_button};
use crate::widgets::combo::Combo;
use crate::widgets::{galley, paint_text};

/// Picker size (fixed 330×350).
pub const PICKER_SIZE: Vec2 = Vec2::new(330.0, 350.0);
/// Share window size (fixed 340×242).
pub const WINDOW_SIZE: Vec2 = Vec2::new(340.0, 242.0);

const HINT: &str = "Lossless uses MKV and may not preview in chat apps. Size presets require Very fast or High quality.";
/// The filename bar's weight (Qt 500; the nearest bundled Oswald is 400).
const LINE_WEIGHT: Weight = Weight::W400;
/// Qt's filename label box is 3 px wider than its text at any length.
const LINE_PAD: f32 = 3.0;
/// How long closing waits for the worker (`join(2.5)`).
const CLOSE_WAIT: Duration = Duration::from_millis(2500);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// What the picker chose.
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /// "Share clip": full quality.
    Plain,
    /// A size preset (Discord or custom).
    Preset(ExportPreset),
}

/// The share window's badge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareStatus {
    /// "Exporting...".
    Exporting,
    /// "Ready".
    Ready,
    /// "Failed".
    Failed,
}

/// The open picker.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerState {
    quality: usize,
    presets: Vec<ExportPreset>,
    preset: usize,
}

enum Event {
    Line(String),
    Done(Result<PathBuf, String>),
}

enum Poster {
    None,
    Loading(Receiver<Option<ColorImage>>),
    Ready(TextureHandle, [usize; 2]),
}

/// One share window.
pub struct ShareWindow {
    /// Id.
    pub id: u64,
    /// The clip.
    pub source: PathBuf,
    /// "SHARE CLIP" or `SHARE  ·  {PRESET}`.
    pub title: String,
    /// Badge.
    pub status: ShareStatus,
    /// The filename bar.
    pub line: String,
    /// The written file once ready.
    pub output: Option<PathBuf>,
    stretch: f64,
    cancel: Arc<AtomicBool>,
    rx: Option<Receiver<Event>>,
    thread: Option<JoinHandle<()>>,
    poster: Poster,
}

impl ShareWindow {
    /// A window in a fixed state (fixtures).
    pub fn for_tests(title: &str, status: ShareStatus, line: &str) -> ShareWindow {
        ShareWindow {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            source: PathBuf::new(),
            title: title.into(),
            status,
            line: line.into(),
            output: None,
            stretch: 1.0,
            cancel: Arc::new(AtomicBool::new(false)),
            rx: None,
            thread: None,
            poster: Poster::None,
        }
    }
}

fn quality_index(value: &str) -> usize {
    let v = normalize_quality(value);
    QUALITY_OPTIONS
        .iter()
        .position(|(_, q)| *q == v)
        .unwrap_or(1)
}

/// The Share button: opens the picker with the saved quality and preset.
pub fn open_picker(state: &mut AppState) {
    let presets = ExportPresetStore::new(&state.paths)
        .custom()
        .unwrap_or_default();
    let selected = state
        .settings
        .get_str("selected_export_preset")
        .unwrap_or("discord")
        .to_owned();
    state.share_picker = Some(PickerState {
        quality: quality_index(
            state
                .settings
                .get_str("export_quality")
                .unwrap_or("quality"),
        ),
        preset: presets
            .iter()
            .position(|p| p.preset_id == selected)
            .unwrap_or(0),
        presets,
    });
}

fn share_failed(state: &mut AppState, detail: &str) {
    state.error_bar.push_error(
        "SHARE FAILED",
        detail,
        Level::Warning,
        Vec::new(),
        None,
        false,
    );
}

/// A picker choice with the picker's `quality` value: refuses Lossless
/// with a size preset, saves the choice and starts the share.
pub fn choose(state: &mut AppState, choice: Choice, quality: &str) {
    let quality = normalize_quality(quality);
    if matches!(choice, Choice::Preset(_)) && quality == "lossless" {
        state.dialogs.push_message(
            "Choose a quality",
            "Lossless cannot meet a fixed file size. Choose Very fast or High quality for this preset.",
        );
        return;
    }
    state.set_setting("export_quality", json!(quality));
    if let Choice::Preset(p) = &choice
        && !p.built_in
    {
        state.set_setting("selected_export_preset", json!(p.preset_id));
    }
    state.share_picker = None;
    start(
        state,
        match choice {
            Choice::Plain => None,
            Choice::Preset(p) => Some(p),
        },
    );
}

/// A simple edit (one range, no effects, watermark, stretch or speed)
/// keeps every audio track (`_preserve_tracks`); otherwise the editor's
/// export graph mixes them.
fn simple(spec: &ExportSpec) -> bool {
    spec.ranges_s.len() == 1
        && spec.effects == Effects::default()
        && spec.watermark.is_none()
        && (spec.stretch - 1.0).abs() <= 0.005
        && (spec.speed - 1.0).abs() < 1e-9
}

fn local_hms() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!("{:02}-{:02}-{:02}", now.hour(), now.minute(), now.second())
}

/// `_create_share_window`: plans the share and opens its window.
fn start(state: &mut AppState, preset: Option<ExportPreset>) {
    state.export_queue.configure(&state.settings);
    let mut spec = match export_ui::spec_for(state) {
        Ok(s) => s,
        Err(e) => return share_failed(state, &e),
    };
    let Some(e) = state.editor.as_ref() else {
        return;
    };
    let source = e.path.clone();
    let (crop, stretch) = (e.crop, e.stretch);
    let poster_ms = ((spec.ranges_s[0].0 + 0.5) * 1000.0) as u64;
    if preset.is_none() && simple(&spec) {
        spec.has_manifest_tracks = false;
    }
    let quality = export_ui::quality(state);
    let dir = state.library.clips_root.join("Shared");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        return share_failed(state, &format!("Cannot create {}: {err}", dir.display()));
    }
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "clip".into());
    let discord = preset.as_ref().is_some_and(|p| p.preset_id == "discord");
    let output = unique(&dir.join(share_name(
        &stem,
        &local_hms(),
        &random_hex(8),
        discord,
        quality == Quality::Lossless,
    )));
    let title = match &preset {
        Some(p) => format!("SHARE  \u{b7}  {}", p.name.to_uppercase()),
        None => "SHARE CLIP".into(),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    let worker = Worker {
        queue: state.export_queue.clone(),
        source: source.clone(),
        output,
        spec,
        preset,
        quality,
        cancel: cancel.clone(),
        tx,
    };
    let thread = std::thread::Builder::new()
        .name("fthr-share-export".into())
        .spawn(move || worker.run());
    let thread = match thread {
        Ok(t) => t,
        Err(err) => return share_failed(state, &err.to_string()),
    };
    let (ptx, prx) = channel();
    let p = source.clone();
    let poster = match std::thread::Builder::new()
        .name("share-poster".into())
        .spawn(move || {
            let _ = ptx.send(super::dialogs::decode(&p, poster_ms, crop));
        }) {
        Ok(_) => Poster::Loading(prx),
        Err(_) => Poster::None,
    };
    state.active_shares.push(source.clone());
    state.share_windows.push(ShareWindow {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        source,
        title,
        status: ShareStatus::Exporting,
        line: "Preparing...".into(),
        output: None,
        stretch,
        cancel,
        rx: Some(rx),
        thread: Some(thread),
        poster,
    });
}

struct Worker {
    queue: ExportQueue,
    source: PathBuf,
    output: PathBuf,
    spec: ExportSpec,
    preset: Option<ExportPreset>,
    quality: Quality,
    cancel: Arc<AtomicBool>,
    tx: Sender<Event>,
}

impl Worker {
    fn run(mut self) {
        let result = self.export();
        let _ = self.tx.send(Event::Done(result));
    }

    fn export(&mut self) -> Result<PathBuf, String> {
        let kept: f64 = self.spec.ranges_s.iter().map(|(a, b)| b - a).sum();
        let expected = speed_adjusted_duration(kept, self.spec.speed);
        let (mut bitrate, mut max_rate, mut target) = (None, None, None);
        if let Some(p) = &self.preset {
            let facts = fthrust_media::export::media_facts(&self.source)?;
            let plan = plan_export(&facts, p, Some(expected));
            let _ = self.tx.send(Event::Line(plan.summary()));
            if plan.reencode_video {
                self.spec.extra_video_filters = plan.preset_filters(&facts);
            }
            self.spec.force_video_encode = plan.reencode_video;
            self.spec.force_audio_encode = plan.reencode_audio;
            self.spec.audio_bitrate_kbps = plan.audio_kbps;
            bitrate = Some(plan.video_kbps);
            max_rate = Some(plan.video_kbps * 115 / 100);
            target = Some(p.target_size_mb);
        }
        let graph = plan(&self.spec)?;
        let job = Job {
            source: self.source.clone(),
            output: self.output.clone(),
            plan: graph,
            quality: self.quality,
            video_bitrate_kbps: bitrate,
            max_rate_kbps: max_rate,
            expected_duration_s: expected,
            target_size_mb: target,
            fail_first_encoder: false,
        };
        let tx = self.tx.clone();
        let r = self.queue.run_share(
            &job,
            &self.cancel,
            &mut |s| {
                let _ = tx.send(Event::Line(s.to_owned()));
            },
            &mut |_| {},
        );
        match r.state {
            ExportState::Completed => Ok(self.output.clone()),
            _ => {
                let line = r
                    .detail
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .map(str::to_owned);
                Err(line.unwrap_or_else(|| "Export failed".into()))
            }
        }
    }
}

/// `keep` characters plus "…" when longer than `limit`.
fn ellipsize(text: &str, keep: usize, limit: usize) -> String {
    if text.chars().count() > limit {
        let mut s: String = text.chars().take(keep).collect();
        s.push('…');
        s
    } else {
        text.to_owned()
    }
}

fn running_done(state: &mut AppState, source: &Path) {
    if let Some(i) = state.active_shares.iter().position(|p| p == source) {
        state.active_shares.remove(i);
    }
}

/// Applies worker updates; called every frame.
pub fn poll(state: &mut AppState) {
    let mut finished = Vec::new();
    let mut failed = false;
    for w in state.share_windows.iter_mut() {
        let Some(rx) = &w.rx else {
            continue;
        };
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Event::Line(text) if w.status == ShareStatus::Exporting => w.line = text,
                Event::Line(_) => {}
                Event::Done(Ok(path)) => {
                    w.status = ShareStatus::Ready;
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    w.line = ellipsize(&name, 40, 42);
                    w.output = Some(path);
                    finished.push(w.source.clone());
                }
                Event::Done(Err(e)) => {
                    w.status = ShareStatus::Failed;
                    w.line = ellipsize(&e, 42, 44);
                    finished.push(w.source.clone());
                    failed = true;
                }
            }
        }
        if w.status != ShareStatus::Exporting {
            w.rx = None;
            if let Some(t) = w.thread.take() {
                let _ = t.join();
            }
        }
    }
    for s in finished {
        running_done(state, &s);
    }
    if failed {
        state.error_bar.push_error(
            "EXPORT FAILED",
            "FFmpeg returned an error. The output file may be incomplete.",
            Level::Warning,
            Vec::new(),
            None,
            false,
        );
    }
}

/// Closes a window: cancels its export, waits up to 2.5 s for the worker
/// (which removes the staged file) and shows nothing further.
pub fn close(state: &mut AppState, id: u64) {
    let Some(i) = state.share_windows.iter().position(|w| w.id == id) else {
        return;
    };
    let mut w = state.share_windows.remove(i);
    w.cancel.store(true, Ordering::Relaxed);
    if let Some(t) = w.thread.take() {
        let deadline = Instant::now() + CLOSE_WAIT;
        while !t.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if t.is_finished() {
            let _ = t.join();
        }
    }
    if w.status == ShareStatus::Exporting {
        running_done(state, &w.source);
    }
}

/// Closes every share window (app exit, before the queue shuts down).
pub fn close_all(state: &mut AppState) {
    let ids: Vec<u64> = state.share_windows.iter().map(|w| w.id).collect();
    for id in ids {
        close(state, id);
    }
}

// ---------------------------------------------------------------------------
// Drawing.

/// The picker (modal) and every share window.
pub fn ui(ctx: &egui::Context, state: &mut AppState) {
    poll(state);
    let screen = ctx.content_rect();
    if state.share_picker.is_some() {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            state.share_picker = None;
        } else {
            Area::new(Id::unique("share-picker-blocker"))
                .order(Order::Foreground)
                .fixed_pos(screen.min)
                .constrain(false)
                .interactable(true)
                .show(ctx, |ui| {
                    ui.allocate_rect(screen, Sense::click_and_drag());
                });
            let pos = (screen.center() - PICKER_SIZE / 2.0).round();
            Area::new(Id::unique("share-picker"))
                .order(Order::Tooltip)
                .fixed_pos(pos)
                .constrain(false)
                .interactable(true)
                .show(ctx, |ui| {
                    draw_picker(ui, state, Rect::from_min_size(pos, PICKER_SIZE))
                });
        }
    }
    let mut closed = Vec::new();
    for i in 0..state.share_windows.len() {
        let pos = Pos2::new(
            screen.max.x - WINDOW_SIZE.x - 16.0,
            screen.min.y + 56.0 + i as f32 * (WINDOW_SIZE.y + 12.0),
        );
        let id = state.share_windows[i].id;
        Area::new(Id::unique(("share-window", id)))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .constrain(false)
            .interactable(true)
            .show(ctx, |ui| {
                if draw_window(ui, state, i, Rect::from_min_size(pos, WINDOW_SIZE)) {
                    closed.push(id);
                }
            });
    }
    for id in closed {
        close(state, id);
    }
    if state
        .share_windows
        .iter()
        .any(|w| w.status == ShareStatus::Exporting)
    {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

#[allow(clippy::too_many_arguments)]
fn text(
    ui: &Ui,
    t: &Tokens,
    r: Rect,
    s: &str,
    px: f32,
    w: Weight,
    c: Color,
    align: Align,
    name: &str,
) {
    let g = galley(ui, s, px, w, t.color(c));
    paint_text(ui, g, r, align, 0.0, t.color(c));
    fidelity::mark(ui, name, r).text(s, px, w).done();
}

fn label_name(s: &str) -> String {
    format!("QLabel[{}]", fidelity::marker_label(s))
}

fn close_button(ui: &mut Ui, t: &Tokens, icons: &mut IconCache, r: Rect, name: &str) -> bool {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    icon_button(
        &mut child,
        t,
        icons,
        Icon::Close,
        14.0,
        IconButtonStyle {
            size: r.size(),
            bg: None,
            hover_bg: Some(Color::Error),
            pressed_bg: Some(Color::Error),
            border: None,
            hover_border: None,
            radius: geom::RADIUS,
            hover_tint: Some(Color::Text),
            enabled: true,
        },
        name,
    )
    .clicked()
}

/// A flat button: `fill`/`border`, text centred; hover lightens to ACCENT.
#[allow(clippy::too_many_arguments)]
fn flat_button(
    ui: &mut Ui,
    t: &Tokens,
    r: Rect,
    label: &str,
    px: f32,
    fill: Color,
    border: Color,
    fg: Color,
) -> bool {
    let resp = ui.interact(r, ui.scope_id().with(("share-btn", label)), Sense::click());
    let (fill, fg) = if resp.hovered() {
        (Color::Accent, Color::Bg)
    } else {
        (fill, fg)
    };
    let radius = CornerRadius::same(geom::RADIUS as u8);
    ui.painter().rect_filled(r, radius, t.color(fill));
    ui.painter().rect_stroke(
        r,
        radius,
        Stroke::new(1.0, t.color(border)),
        StrokeKind::Inside,
    );
    let g = galley(ui, label, px, Weight::W600, t.color(fg));
    paint_text(ui, g, r, Align::Center, 0.0, t.color(fg));
    fidelity::mark(ui, format!("QPushButton[{label}]"), r)
        .text(label, px, Weight::W600)
        .done();
    resp.clicked()
}

fn at(rect: Rect, x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(rect.min + Vec2::new(x, y), Vec2::new(w, h))
}

/// Draws the picker into `rect` (tests render it standalone).
pub fn draw_picker(ui: &mut Ui, state: &mut AppState, rect: Rect) {
    let Some(mut picker) = state.share_picker.clone() else {
        return;
    };
    let t = state.tokens.clone();
    let p = ui.painter();
    p.rect_filled(rect, 0.0, t.color(Color::Bg));
    p.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, t.color(Color::Border)),
        StrokeKind::Inside,
    );
    ui.allocate_rect(rect, Sense::click());
    text(
        ui,
        &t,
        at(rect, 16.0, 14.0, 29.0, 22.0),
        "Share as",
        8.0,
        Weight::W600,
        Color::TextDim,
        Align::Min,
        "QLabel[Share as]",
    );
    let close = close_button(
        ui,
        &t,
        &mut state.icons,
        at(rect, 288.0, 10.0, 30.0, 30.0),
        "shareClose",
    );
    // Quality.
    let labels: Vec<String> = QUALITY_OPTIONS
        .iter()
        .map(|(l, _)| (*l).to_owned())
        .collect();
    let mut combo = Combo::new(labels, picker.quality);
    combo.background = Color::CardBgHi;
    combo.height = 38.0;
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(at(rect, 16.0, 46.0, 298.0, 38.0)));
    let name = format!("WheelSafeComboBox[{}]", combo.current());
    if combo.ui(&mut child, &t, &mut state.icons, &name).changed() {
        picker.quality = combo.selected;
    }
    // Hint, wrapped and vertically centred (QLabel's default alignment).
    let hint_r = at(rect, 16.0, 94.0, 298.0, 94.0);
    let g = ui.painter().layout(
        HINT.to_owned(),
        crate::fonts::id(11.0, Weight::W400),
        t.color(Color::TextDim),
        hint_r.width(),
    );
    let y = hint_r.center().y - g.size().y / 2.0;
    ui.painter().galley(
        Pos2::new(hint_r.min.x, y.round()),
        g,
        t.color(Color::TextDim),
    );
    fidelity::mark(ui, label_name(HINT), hint_r)
        .text(HINT, 11.0, Weight::W400)
        .done();
    let quality = QUALITY_OPTIONS[picker.quality.min(QUALITY_OPTIONS.len() - 1)].1;
    let mut choice = None;
    if flat_button(
        ui,
        &t,
        at(rect, 16.0, 198.0, 298.0, 40.0),
        "Share clip",
        10.0,
        Color::Text,
        Color::Text,
        Color::Bg,
    ) {
        choice = Some(Choice::Plain);
    }
    if flat_button(
        ui,
        &t,
        at(rect, 16.0, 248.0, 298.0, 40.0),
        "10 MB  \u{b7}  Discord",
        10.0,
        Color::Bg,
        Color::Border,
        Color::Text,
    ) {
        choice = Some(Choice::Preset(ExportPreset::builtin_discord()));
    }
    if !picker.presets.is_empty() {
        let names: Vec<String> = picker.presets.iter().map(|p| p.name.clone()).collect();
        let mut combo = Combo::new(names, picker.preset.min(picker.presets.len() - 1));
        combo.background = Color::CardBgHi;
        combo.height = 36.0;
        combo.width = Some(200.0);
        let mut child =
            ui.new_child(egui::UiBuilder::new().max_rect(at(rect, 16.0, 299.0, 200.0, 36.0)));
        let name = format!("WheelSafeComboBox[{}]", combo.current());
        if combo.ui(&mut child, &t, &mut state.icons, &name).changed() {
            picker.preset = combo.selected;
        }
        if flat_button(
            ui,
            &t,
            at(rect, 226.0, 299.0, 88.0, 34.0),
            "Use",
            14.0,
            Color::Success,
            Color::Success,
            Color::Bg,
        ) {
            choice = picker
                .presets
                .get(picker.preset)
                .cloned()
                .map(Choice::Preset);
        }
    }
    state.share_picker = Some(picker);
    if close {
        state.share_picker = None;
    } else if let Some(c) = choice {
        choose(state, c, quality);
    }
}

/// Draws share window `index` into `rect`; returns whether its close
/// button was clicked.
pub fn draw_window(ui: &mut Ui, state: &mut AppState, index: usize, rect: Rect) -> bool {
    let t = state.tokens.clone();
    let Some(w) = state.share_windows.get_mut(index) else {
        return false;
    };
    if let Poster::Loading(rx) = &w.poster
        && let Ok(img) = rx.try_recv()
    {
        w.poster = match img {
            Some(img) => {
                let size = img.size;
                Poster::Ready(
                    ui.ctx()
                        .load_texture("share-poster", img, egui::TextureOptions::LINEAR),
                    size,
                )
            }
            None => Poster::None,
        };
    }
    let p = ui.painter();
    p.rect_filled(rect, 0.0, t.color(Color::Bg));
    ui.allocate_rect(rect, Sense::click());
    // Header.
    let hdr = at(rect, 0.0, 0.0, rect.width(), 38.0);
    ui.painter().rect_filled(hdr, 0.0, t.color(Color::CardBg));
    ui.painter().hline(
        hdr.x_range(),
        hdr.max.y - 0.5,
        Stroke::new(1.0, t.color(Color::Border)),
    );
    fidelity::mark(ui, "swHdr", hdr)
        .sample("fill", hdr.center(), Color::CardBg)
        .done();
    let tg = galley(ui, &w.title, 8.0, Weight::W600, Color32::WHITE);
    text(
        ui,
        &t,
        at(rect, 16.0, 0.0, tg.size().x.ceil(), 38.0),
        &w.title,
        8.0,
        Weight::W600,
        Color::TextDim,
        Align::Min,
        &label_name(&w.title),
    );
    let (badge, badge_c) = match w.status {
        ShareStatus::Exporting => ("Exporting...", Color::TextDim),
        ShareStatus::Ready => ("Ready", Color::Accent),
        ShareStatus::Failed => ("Failed", Color::Error),
    };
    let bw = galley(ui, badge, 7.0, Weight::W600, Color32::WHITE)
        .size()
        .x
        .ceil();
    text(
        ui,
        &t,
        at(rect, 288.0 - bw, 0.0, bw, 38.0),
        badge,
        7.0,
        Weight::W600,
        badge_c,
        Align::Min,
        &label_name(badge),
    );
    // Thumbnail zone: the poster, cropped and stretched like the edit.
    let zone = at(rect, 0.0, 38.0, rect.width(), 174.0);
    if let Poster::Ready(tex, [pw, ph]) = &w.poster {
        let src = Vec2::new(*pw as f32 * w.stretch as f32, *ph as f32);
        let scale = (310.0 / src.x).min(174.0 / src.y);
        let r = Rect::from_center_size(zone.center(), src * scale);
        ui.painter().image(
            tex.id(),
            r,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    // Ready: copy the path or open the folder (in place of the drag zone).
    if w.status == ShareStatus::Ready
        && let Some(out) = w.output.clone()
    {
        let strip = at(rect, 0.0, 184.0, rect.width(), 28.0);
        ui.painter()
            .rect_filled(strip, 0.0, Color32::from_black_alpha(160));
        let copy_r = Rect::from_min_size(
            Pos2::new(strip.center().x - 96.0, strip.min.y + 3.0),
            Vec2::new(90.0, 22.0),
        );
        let open_r = Rect::from_min_size(
            Pos2::new(strip.center().x + 6.0, strip.min.y + 3.0),
            Vec2::new(90.0, 22.0),
        );
        if flat_button(
            ui,
            &t,
            copy_r,
            "Copy path",
            8.0,
            Color::Surface2,
            Color::Border,
            Color::Text,
        ) {
            ui.ctx().copy_text(out.to_string_lossy().into_owned());
        }
        if flat_button(
            ui,
            &t,
            open_r,
            "Open folder",
            8.0,
            Color::Surface2,
            Color::Border,
            Color::Text,
        ) && let Some(dir) = out.parent()
        {
            crate::shell::open_url(&dir.to_string_lossy());
        }
    }
    // Filename bar.
    let bar = at(rect, 0.0, 213.0, rect.width(), 29.0);
    ui.painter().rect_filled(bar, 0.0, t.color(Color::ShellBg));
    ui.painter().hline(
        bar.x_range(),
        bar.min.y + 0.5,
        Stroke::new(1.0, t.color(Color::Hairline)),
    );
    let line_c = if w.status == ShareStatus::Failed {
        Color::Error
    } else {
        Color::TextDim
    };
    let lw = galley(ui, &w.line, 8.0, LINE_WEIGHT, Color32::WHITE)
        .size()
        .x
        .ceil();
    let line = w.line.clone();
    text(
        ui,
        &t,
        at(
            rect,
            14.0,
            213.0,
            (lw + LINE_PAD).min(rect.width() - 28.0),
            29.0,
        ),
        &line,
        8.0,
        LINE_WEIGHT,
        line_c,
        Align::Min,
        &label_name(&line),
    );
    let close_r = at(rect, rect.width() - 8.0 - 30.0, 4.0, 30.0, 30.0);
    close_button(ui, &t, &mut state.icons, close_r, "shareWindowClose")
}
