//! The clip editor: one open clip's controller (this module), its egui
//! layer and the session manager.
//!
//! The controller owns the editable state (timeline, crop, stretch,
//! effects, speed), the undo stack and the playback service. Time comes in
//! through `now_s` so tests drive every timer; the player is built on a
//! worker thread because opening it probes and decodes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use fthrust_core::clip_metadata::{ClipMetadataStore, DraftFile, EditorDraft};
use fthrust_core::editor::snapshot::{Effects, Snapshot, UndoStack, decode_state, encode_state};
use fthrust_core::editor::timeline::Timeline;
use fthrust_core::editor::tools::{QuickCrop, normalise_speed};
use fthrust_core::media_identity::file_identity;
use fthrust_core::settings::SettingsStore;
use serde_json::json;

use crate::services::playback::{PlaybackEvent, PlaybackService, SourceState};

pub mod layer;

/// Builds the player for a clip (real: PipeWire output and the wall clock).
pub type PlayerFactory = Arc<dyn Fn(&Path) -> Result<PlaybackService, String> + Send + Sync>;

/// Scrub debounce.
const SEEK_DEBOUNCE_S: f64 = 0.050;
/// Kept-range enforcement interval while playing.
const PLAYHEAD_INTERVAL_S: f64 = 0.080;
/// Slack around kept-range edges.
const RANGE_SLACK_MS: u64 = 25;
/// Play starts inside `[start, end - 60 ms)`.
const PLAY_START_MARGIN_MS: u64 = 60;
/// Draft save debounce.
const DRAFT_DEBOUNCE_S: f64 = 0.450;
/// "Ready" stays this long.
const READY_CLEAR_S: f64 = 0.900;
/// No player after this long: failed.
const STALL_S: f64 = 15.0;
/// "Move playhead" feedback after a refused split.
const SPLIT_FEEDBACK_S: f64 = 1.2;
/// "saved" feedback after saving the quick crop.
const SAVED_FEEDBACK_S: f64 = 1.8;
/// Source size assumed until the player reports it.
const PLACEHOLDER_SIZE: (u32, u32) = (1920, 1080);

/// `PlayerLifecycleState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    /// Loading.
    Preparing,
    /// Loaded, never played.
    Ready,
    /// Play pressed while loading.
    PlayQueued,
    /// Playing.
    Playing,
    /// Paused.
    Paused,
    /// The clip cannot be played.
    Failed,
    /// Closed.
    Closed,
}

/// Something the layer or the session manager must act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorRequest {
    /// Share the edit (plan 6c).
    Share,
    /// Export the edit (plan 6c).
    Export,
    /// Park the editor in the rail.
    Park,
    /// Close the editor.
    Close,
    /// Open the adjacent library clip (+1 / −1).
    Navigate(i32),
    /// Delete the clip file.
    DeleteClip,
}

/// What survives parking (`capture_session_state`).
#[derive(Clone, Debug, PartialEq)]
pub struct SessionState {
    /// The edit.
    pub snapshot: Snapshot,
    /// Playback position.
    pub position_ms: u64,
    /// Undo history.
    pub undo: UndoStack,
    /// Master volume.
    pub master_volume: u8,
    /// `(source id, gain %, muted)`.
    pub source_gains: Vec<(String, u8, bool)>,
    /// Timeline zoom.
    pub zoom: f64,
    /// Timeline view start.
    pub view_start: f64,
}

/// One open clip.
pub struct Editor {
    /// The clip.
    pub path: PathBuf,
    /// Timeline model.
    pub timeline: Timeline,
    /// Crop in source pixels.
    pub crop: Option<(u32, u32, u32, u32)>,
    /// Stretch ratio.
    pub stretch: f64,
    /// Colour and effects.
    pub effects: Effects,
    /// Speed.
    pub speed: f64,
    /// Preserve pitch.
    pub preserve_pitch: bool,
    /// Undo history.
    pub undo: UndoStack,
    /// Lifecycle.
    pub lifecycle: Lifecycle,
    /// Tag (CLIP DETAILS).
    pub tag: String,
    /// Description (CLIP DETAILS).
    pub description: String,
    /// Tag or description edited since the last flush.
    pub details_dirty: bool,
    /// "Move playhead" shows until this time.
    pub split_feedback_until: Option<f64>,
    /// "saved" shows until this time.
    pub saved_feedback_until: Option<f64>,
    /// Requests for the layer and the session manager.
    pub requests: Vec<EditorRequest>,
    /// Master volume 0–100.
    pub master_volume: u8,
    player: Option<PlaybackService>,
    loading: Option<Receiver<Result<PlaybackService, String>>>,
    status: String,
    status_clear_at: Option<f64>,
    opened_at: Option<f64>,
    play_when_ready: bool,
    restored: bool,
    pending_seek_ms: Option<u64>,
    seek_due: Option<f64>,
    next_playhead_check: f64,
    draft_due: Option<f64>,
    last_persisted: Option<Snapshot>,
    gesture: Option<Snapshot>,
    last_now: f64,
    source_gains: Vec<(String, u8, bool)>,
    duration_ms: u64,
    src_size: (u32, u32),
}

impl Editor {
    /// Opens `path`: reads its details and current draft, and starts
    /// loading the player on a worker thread.
    pub fn open(
        path: &Path,
        store: &ClipMetadataStore,
        settings: &SettingsStore,
        factory: PlayerFactory,
    ) -> Result<Editor, String> {
        let (tx, rx) = channel();
        let p = path.to_path_buf();
        std::thread::Builder::new()
            .name("editor-open".into())
            .spawn(move || {
                let _ = tx.send(factory(&p));
            })
            .map_err(|e| e.to_string())?;
        let mut e = Editor::blank(path, 1);
        e.loading = Some(rx);
        e.master_volume = settings
            .get_i64("master_volume")
            .unwrap_or(80)
            .clamp(0, 100) as u8;
        let entry = store.get(path).map_err(|e| e.to_string())?;
        e.tag = entry.tag;
        e.description = entry.description;
        if let Ok(identity) = file_identity(path)
            && let Ok(Some(draft)) = store.editor_draft_if_current(path, &identity)
            && let Some(s) = decode_state(&draft.state)
        {
            e.apply_snapshot(&s);
            e.last_persisted = Some(s);
        }
        Ok(e)
    }

    /// An editor without a player, in `lifecycle` (UI tests).
    pub fn for_tests(path: &Path, duration_ms: u64, lifecycle: Lifecycle) -> Editor {
        let mut e = Editor::blank(path, duration_ms);
        e.lifecycle = lifecycle;
        e.status = match lifecycle {
            Lifecycle::PlayQueued => "Preparing · play queued".into(),
            Lifecycle::Preparing => "Preparing clip…".into(),
            _ => String::new(),
        };
        e
    }

    fn blank(path: &Path, duration_ms: u64) -> Editor {
        Editor {
            path: path.to_path_buf(),
            timeline: Timeline::new(duration_ms),
            crop: None,
            stretch: 1.0,
            effects: Effects::default(),
            speed: 1.0,
            preserve_pitch: true,
            undo: UndoStack::default(),
            lifecycle: Lifecycle::Preparing,
            tag: String::new(),
            description: String::new(),
            details_dirty: false,
            split_feedback_until: None,
            saved_feedback_until: None,
            requests: Vec::new(),
            master_volume: 80,
            player: None,
            loading: None,
            status: "Preparing clip…".into(),
            status_clear_at: None,
            opened_at: None,
            play_when_ready: false,
            restored: false,
            pending_seek_ms: None,
            seek_due: None,
            next_playhead_check: 0.0,
            draft_due: None,
            last_persisted: None,
            gesture: None,
            last_now: 0.0,
            source_gains: Vec::new(),
            duration_ms,
            src_size: PLACEHOLDER_SIZE,
        }
    }

    // ------------------------------------------------------------------
    // Read side

    /// The status label text.
    pub fn status_text(&self) -> &str {
        &self.status
    }

    /// The play button text.
    pub fn play_label(&self) -> &'static str {
        match self.lifecycle {
            Lifecycle::PlayQueued => "Preparing…",
            Lifecycle::Playing => "PAUSE",
            _ => "PLAY",
        }
    }

    /// The player has loaded (successfully).
    pub fn player_loaded(&self) -> bool {
        self.player.is_some()
    }

    /// The player itself (frames, sources).
    pub fn player_mut(&mut self) -> Option<&mut PlaybackService> {
        self.player.as_mut()
    }

    /// The player's own position (tests and the debounce check).
    pub fn player_position_ms(&self) -> Option<u64> {
        self.player.as_ref().map(PlaybackService::position_ms)
    }

    /// The position the time label shows: a pending seek wins.
    pub fn position_ms(&self) -> u64 {
        if let Some(p) = self.pending_seek_ms {
            return p;
        }
        self.player.as_ref().map_or(0, PlaybackService::position_ms)
    }

    /// Source duration.
    pub fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    /// Source size (a 1920×1080 placeholder until loaded).
    pub fn source_size(&self) -> (u32, u32) {
        self.src_size
    }

    /// The clip's audio tracks.
    pub fn sources(&self) -> Vec<(String, SourceState)> {
        self.player
            .as_ref()
            .map(|p| p.sources().to_vec())
            .unwrap_or_default()
    }

    /// The current edit.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot::from_timeline(
            &self.timeline,
            self.crop,
            self.stretch,
            self.effects,
            self.speed,
            self.preserve_pitch,
        )
    }

    // ------------------------------------------------------------------
    // Timers

    /// Advances every timer and the player.
    pub fn tick(
        &mut self,
        now_s: f64,
        _settings: &mut SettingsStore,
        store: &mut ClipMetadataStore,
    ) {
        self.last_now = now_s;
        let opened_at = *self.opened_at.get_or_insert(now_s);
        self.poll_loading(now_s);
        if self.loading.is_some() && now_s - opened_at >= STALL_S {
            self.loading = None;
            self.fail("Playback failed");
        }
        if let Some(at) = self.status_clear_at
            && now_s >= at
        {
            self.status.clear();
            self.status_clear_at = None;
        }
        if self.split_feedback_until.is_some_and(|t| now_s >= t) {
            self.split_feedback_until = None;
        }
        if self.saved_feedback_until.is_some_and(|t| now_s >= t) {
            self.saved_feedback_until = None;
        }
        if self.seek_due.is_some_and(|t| now_s >= t) {
            self.flush_seek();
        }
        if let Some(p) = self.player.as_mut() {
            p.tick();
            for ev in p.events() {
                if ev == PlaybackEvent::Eof {
                    self.end_of_edit();
                }
            }
        }
        if self.lifecycle == Lifecycle::Playing && now_s >= self.next_playhead_check {
            self.next_playhead_check = now_s + PLAYHEAD_INTERVAL_S;
            self.enforce_kept_ranges();
        }
        if self.lifecycle == Lifecycle::Playing || self.pending_seek_ms.is_none() {
            let d = self.duration_ms.max(1) as f64;
            if let Some(p) = &self.player {
                self.timeline.playhead = (p.position_ms() as f64 / d).clamp(0.0, 1.0);
            }
        }
        if self.draft_due.is_some_and(|t| now_s >= t) {
            self.flush_draft(store, now_s);
        }
    }

    fn poll_loading(&mut self, now_s: f64) {
        let Some(rx) = &self.loading else {
            return;
        };
        match rx.try_recv() {
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.loading = None;
                self.fail("CLIP UNAVAILABLE");
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, path = %self.path.display(), "editor could not open the clip");
                self.loading = None;
                self.fail("CLIP UNAVAILABLE");
            }
            Ok(Ok(player)) => {
                self.loading = None;
                self.loaded(player, now_s);
            }
        }
    }

    fn loaded(&mut self, mut player: PlaybackService, now_s: f64) {
        let info = player.info().clone();
        self.src_size = (info.width, info.height);
        if player.duration_ms() != self.duration_ms {
            self.duration_ms = player.duration_ms().max(1);
            self.timeline.duration_ms = self.duration_ms;
        }
        player.set_master_percent(self.master_volume);
        player.set_rate(self.speed as f32, self.preserve_pitch);
        for (id, gain, muted) in &self.source_gains {
            player.set_source(id, Some(*gain), Some(*muted));
        }
        self.player = Some(player);
        self.status = "Ready".into();
        self.status_clear_at = Some(now_s + READY_CLEAR_S);
        self.lifecycle = if self.restored {
            Lifecycle::Paused
        } else {
            Lifecycle::Ready
        };
        self.flush_seek();
        if self.play_when_ready && !self.restored {
            self.play_when_ready = false;
            self.start_playing();
        }
    }

    fn fail(&mut self, status: &str) {
        self.lifecycle = Lifecycle::Failed;
        self.status = status.into();
        self.status_clear_at = None;
        self.play_when_ready = false;
    }

    fn flush_seek(&mut self) {
        self.seek_due = None;
        if let (Some(ms), Some(p)) = (self.pending_seek_ms, self.player.as_mut()) {
            p.seek_ms(ms);
            self.pending_seek_ms = None;
        }
    }

    fn seek_now(&mut self, ms: u64) {
        self.pending_seek_ms = None;
        self.seek_due = None;
        if let Some(p) = self.player.as_mut() {
            p.seek_ms(ms);
        }
        self.next_playhead_check = self.last_now + PLAYHEAD_INTERVAL_S;
        self.timeline.playhead = (ms as f64 / self.duration_ms.max(1) as f64).clamp(0.0, 1.0);
    }

    fn first_kept_start_ms(&self) -> u64 {
        self.timeline
            .kept_ranges_ms()
            .first()
            .map_or(0, |&(s, _)| s)
    }

    /// `_update_playhead`: skip deleted spans; pause and rewind at the end.
    fn enforce_kept_ranges(&mut self) {
        let Some(pos) = self.player.as_ref().map(PlaybackService::position_ms) else {
            return;
        };
        if self
            .timeline
            .kept_range_at_ms(pos, RANGE_SLACK_MS)
            .is_some()
        {
            return;
        }
        match self.timeline.next_kept_start_ms(pos) {
            Some(next) => self.seek_now(next),
            None => self.end_of_edit(),
        }
    }

    fn end_of_edit(&mut self) {
        if let Some(p) = self.player.as_mut() {
            p.pause();
        }
        if matches!(self.lifecycle, Lifecycle::Playing) {
            self.lifecycle = Lifecycle::Paused;
        }
        let first = self.first_kept_start_ms();
        self.seek_now(first);
    }

    // ------------------------------------------------------------------
    // Playback

    /// `_toggle_play`.
    pub fn toggle_play(&mut self, _now_s: f64) {
        match self.lifecycle {
            Lifecycle::Failed | Lifecycle::Closed => {}
            Lifecycle::Preparing => {
                self.play_when_ready = true;
                self.restored = false;
                self.lifecycle = Lifecycle::PlayQueued;
                self.status = "Preparing · play queued".into();
            }
            Lifecycle::PlayQueued => {
                self.play_when_ready = false;
                self.lifecycle = Lifecycle::Preparing;
                self.status = "Preparing clip…".into();
            }
            Lifecycle::Playing => {
                if let Some(p) = self.player.as_mut() {
                    p.pause();
                }
                self.lifecycle = Lifecycle::Paused;
            }
            Lifecycle::Ready | Lifecycle::Paused => self.start_playing(),
        }
    }

    fn start_playing(&mut self) {
        self.flush_seek();
        let Some(pos) = self.player.as_ref().map(PlaybackService::position_ms) else {
            return;
        };
        let ranges = self.timeline.kept_ranges_ms();
        let inside = ranges
            .iter()
            .any(|&(s, e)| s <= pos && pos < e.saturating_sub(PLAY_START_MARGIN_MS));
        if !inside {
            let target = self
                .timeline
                .next_kept_start_ms(pos)
                .unwrap_or_else(|| self.first_kept_start_ms());
            self.seek_now(target);
        }
        if let Some(p) = self.player.as_mut() {
            p.play();
        }
        self.lifecycle = Lifecycle::Playing;
        // The audio worker reports its new position a tick after a seek.
        self.next_playhead_check = self.last_now + PLAYHEAD_INTERVAL_S;
    }

    /// Moves the playhead now and seeks 50 ms after the last call.
    pub fn scrub_to(&mut self, pct: f64, now_s: f64) {
        let pct = pct.clamp(0.0, 1.0);
        self.timeline.playhead = pct;
        let ms = ((pct * self.duration_ms as f64) as u64).min(self.duration_ms);
        self.pending_seek_ms = Some(ms);
        self.seek_due = Some(now_s + SEEK_DEBOUNCE_S);
    }

    // ------------------------------------------------------------------
    // Edits and undo

    fn apply_snapshot(&mut self, s: &Snapshot) {
        s.apply_to(&mut self.timeline);
        self.crop = s.crop;
        self.stretch = s.stretch;
        self.effects = s.effects;
        self.speed = s.speed;
        self.preserve_pitch = s.preserve_pitch;
        if let Some(p) = self.player.as_mut() {
            p.set_rate(self.speed as f32, self.preserve_pitch);
        }
    }

    fn schedule_draft(&mut self, now_s: f64) {
        self.draft_due = Some(now_s + DRAFT_DEBOUNCE_S);
    }

    /// Records one undo step from `before` to the current state.
    pub fn commit(&mut self, before: Snapshot, now_s: f64) {
        let current = self.snapshot();
        if self.undo.commit(before, &current) {
            self.schedule_draft(now_s);
        }
    }

    /// Starts a compound edit (trim drag, effect slider drag).
    pub fn begin_gesture(&mut self) {
        if self.gesture.is_none() {
            self.gesture = Some(self.snapshot());
        }
    }

    /// A compound edit is in progress.
    pub fn in_gesture(&self) -> bool {
        self.gesture.is_some()
    }

    /// Ends a compound edit as one undo step.
    pub fn end_gesture(&mut self, now_s: f64) {
        if let Some(before) = self.gesture.take() {
            self.commit(before, now_s);
        }
    }

    fn restore_edit(&mut self, s: Snapshot, now_s: f64) {
        self.apply_snapshot(&s);
        let pos = self.position_ms();
        if self.timeline.kept_range_at_ms(pos, 0).is_none() {
            let first = self.first_kept_start_ms();
            self.seek_now(first);
        }
        self.schedule_draft(now_s);
    }

    /// Undo.
    pub fn undo(&mut self, now_s: f64) {
        let current = self.snapshot();
        if let Some(s) = self.undo.undo(&current) {
            self.restore_edit(s, now_s);
        }
    }

    /// Redo.
    pub fn redo(&mut self, now_s: f64) {
        let current = self.snapshot();
        if let Some(s) = self.undo.redo(&current) {
            self.restore_edit(s, now_s);
        }
    }

    /// Splits at the playhead; a refusal shows "Move playhead" for 1.2 s.
    pub fn split(&mut self, now_s: f64) -> bool {
        let before = self.snapshot();
        if self.timeline.split_at(self.timeline.playhead) {
            self.commit(before, now_s);
            true
        } else {
            self.split_feedback_until = Some(now_s + SPLIT_FEEDBACK_S);
            false
        }
    }

    /// Sets the selected segment's deleted flag (keyboard delete/restore).
    pub fn set_selected_deleted(&mut self, deleted: bool, now_s: f64) -> bool {
        let Some(s) = self.timeline.segments.get(self.timeline.selected) else {
            return false;
        };
        if s.deleted == deleted || self.timeline.segments.len() < 2 {
            return false;
        }
        self.toggle_selected(now_s)
    }

    /// Delete/Restore button.
    pub fn toggle_selected(&mut self, now_s: f64) -> bool {
        let before = self.snapshot();
        if !self.timeline.toggle_selected() {
            return false;
        }
        self.commit(before, now_s);
        let pos = self.position_ms();
        if self.timeline.kept_range_at_ms(pos, 0).is_none() {
            let target = self
                .timeline
                .next_kept_start_ms(pos)
                .unwrap_or_else(|| self.first_kept_start_ms());
            self.seek_now(target);
        }
        true
    }

    /// Speed combo.
    pub fn set_speed(&mut self, rate: f64, now_s: f64) {
        let before = self.snapshot();
        self.speed = normalise_speed(rate);
        if let Some(p) = self.player.as_mut() {
            p.set_rate(self.speed as f32, self.preserve_pitch);
        }
        self.commit(before, now_s);
    }

    /// Preserve-pitch toggle.
    pub fn set_preserve_pitch(&mut self, on: bool, now_s: f64) {
        let before = self.snapshot();
        self.preserve_pitch = on;
        if let Some(p) = self.player.as_mut() {
            p.set_rate(self.speed as f32, self.preserve_pitch);
        }
        self.commit(before, now_s);
    }

    /// One effect value; outside a gesture it is its own undo step.
    pub fn set_effect(&mut self, key: &str, value: i32, now_s: f64) {
        let before = self.snapshot();
        self.effects.set(key, value);
        if self.gesture.is_none() {
            self.commit(before, now_s);
        }
    }

    /// "Reset look".
    pub fn reset_look(&mut self, now_s: f64) {
        let before = self.snapshot();
        self.effects = Effects::default();
        self.commit(before, now_s);
    }

    /// Stretch "Reset".
    pub fn reset_stretch(&mut self, now_s: f64) {
        let before = self.snapshot();
        self.stretch = 1.0;
        self.commit(before, now_s);
    }

    /// Sets the crop (crop dialog accept) as one undo step.
    pub fn set_crop(&mut self, crop: Option<(u32, u32, u32, u32)>, now_s: f64) {
        let before = self.snapshot();
        self.crop = crop;
        self.commit(before, now_s);
    }

    /// Sets the stretch (stretch dialog accept) as one undo step.
    pub fn set_stretch(&mut self, ratio: f64, now_s: f64) {
        let before = self.snapshot();
        self.stretch = ratio.clamp(0.25, 4.0);
        self.commit(before, now_s);
    }

    /// "Quick": applies the saved quick crop scaled to this source.
    pub fn apply_quick_crop(&mut self, settings: &SettingsStore, now_s: f64) -> bool {
        let Some(qc) = settings.get("quick_crop").and_then(QuickCrop::from_setting) else {
            return false;
        };
        let (w, h) = self.src_size;
        let Some(crop) = qc.apply(w, h) else {
            return false;
        };
        self.set_crop(Some(crop), now_s);
        true
    }

    /// "Save as quick crop".
    pub fn save_quick_crop(&mut self, settings: &mut SettingsStore, now_s: f64) -> bool {
        let Some((x, y, w, h)) = self.crop else {
            return false;
        };
        let (src_w, src_h) = self.src_size;
        let qc = QuickCrop {
            x,
            y,
            w,
            h,
            src_w,
            src_h,
        };
        settings.set("quick_crop", qc.to_setting());
        self.saved_feedback_until = Some(now_s + SAVED_FEEDBACK_S);
        true
    }

    // ------------------------------------------------------------------
    // Mix

    /// Master volume; persisted as `master_volume`.
    pub fn set_master(&mut self, percent: u8, settings: &mut SettingsStore) {
        self.master_volume = percent.min(100);
        settings.set("master_volume", json!(self.master_volume));
        if let Some(p) = self.player.as_mut() {
            p.set_master_percent(self.master_volume);
        }
    }

    /// One track's gain or mute (this open clip only).
    pub fn set_source(&mut self, id: &str, gain: Option<u8>, muted: Option<bool>) {
        if let Some(p) = self.player.as_mut() {
            p.set_source(id, gain, muted);
        }
        let entry = match self.source_gains.iter_mut().find(|(s, _, _)| s == id) {
            Some(e) => e,
            None => {
                self.source_gains.push((id.to_owned(), 100, false));
                self.source_gains.last_mut().expect("just pushed")
            }
        };
        if let Some(g) = gain {
            entry.1 = g.min(100);
        }
        if let Some(m) = muted {
            entry.2 = m;
        }
    }

    // ------------------------------------------------------------------
    // Persistence and sessions

    /// Writes tag and description when edited (`_flush_clip_details`).
    pub fn flush_details(&mut self, store: &mut ClipMetadataStore) {
        if !self.details_dirty {
            return;
        }
        self.details_dirty = false;
        let tag = self.tag.trim().to_owned();
        let description = self.description.trim().to_owned();
        if let Err(e) = store.update(&self.path, Some(&tag), Some(&description)) {
            tracing::warn!(error = %e, "clip details not saved");
        }
    }

    fn flush_draft(&mut self, store: &mut ClipMetadataStore, now_s: f64) {
        let _ = now_s;
        self.draft_due = None;
        let snap = self.snapshot();
        if self.last_persisted.as_ref() == Some(&snap) {
            return;
        }
        let Ok(identity) = file_identity(&self.path) else {
            return;
        };
        let draft = EditorDraft {
            version: 1,
            file: DraftFile {
                mtime_ns: identity.mtime_ns,
                size: identity.size,
            },
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |d| d.as_secs_f64()),
            state: encode_state(&snap),
        };
        match store.set_editor_draft(&self.path, draft) {
            Ok(()) => self.last_persisted = Some(snap),
            Err(e) => tracing::warn!(error = %e, "editor draft not saved"),
        }
    }

    /// Flushes details, a running gesture and the draft; pauses; returns
    /// what parking keeps.
    pub fn capture(&mut self, store: &mut ClipMetadataStore) -> SessionState {
        self.flush_details(store);
        if let Some(before) = self.gesture.take() {
            let current = self.snapshot();
            self.undo.commit(before, &current);
        }
        self.flush_draft(store, 0.0);
        self.play_when_ready = false;
        let position_ms = self.position_ms();
        if let Some(p) = self.player.as_mut() {
            p.pause();
        }
        if self.lifecycle == Lifecycle::Playing {
            self.lifecycle = Lifecycle::Paused;
        }
        SessionState {
            snapshot: self.snapshot(),
            position_ms,
            undo: self.undo.clone(),
            master_volume: self.master_volume,
            source_gains: self.source_gains.clone(),
            zoom: self.timeline.zoom,
            view_start: self.timeline.view_start,
        }
    }

    /// Restores a parked session: paused, seeking once loaded.
    pub fn restore(&mut self, state: SessionState) {
        self.apply_snapshot(&state.snapshot);
        self.undo = state.undo;
        self.master_volume = state.master_volume;
        for (id, gain, muted) in &state.source_gains {
            self.set_source(id, Some(*gain), Some(*muted));
        }
        self.timeline.zoom = state.zoom;
        self.timeline.view_start = state.view_start;
        self.restored = true;
        self.play_when_ready = false;
        if self.lifecycle == Lifecycle::PlayQueued {
            self.lifecycle = Lifecycle::Preparing;
        }
        if self.lifecycle == Lifecycle::Ready || self.lifecycle == Lifecycle::Playing {
            self.lifecycle = Lifecycle::Paused;
        }
        self.pending_seek_ms = Some(
            state
                .position_ms
                .min(self.duration_ms.max(state.position_ms)),
        );
        self.seek_due = Some(0.0);
        self.timeline.playhead =
            (state.position_ms as f64 / self.duration_ms.max(1) as f64).clamp(0.0, 1.0);
    }

    /// Final flush; the player is released.
    pub fn close(mut self, store: &mut ClipMetadataStore) {
        self.flush_details(store);
        if let Some(before) = self.gesture.take() {
            let current = self.snapshot();
            self.undo.commit(before, &current);
        }
        self.flush_draft(store, 0.0);
        self.lifecycle = Lifecycle::Closed;
        if let Some(p) = self.player.take() {
            p.close();
        }
    }
}
