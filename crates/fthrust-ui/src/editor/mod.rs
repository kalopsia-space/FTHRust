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

pub mod dialogs;
pub mod export_ui;
pub mod filmstrip;
pub mod keys;
pub mod layer;
pub mod sessions;
pub mod sidebar;
pub mod timeline_ui;

/// Builds the player for a clip (real: PipeWire output and the wall clock).
pub type PlayerFactory = Arc<dyn Fn(&Path) -> Result<PlaybackService, String> + Send + Sync>;

/// The loader thread's result: the player and the manifest's source labels.
type Loaded = (Result<PlaybackService, String>, Vec<(String, String)>);

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
/// Filmstrip load after the player is ready / after a pause.
const FILMSTRIP_IDLE_S: f64 = 0.3;
/// Filmstrip load after a pause.
const FILMSTRIP_PAUSE_S: f64 = 1.5;
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
    /// Open the crop dialog.
    OpenCrop,
    /// Open the stretch dialog.
    OpenStretch,
}

/// What the timeline header label shows (the last change wins).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LabelMode {
    /// `m:ss → m:ss` (trim moved, and initially).
    Range,
    /// `{kept} kept  ·  {n} cuts` (segments changed).
    Kept,
}

/// One AUDIO MIX source row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRow {
    /// Source id (manifest uuid).
    pub id: String,
    /// Display name.
    pub label: String,
    /// Gain 0–100.
    pub gain: u8,
    /// Muted.
    pub muted: bool,
    /// The track decodes.
    pub available: bool,
}

/// FILE DETAILS metadata.
#[derive(Clone, Debug, PartialEq)]
pub enum MetaState {
    /// Probing.
    Loading,
    /// Probed.
    Ready(fthrust_media::probe::VideoMetadata),
    /// The probe failed.
    Unavailable,
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
    /// Timeline filmstrip.
    pub filmstrip: filmstrip::Filmstrip,
    /// The clip is a linked import (resolved once: it touches the disk).
    pub linked_import: Option<bool>,
    /// FILE DETAILS "Created" (local time) and size in MB, read once.
    pub file_info: Option<(time::OffsetDateTime, f64)>,
    /// Source position of an open timeline context menu.
    pub context_pct: Option<f64>,
    filmstrip_due: Option<f64>,
    label_mode: LabelMode,
    /// `None` until the first selection change (initial "Delete segment").
    segment_button: Option<bool>,
    player: Option<PlaybackService>,
    loading: Option<Receiver<Loaded>>,
    meta: MetaState,
    meta_rx: Option<Receiver<Option<fthrust_media::probe::VideoMetadata>>>,
    source_labels: Vec<(String, String)>,
    failed_sources: Vec<String>,
    test_rows: Option<Vec<SourceRow>>,
    status: String,
    status_clear_at: Option<f64>,
    opened_at: Option<f64>,
    play_when_ready: bool,
    restored: bool,
    pending_seek_ms: Option<u64>,
    seek_due: Option<f64>,
    next_playhead_check: f64,
    draft_due: Option<f64>,
    /// An edit since the last draft write (`_draft_dirty`).
    draft_dirty: bool,
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
                let labels = fthrust_core::audio_manifest::read_verified(&p)
                    .ok()
                    .flatten()
                    .map(|m| {
                        m.sources
                            .iter()
                            .map(|s| (s.source_uuid.clone(), s.display_name.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let _ = tx.send((factory(&p), labels));
            })
            .map_err(|e| e.to_string())?;
        let (mtx, mrx) = channel();
        let p = path.to_path_buf();
        std::thread::Builder::new()
            .name("editor-probe".into())
            .spawn(move || {
                let _ = mtx.send(fthrust_media::probe::probe_video_metadata(&p));
            })
            .map_err(|e| e.to_string())?;
        let mut e = Editor::blank(path, 1);
        e.loading = Some(rx);
        e.meta_rx = Some(mrx);
        e.master_volume = settings
            .get_i64("master_volume")
            .unwrap_or(80)
            .clamp(0, 100) as u8;
        e.timeline.hide_deleted = settings
            .get_bool("hide_deleted_timeline_segments")
            .unwrap_or(false);
        let entry = store.get(path).map_err(|e| e.to_string())?;
        e.tag = entry.tag;
        e.description = entry.description;
        if let Ok(identity) = file_identity(path)
            && let Ok(Some(draft)) = store.editor_draft_if_current(path, &identity)
            && let Some(s) = decode_state(&draft.state)
        {
            e.apply_snapshot(&s);
            e.last_persisted = Some(s);
            if e.timeline.segments.len() > 1 {
                e.segments_changed();
            }
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
            filmstrip: filmstrip::Filmstrip::default(),
            context_pct: None,
            linked_import: None,
            file_info: None,
            filmstrip_due: None,
            label_mode: LabelMode::Range,
            segment_button: None,
            player: None,
            loading: None,
            meta: MetaState::Loading,
            meta_rx: None,
            source_labels: Vec::new(),
            failed_sources: Vec::new(),
            test_rows: None,
            status: "Preparing clip…".into(),
            status_clear_at: None,
            opened_at: None,
            play_when_ready: false,
            restored: false,
            pending_seek_ms: None,
            seek_due: None,
            next_playhead_check: 0.0,
            draft_due: None,
            draft_dirty: false,
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

    /// Each track's `0:a:N` index, parallel to [`Editor::sources`].
    pub fn source_audio_indexes(&self) -> Vec<Option<u32>> {
        self.player
            .as_ref()
            .map(|p| p.audio_indexes().to_vec())
            .unwrap_or_default()
    }

    /// AUDIO MIX source rows (manifest names, else "Track n").
    pub fn source_rows(&self) -> Vec<SourceRow> {
        if let Some(rows) = &self.test_rows {
            return rows.clone();
        }
        self.sources()
            .into_iter()
            .enumerate()
            .map(|(i, (id, st))| SourceRow {
                label: self
                    .source_labels
                    .iter()
                    .find(|(sid, _)| *sid == id)
                    .map(|(_, l)| l.clone())
                    .filter(|l| !l.trim().is_empty())
                    .unwrap_or_else(|| format!("Track {}", i + 1)),
                available: !self.failed_sources.contains(&id),
                gain: st.gain_percent,
                muted: st.muted,
                id,
            })
            .collect()
    }

    /// Replaces the source rows (UI tests without a player).
    pub fn set_source_rows_for_tests(&mut self, rows: Vec<(String, String)>) {
        self.test_rows = Some(
            rows.into_iter()
                .map(|(id, label)| SourceRow {
                    id,
                    label,
                    gain: 100,
                    muted: false,
                    available: true,
                })
                .collect(),
        );
    }

    /// FILE DETAILS metadata.
    pub fn meta(&self) -> &MetaState {
        &self.meta
    }

    /// Seconds until the earliest pending timer (`None`: nothing due), so
    /// the layer can wake the loop for it instead of the 1 s heartbeat.
    pub fn next_wake(&self, now_s: f64) -> Option<f64> {
        if self.lifecycle == Lifecycle::Playing {
            return Some(0.0);
        }
        let polling = (self.loading.is_some() || self.meta_rx.is_some()).then_some(now_s + 0.05);
        [
            self.seek_due,
            self.draft_due,
            self.status_clear_at,
            self.split_feedback_until,
            self.saved_feedback_until,
            self.filmstrip_due,
            polling,
        ]
        .into_iter()
        .flatten()
        .reduce(f64::min)
        .map(|t| (t - now_s).max(0.0))
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
        settings: &mut SettingsStore,
        store: &mut ClipMetadataStore,
    ) {
        self.last_now = now_s;
        if let Some(rx) = &self.meta_rx
            && let Ok(m) = rx.try_recv()
        {
            self.meta = m.map_or(MetaState::Unavailable, MetaState::Ready);
            self.meta_rx = None;
        }
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
                match ev {
                    PlaybackEvent::Eof => self.end_of_edit(),
                    PlaybackEvent::SourceFailed(id) => self.failed_sources.push(id),
                    _ => {}
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
        if self.filmstrip_due.is_some_and(|t| now_s >= t) && self.lifecycle != Lifecycle::Playing {
            self.filmstrip_due = None;
            let enabled = settings_strip_enabled(settings);
            if enabled && self.filmstrip.state() == filmstrip::FilmstripState::Idle {
                let d = self.duration_ms;
                self.filmstrip
                    .request(&self.path, d, filmstrip::frame_count(d));
            }
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
            Ok((Err(e), _)) => {
                tracing::warn!(error = %e, path = %self.path.display(), "editor could not open the clip");
                self.loading = None;
                self.fail("CLIP UNAVAILABLE");
            }
            Ok((Ok(player), labels)) => {
                self.loading = None;
                self.source_labels = labels;
                self.loaded(player, now_s);
            }
        }
    }

    fn loaded(&mut self, mut player: PlaybackService, now_s: f64) {
        let info = player.info().clone();
        self.src_size = (info.width, info.height);
        // The reference clamps the crop into the probed source size.
        self.crop = self
            .crop
            .and_then(|c| fthrust_core::editor::tools::clamp_crop(c, info.width, info.height));
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
        self.filmstrip_due = Some(now_s + FILMSTRIP_IDLE_S);
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
            self.filmstrip_due = Some(self.last_now + FILMSTRIP_PAUSE_S);
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
                self.filmstrip_due = Some(self.last_now + FILMSTRIP_PAUSE_S);
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
        self.filmstrip_due = None;
        if self.filmstrip.state() == filmstrip::FilmstripState::Loading {
            self.filmstrip.cancel();
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
        self.draft_dirty = true;
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
        self.segments_changed();
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
            self.segments_changed();
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
        self.segments_changed();
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
        // The source size is a placeholder until the player reports it.
        if self.player.is_none() {
            return false;
        }
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
    // Timeline header state

    /// Segments changed (`_on_segments_changed`): the label shows the kept
    /// summary and the segment button its selection state.
    pub fn segments_changed(&mut self) {
        self.label_mode = LabelMode::Kept;
        self.selection_changed();
    }

    fn selection_changed(&mut self) {
        let deleted = self
            .timeline
            .segments
            .get(self.timeline.selected)
            .is_some_and(|s| s.deleted);
        self.segment_button = Some(deleted);
    }

    /// The timeline header's right label.
    pub fn timeline_label(&self) -> String {
        use fthrust_core::editor::format;
        let d = self.duration_ms as f64;
        match self.label_mode {
            LabelMode::Range => format::range_label(
                (self.timeline.trim_start * d) as u64,
                (self.timeline.trim_end * d) as u64,
            ),
            LabelMode::Kept => {
                let kept: u64 = self
                    .timeline
                    .kept_ranges_ms()
                    .iter()
                    .map(|(s, e)| e - s)
                    .sum();
                format::kept_label(kept, self.timeline.cut_count())
            }
        }
    }

    /// Segment button: text, enabled, restore styling.
    pub fn segment_button(&self) -> (&'static str, bool, bool) {
        match self.segment_button {
            None => ("Delete segment", false, false),
            Some(deleted) => (
                if deleted {
                    "RESTORE SEGMENT"
                } else {
                    "DELETE SEGMENT"
                },
                self.timeline.segments.len() > 1,
                deleted,
            ),
        }
    }

    /// Split button text.
    pub fn split_label(&self) -> &'static str {
        if self.split_feedback_until.is_some() {
            "Move playhead"
        } else {
            "Split"
        }
    }

    /// Selects the segment `index`.
    pub fn select(&mut self, index: usize) {
        if index < self.timeline.segments.len() {
            self.timeline.selected = index;
            self.selection_changed();
        }
    }

    /// Selects the segment under `pct` (a track click).
    pub fn select_at(&mut self, pct: f64) {
        if let Some(i) = self.timeline.segment_index_at(pct) {
            self.select(i);
        }
    }

    /// A trim handle moved (`_on_trim_changed`): the label shows the range
    /// and a playhead outside it jumps to the first kept start.
    pub fn on_trim_changed(&mut self, now_s: f64) {
        self.label_mode = LabelMode::Range;
        let d = self.duration_ms as f64;
        let (s, e) = (
            (self.timeline.trim_start * d) as u64,
            (self.timeline.trim_end * d) as u64,
        );
        let pos = self.position_ms();
        if pos < s || pos >= e {
            let target = self
                .timeline
                .kept_ranges_ms()
                .first()
                .map_or(s, |&(k, _)| k);
            let pct = target as f64 / d.max(1.0);
            self.timeline.ensure_visible(pct, true);
            self.scrub_to(pct, now_s);
        }
        self.schedule_draft(now_s);
    }

    /// "Split here" from the context menu.
    pub fn split_at(&mut self, pct: f64, now_s: f64) -> bool {
        self.timeline.playhead = pct;
        self.split(now_s)
    }

    /// Context-menu Delete/Restore of segment `index`.
    pub fn toggle_segment(&mut self, index: usize, now_s: f64) -> bool {
        self.select(index);
        self.toggle_selected(now_s)
    }

    /// "Hide deleted segments" (persisted as `hide_deleted_timeline_segments`).
    pub fn set_hide_deleted(&mut self, on: bool, settings: &mut SettingsStore) {
        if self.timeline.hide_deleted == on {
            return;
        }
        self.timeline.hide_deleted = on;
        self.timeline.view_start = 0.0;
        let p = self.timeline.playhead;
        self.timeline.ensure_visible(p, false);
        settings.set("hide_deleted_timeline_segments", json!(on));
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
        if !self.draft_dirty {
            return;
        }
        self.draft_dirty = false;
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
        if self.timeline.segments.len() > 1 {
            self.segments_changed();
        }
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

fn settings_strip_enabled(settings: &SettingsStore) -> bool {
    settings
        .get_bool("timeline_preview_strip_enabled")
        .unwrap_or(true)
}
