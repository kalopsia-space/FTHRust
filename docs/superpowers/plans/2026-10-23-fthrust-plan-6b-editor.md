# FTHRust Plan 6b: Clip Editor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The clip editor: a full-window editor layer with video playback, the control bar, the trim timeline (segments, split, delete/restore, trim handles, zoom, pan, minimap, filmstrip), undo/redo, persisted drafts, the sidebar panels (speed, edit tools with crop and stretch dialogs, audio mix, colour and effects values, clip and file details, delete), up to three parked sessions in the nav rail, and the editor keyboard shortcuts.

**Architecture:** Everything that is pure editing logic lives in `fthrust-core::editor` (timeline model, snapshot and undo stack, draft codec, quick-crop scaling, formatting) so plan 6c's export reads the same model. `fthrust-ui::editor` holds the `Editor` controller (one open clip: the `PlaybackService` from plan 6a, lifecycle and status text, debounced scrub and draft save, kept-range playhead enforcement, session capture/restore; time is injected so tests drive it), the egui layer (header, video surface, control bar, timeline widget, sidebar, dialogs) and the `Sessions` manager (active editor plus up to two parked `SessionState`s, the nav-rail buttons, clip navigation). The editor renders as a full-window layer inside the single winit window instead of a second toplevel; parking returns to the main shell.

**Tech Stack:** Rust 2024, egui 0.37 (custom painting for the timeline), plan 6a's `services::playback` and `video_texture`, `fthrust-media::playback::video` for filmstrip frames, `ClipMetadataStore` (sqlite drafts, tag and description), `NavigationBindings` for keys.

**Spec:** `docs/superpowers/specs/2026-10-22-fthrust-design.md` (5.7 Editor, 6.4 UI fidelity, 7 `clip_metadata.sqlite3`); the reference digest `docs/superpowers/notes-editor-reference.md` (section numbers below are its sections; `cv:` pointers resolve in `reference/FTHR_UI/ui/clip_viewer.py`); checklist `docs/reference/qt-screens/clip-editor.json` (1440×900).

## Global Constraints

- Fidelity: every widget in an editor Qt dump gets a marker with the dump's name (`docs/ui-fidelity.md`); a screen passes when its `assert_fidelity!` test passes. No screen is built before its Qt PNG/JSON exists (spec section 4); Task 1 renders the missing ones.
- Layout constants: editor minimum 960×600; header 40 px; control bar 46 px; trim bar 198 px containing the 116 px timeline widget; sidebar 280 px wide; timeline padding 14, track `ty=22, th=70`, minimap `y=103, h=7`, press `y >= 98` hits the minimap; 5 ruler ticks; filmstrip slots `max(4, min(14, tw / 92))`.
- Model numbers: positions are fractions 0..1 of source duration; minimum gap `max(0.001, 250 / duration_ms)`; kept pieces of 0.0001 or less are dropped; trim-handle grab radius 9 px in display space; zoom ×1.25 / ×0.8, `max_zoom = max(8, min(2048, duration_ms / 5000))`; pan ±0.7 span; `ensure_visible` offset 0.5 span (0.08 with `align_start`).
- Timing: scrub debounce 50 ms; playhead enforcement every 80 ms with 25 ms slack; play-start window `[start, end - 60 ms)`; draft save debounce 450 ms; undo depth 100; audio-prep deadline 1000 ms; stall deadline 15 s; transient "Move playhead" 1200 ms; "saved" 1800 ms; filmstrip load 300 ms after idle, 1500 ms after pause, cancelled on play.
- Sessions: at most 3 (`MAX_EDITOR_SESSIONS`); restored sessions are always paused; undo/redo stacks survive parking but are not persisted in the draft.
- Draft format: notes section 3 verbatim (`version 1`, `file {mtime_ns,size}`, `updated_at`, `state {…}` keys); validation rules exactly as listed there. The store and its 250-draft cap already exist in `fthrust_core::clip_metadata`.
- Copy: every user-visible string is the reference's (notes section 10), with two rulings recorded in `CHANGES.md`: the segment button is always title case ("Delete segment" / "Restore segment"), and the "Ready" status clears after 900 ms as the reference intended.
- Settings keys read/written: `quick_crop`, `master_volume`, `hide_deleted_timeline_segments`, `timeline_preview_strip_enabled`, `export_quality`, `clip_editor_live_preview`, `editor_color_preview_enabled`, `clip_editor_enabled`, `imported_clip_folders`.
- The UI thread never decodes: filmstrip frames, the player and probes run on workers; the UI only takes ready results.
- Out of scope (plan 6c): running exports and shares, the export-jobs list, the live preview of crop/stretch/colour on the video surface. In 6b the Share and Export buttons push `EditorRequest::{Share, Export}` that nothing handles yet, the quality combo persists `export_quality`, and "Export jobs" is drawn without a count.
- Tests generate clips with the existing writers (`fthrust_media` test support `write_test_clip`, the UI tests' `support` module as `playback_service.rs` uses it); no binary fixtures. UI behaviour is checked with `egui_kittest` and on the bench (`tools/uidrive`), never on the user's monitors.

## Review Focus

1. A clip changed on disk since its draft (mtime or size differ) opens with defaults, and a parked session whose file was deleted shows "CLIP NOT FOUND" / "The editor clip is no longer available." and disappears from the rail. Tests in Task 2 (`draft_for_changed_file_is_ignored`) and Task 7 (`restore_of_deleted_clip_removes_session`).
2. A very short clip (300 ms): the minimum gap covers most of it, split is always refused, trim handles never cross (`start < end` holds after any drag), zoom limit is 8. Test in Task 2 (`short_clip_limits`).
3. A clip with no audio, or audio that fails mid-play: the editor still reaches "Ready", plays video against the video clock, and the Audio mix shows MASTER only. Test in Task 3 (`video_only_clip_plays`).
4. A hand-edited or corrupt draft (NaN, 0 or 501 segments, all deleted, gaps, crop of 1×1): the editor opens with defaults or the lenient fixes, never panics. Test in Task 2 (`draft_validation_matches_reference`).
5. Pressing Tab+Right repeatedly while the next clip is still opening: exactly one player is alive afterwards, every left clip's state was captured, and the editor lands on the last requested clip. Test in Task 7 (`rapid_clip_navigation_keeps_one_player`).

---

## File structure

```
tools/render-qt-reference.py                     + editor variants (Task 1)
docs/reference/qt-screens/clip-editor-{sidebar,timeline,crop,stretch,rail}.{png,json}
crates/fthrust-core/src/editor/mod.rs            re-exports
crates/fthrust-core/src/editor/timeline.rs       Segment, Timeline (trim, split, delete, kept ranges, display map, view)
crates/fthrust-core/src/editor/snapshot.rs       Snapshot, Effects, UndoStack, draft encode/decode
crates/fthrust-core/src/editor/tools.rs          speed presets, QuickCrop, crop clamp
crates/fthrust-core/src/editor/format.rs         m:ss, range/kept labels, created date, location, title elide
crates/fthrust-core/tests/editor_model.rs
crates/fthrust-ui/src/editor/mod.rs              Editor controller, Lifecycle, EditorRequest
crates/fthrust-ui/src/editor/filmstrip.rs        filmstrip worker + LRU
crates/fthrust-ui/src/editor/layer.rs            header, video surface, control bar, layout
crates/fthrust-ui/src/editor/timeline_ui.rs      timeline widget + header row + context menu
crates/fthrust-ui/src/editor/sidebar.rs          sidebar panels
crates/fthrust-ui/src/editor/dialogs.rs          crop and stretch dialogs
crates/fthrust-ui/src/editor/sessions.rs         Sessions manager, clip navigation, delete clip
crates/fthrust-ui/src/editor/keys.rs             shortcut dispatch
crates/fthrust-ui/tests/editor_controller.rs
crates/fthrust-ui/tests/editor_ui.rs
crates/fthrust-ui/tests/editor_sessions.rs
tools/uidrive/scenarios/editor.sh
docs/editor.md
```

---

### Task 1: Qt reference renders for the editor variants

**Files:**
- Modify: `tools/render-qt-reference.py` (after the existing `clip-editor` dump, inside the same patches), `tools/test_render_qt_reference.py`
- Create: `docs/reference/qt-screens/clip-editor-{sidebar,timeline,crop,stretch,rail}.{png,json}`

**Interfaces:**
- Produces the five screens later tasks test against:
  - `clip-editor-sidebar`: the editor's `editorSidebarContent` widget alone, every panel expanded (CLIP SPEED, EDIT TOOLS, AUDIO MIX with MASTER plus two sources "Desktop" and "Mic", COLOR & EFFECTS, CLIP DETAILS, FILE DETAILS), resized to its `sizeHint` height, metadata patch as `clip-editor` (10 s, 1920×1080, 60 fps, 25 Mb/s).
  - `clip-editor-timeline`: the `trimBar` widget after `split_at(0.3)`, `split_at(0.6)`, deleting the middle segment, selecting it, playhead 0.45, zoom 1.25 once.
  - `clip-editor-crop` and `clip-editor-stretch`: `CropDialog` (cv:3084) and `StretchDialog` (cv:2905) opened on the 1920×1080 metadata with the poster pixmap, at their default sizes.
  - `clip-editor-rail`: the main window on the Clips page with two parked sessions (`Overwatch_clip.mp4`, `Valorant_clip.mp4`) so the rail shows "Editor 1" and "Editor 2" buttons.

- [ ] Step 1: Add assertions to `tools/test_render_qt_reference.py`: each of the five JSONs exists, has `widgets`, and contains respectively `audioMixName#3` (third mix row), the `timelineActionDanger` text "RESTORE SEGMENT", a widget of class `CropDialog`, a widget of class `StretchDialog`, and two widgets whose names start with `editorSidebarButton`.
- [ ] Step 2: Run `.venv-reference/bin/pytest tools/test_render_qt_reference.py` → FAIL (files missing).
- [ ] Step 3: Extend the script; render with `.venv-reference/bin/python tools/render-qt-reference.py --out docs/reference/qt-screens`. Where the reference names a widget only by class, keep the extractor's `Class[text]` / `#n` naming.
- [ ] Step 4: Run the pytest → PASS; look at each PNG (Read tool) and confirm it shows the intended state.
- [ ] Step 5: Commit `docs(reference): Qt renders for the editor sidebar, timeline, dialogs and rail`.

---

### Task 2: Editor model in `fthrust-core`

**Files:**
- Create: `crates/fthrust-core/src/editor/{mod,timeline,snapshot,tools,format}.rs`, `crates/fthrust-core/tests/editor_model.rs`
- Modify: `crates/fthrust-core/src/lib.rs` (`pub mod editor;`)

**Interfaces (Produces):**
- `timeline::Segment { start: f64, end: f64, deleted: bool }`.
- `timeline::Timeline { duration_ms: u64, segments: Vec<Segment>, trim_start: f64, trim_end: f64, selected: usize, hide_deleted: bool, zoom: f64, view_start: f64 }` with `new(duration_ms)`, `minimum() -> f64`, `max_zoom() -> f64`, `segment_index_at(pct) -> Option<usize>`, `can_split(pct) -> bool`, `split_at(pct) -> bool`, `can_toggle_selected() -> bool` (refuses deleting the last active segment), `toggle_selected() -> bool`, `set_trim_start(pct)`, `set_trim_end(pct)` (clamped by `minimum`), `kept_ranges() -> Vec<(f64, f64)>`, `kept_ranges_ms() -> Vec<(u64, u64)>`, `kept_fraction() -> f64`, `cut_count() -> usize` (deleted segments), `source_to_display(pct) -> f64`, `display_to_source(d) -> f64`, `set_zoom(z, anchor_display)`, `pan(direction: f64)`, `ensure_visible(pct, align_start)`, `span() -> f64`, `next_kept_start_ms(pos_ms) -> Option<u64>`, `kept_range_at_ms(pos_ms, slack_ms) -> Option<usize>`.
- `snapshot::Effects { exposure, contrast, saturation, temperature, sharpness, vignette: i32 }` with `clamped()` (ranges from notes section 5) and `is_neutral()`.
- `snapshot::Snapshot { trim_start, trim_end, segments: Vec<Segment>, selected: usize, crop: Option<(u32,u32,u32,u32)>, stretch: f64, effects: Effects, speed: f64, preserve_pitch: bool }` (`PartialEq`), `Snapshot::from_timeline(&Timeline, …)`, `apply_to(&self, &mut Timeline)`.
- `snapshot::UndoStack { undo: Vec<Snapshot>, redo: Vec<Snapshot> }`: `commit(before, current) -> bool` (no-op when equal; cap 100 drops oldest; clears redo), `undo(current) -> Option<Snapshot>`, `redo(current) -> Option<Snapshot>`, `can_undo()`, `can_redo()`.
- `snapshot::encode_state(&Snapshot) -> serde_json::Value` and `decode_state(&Value) -> Option<Snapshot>` (the draft `state` object of notes section 3, with every rejection and lenient rule there).
- `tools::SPEED_PRESETS: [(&str, f64); 7]` (labels verbatim incl. two spaces after `×`), `normalise_speed(f64) -> f64`, `tools::QuickCrop { x, y, w, h, src_w, src_h: u32 }` with `from_setting(&Value) -> Option<QuickCrop>`, `to_setting() -> Value`, `apply(src_w, src_h) -> Option<(u32,u32,u32,u32)>` (scaling rule, `None` unless `w > 4 && h > 4`), `tools::clamp_crop(crop, src_w, src_h) -> Option<crop>` (dropped under 2).
- `format::{clock(ms) -> String /* m:ss, minutes unbounded */, range_label(start_ms, end_ms) -> String /* "m:ss → m:ss" */, kept_label(kept_ms, cuts) -> String /* "0:07 kept  ·  1 cut" */, created(mtime) -> String /* "%b %d, %Y  %H:%M" */, location(dir) -> String /* "…" + last 34 when > 36 */, title(stem) -> String /* 36 chars + "…" when > 38 */}`.

- [ ] Step 1: Write the failing tests in `tests/editor_model.rs`:
  - `split_and_delete_follow_reference`: 10 000 ms; `split_at(0.3)` → two segments, `selected == 1`; `split_at(0.3005)` → false (inside `minimum` 0.025); `split_at(0.6)`; select 1, `toggle_selected` → `segments[1].deleted`; `kept_ranges() == [(0,0.3),(0.6,1)]`; `cut_count() == 1`; deleting the remaining two in turn: the second refusal leaves one active segment; restoring is always allowed.
  - `trim_clamps_and_kept_ranges_clip_to_trim`: `set_trim_start(0.95)` on a fresh 10 s timeline → `trim_start == 1 - 0.025`; `set_trim_end(0.0)` → `trim_end == trim_start + 0.025`; with segments `[0,.3) [.3,.6 deleted) [.6,1]` and trim 0.2..0.7 → kept `[(0.2,0.3),(0.6,0.7)]`; `kept_ranges_ms` equals `int(pct * duration)`.
  - `hide_deleted_maps_display_and_back`: with the middle 30 % deleted and `hide_deleted`, `source_to_display(0.6) ≈ 0.3/0.7`, `display_to_source(source_to_display(x)) ≈ x` for kept x within 1e-9.
  - `zoom_and_pan_limits`: 10 s → `max_zoom() == 8`; 3 600 000 ms → 720; `set_zoom(1.25, 0.5)` keeps display 0.5 at the same screen fraction; `pan(1.0)` moves `view_start` by `0.7 * span` and clamps to `1 - span`; `ensure_visible(0.95, false)` centers when outside.
  - `playhead_helpers`: kept `[0,3000) [6000,10000]` → `kept_range_at_ms(2990, 25) == None` (inside slack of the end), `kept_range_at_ms(1000, 25) == Some(0)`, `next_kept_start_ms(4000) == Some(6000)`, `next_kept_start_ms(9990) == None`.
  - `short_clip_limits`: 300 ms → `minimum() ≈ 0.8333`, `can_split(0.5) == false`, after `set_trim_start(0.9)` and `set_trim_end(0.1)` still `trim_start < trim_end`, `max_zoom() == 8`.
  - `undo_stack_caps_and_clears_redo`: 101 commits → `undo.len() == 100`, first dropped; equal before/after → `commit` false; `undo` then a new commit → `can_redo() == false`.
  - `draft_round_trip`: a snapshot with two segments, crop, stretch 1.5, effects, speed 0.5, no pitch → `decode_state(&encode_state(s)) == Some(s)`; the encoded keys are exactly `trim_start, trim_end, segments, selected_segment, crop_rect, stretch_ratio, effects, speed_rate, preserve_pitch`.
  - `draft_validation_matches_reference`: each rejection from notes section 3 returns `None` (NaN trim, `trim_start >= trim_end`, stretch 5, speed 3, 0 and 501 segments, a gap of 0.001, first start 0.1, all deleted); lenient: `selected_segment` 9 with 2 segments → 1; crop `[0,0,1,1]` → `None` crop; effect `exposure: 500` → 100; `preserve_pitch: "yes"` → true.
  - `draft_for_changed_file_is_ignored`: through `ClipMetadataStore::editor_draft_if_current` with an identity whose size differs → `None` (pins the existing store contract the editor relies on).
  - `quick_crop_scales`: saved `{x:100,y:50,w:800,h:600,src_w:1920,src_h:1080}` applied to 1280×720 → `(66,33,533,400)`; applied to 8×8 → `None`; `from_setting(null) == None`.
  - `formatting`: `clock(0) == "0:00"`, `clock(3_725_000) == "62:05"`; `kept_label(7000, 1) == "0:07 kept  ·  1 cut"`, `kept_label(7000, 2)` ends `"2 cuts"`, `kept_label(7000, 0)` ends `"0 cuts"`; `title` of a 40-char stem is 36 chars + "…"; `location` of a 50-char path is "…" + last 34.
- [ ] Step 2: Run `cargo test -p fthrust-core --test editor_model` → FAIL (no `editor` module).
- [ ] Step 3: Implement the five modules, porting the reference functions named in notes sections 1–5 (`_segment_index_at` including the hide-deleted rules, `_source_to_display` / `_display_to_source` with `bisect_right`, `set_zoom`, `pan`, `ensure_visible`, `_editor_snapshot_from_draft`).
- [ ] Step 4: Run → PASS; `cargo clippy -q -p fthrust-core --all-targets -- -D warnings` clean.
- [ ] Step 5: Commit `feat(core): editor timeline model, snapshots, undo and draft codec`.

---

### Task 3: Editor controller

**Files:**
- Create: `crates/fthrust-ui/src/editor/mod.rs`, `crates/fthrust-ui/tests/editor_controller.rs`
- Modify: `crates/fthrust-ui/src/lib.rs` (`pub mod editor;`)

**Interfaces:**
- Consumes: Task 2's model; `PlaybackService::{open, play, pause, seek_ms, set_rate, set_master_percent, set_source, sources, position_ms, duration_ms, playing, take_frame, tick, events, close}`; `ClipMetadataStore`.
- Produces:
  - `editor::Lifecycle { Preparing, Ready, PlayQueued, Playing, Paused, Failed, Stopped, Closing, Closed }`.
  - `editor::EditorRequest { Share, Export, Park, Close, Navigate(i32), DeleteClip }` (drained by the layer/sessions).
  - `editor::SessionState { snapshot: Snapshot, position_ms: u64, undo: UndoStack, master_volume: u8, source_gains: Vec<(String, u8, bool)>, zoom: f64, view_start: f64 }`.
  - `editor::Editor::open(path: &Path, paths: &FthrPaths, settings: &SettingsStore, player: PlayerFactory) -> Result<Editor, String>` where `PlayerFactory = Box<dyn Fn(&Path) -> Result<PlaybackService, String>>` (real: `PlaybackService::open` with PipeWire and `real_clock`; tests: `NullOutput` and a manual clock); loads the current draft through `editor_draft_if_current` and applies it without an undo step.
  - Fields read by the UI: `path`, `timeline: Timeline`, `crop`, `stretch`, `effects`, `speed`, `preserve_pitch`, `undo: UndoStack`, `lifecycle`, `status_text() -> &str`, `play_label() -> &str` ("PLAY" / "PAUSE" / "Preparing…"), `position_ms()`, `tag`, `description`, `details_dirty: bool`, `split_feedback_until: Option<f64>`, `requests: Vec<EditorRequest>`.
  - Methods: `tick(&mut self, now_s: f64, settings: &mut SettingsStore, store: &mut ClipMetadataStore)` (player tick and events, lifecycle, 1000 ms audio deadline, 15 s stall, the 80 ms kept-range enforcement, the 50 ms scrub flush, the 450 ms draft flush, the 900 ms "Ready" clear); `toggle_play(now_s)`; `scrub_to(pct, now_s)`; `begin_gesture()` / `end_gesture(now_s)` (compound undo for trim and effect drags); `commit(before: Snapshot, now_s)`; `undo(now_s)`, `redo(now_s)`; `split(now_s) -> bool` (sets the 1200 ms "Move playhead" feedback on failure); `set_selected_deleted(bool, now_s)`; `set_speed(rate, now_s)`, `set_preserve_pitch(bool, now_s)`; `set_master(percent, settings)` (writes `master_volume`); `set_source(id, gain, muted)`; `apply_quick_crop(now_s)`, `save_quick_crop(settings)`; `reset_stretch(now_s)`; `reset_look(now_s)`; `flush_details(store)`; `capture() -> SessionState` (flushes details, gesture, draft; pauses); `restore(state)` (paused, seeks once ready); `close(store)` (final flush, player closed).

- [ ] Step 1: Write the failing tests in `tests/editor_controller.rs` (4 s test clips, manual clock, a temp `FthrPaths` home):
  - `lifecycle_reaches_ready_and_play_queues`: right after `open`, `status_text() == "Preparing clip…"`; `toggle_play` before ready → `PlayQueued`, status "Preparing · play queued", label "Preparing…"; ticking until ready starts playback (`Playing`, label "PAUSE"); "Ready" shows and is cleared 900 ms later.
  - `scrub_is_debounced`: three `scrub_to` calls 10 ms apart → no seek reaches the player until 50 ms after the last; then `position_ms` ≈ the last target.
  - `playback_skips_deleted_and_stops_at_edit_end`: segments `[0,1s) [1s,2s deleted) [2s,4s]`; play from 0.9 s; after ticks past 1.0 s the position jumps to ≈ 2000; at the end of the last kept range the editor pauses and rewinds to 0.
  - `play_outside_kept_range_jumps_forward`: position 1.5 s (deleted) → `toggle_play` seeks to 2000 first.
  - `trim_drag_is_one_undo_step`: `begin_gesture`, three `set_trim_start` changes, `end_gesture` → `undo.undo.len() == 1`; `undo` restores the old trim; `redo` re-applies it.
  - `draft_saved_after_debounce_and_reloaded`: a split then 449 ms of ticks → no draft in the store; at 450 ms → draft present with `version 1` and the file identity; a new `Editor::open` on the same path shows the split and has an empty undo stack.
  - `split_failure_feedback`: playhead within the minimum of an edge → `split` false and `split_feedback_until == Some(now + 1.2)`.
  - `capture_restore_round_trip`: after edits, `capture()`, close, `open` + `restore(state)` → same snapshot, position, undo depth, zoom; lifecycle `Paused` (never autoplays).
  - `video_only_clip_plays`: a clip without audio → reaches `Ready`, plays, position advances with the manual clock, `player.sources()` empty.
  - `master_and_quick_crop_write_settings`: `set_master(55)` → `settings.get_i64("master_volume") == Some(55)`; with a crop set, `save_quick_crop` writes the six keys; `apply_quick_crop` is one undo step.
- [ ] Step 2: Run `cargo test -p fthrust-ui --test editor_controller` → FAIL.
- [ ] Step 3: Implement `editor/mod.rs`. The status strings are notes section 8's; the speed apply calls `player.set_rate(speed, preserve_pitch)`.
- [ ] Step 4: Run → PASS; clippy clean.
- [ ] Step 5: Commit `feat(ui): editor controller with drafts, undo and kept-range playback`.

---

### Task 4: Editor layer, header, video surface and control bar

**Files:**
- Create: `crates/fthrust-ui/src/editor/layer.rs`, `crates/fthrust-ui/tests/editor_ui.rs`
- Modify: `crates/fthrust-ui/src/state.rs` (`pub editor: Option<crate::editor::Editor>`, `pub editor_video: Option<VideoTexture>`), `crates/fthrust-ui/src/shell/mod.rs` (when `state.editor` is active the layer draws over the whole window instead of the shell), `crates/fthrust-ui/src/lib.rs` (`App::tick` ticks the editor), `crates/fthrust-ui/tests/fidelity_all.rs` (replace the ignored `clip_editor` with a real test)

**Interfaces:**
- Consumes: Task 3's `Editor`.
- Produces: `editor::layer::ui(ui: &mut Ui, state: &mut AppState)` drawing the header (`editorHeader`, `editorTitle`, `editorMin` "Pause editor in sidebar", `editorMax` "Maximize or restore", `editorClose`), the video surface (letterboxed frame from `editor_video`, background token), the control bar (`ctrlBar`, `playBtn`, `timeLabel[…]` ×2) and the reserved trim bar and sidebar rects that Tasks 5 and 6 fill (`trimBar`, `editorSidebar`). `editorMin` pushes `EditorRequest::Park`, `editorClose` `Close`, `editorMax` sends the maximize/restore viewport command.
- A test constructor `Editor::for_tests(path, duration_ms, lifecycle)` with no player, used by the fidelity tests.

- [ ] Step 1: Write the failing tests:
  - `fidelity_all.rs::clip_editor`: `Editor::for_tests("Overwatch_clip.mp4", 10_000, PlayQueued)` in `state.editor`, render at 1440×900 → `assert_fidelity!(…, "clip-editor", …)` (the comparison covers the markers drawn so far: header, control bar, `trimBar` and `editorSidebar` frames).
  - `editor_ui.rs::header_buttons_emit_requests`: kittest click on `editorMin` → `requests == [Park]`; `editorClose` → `[Close]`; `playBtn` → lifecycle toggles.
- [ ] Step 2: Run both → FAIL.
- [ ] Step 3: Implement the layer and the state/shell/App plumbing (frames from `take_frame` go to `VideoTexture::update` in `App::tick`).
- [ ] Step 4: Run → PASS; the workspace UI suite stays green.
- [ ] Step 5: Commit `feat(ui): editor layer with header, video surface and control bar`.

---

### Task 5: Trim timeline widget and filmstrip

**Files:**
- Create: `crates/fthrust-ui/src/editor/timeline_ui.rs`, `crates/fthrust-ui/src/editor/filmstrip.rs`
- Modify: `crates/fthrust-ui/tests/editor_ui.rs`, `crates/fthrust-ui/tests/fidelity_all.rs`

**Interfaces:**
- Consumes: `Timeline`, `Editor::{scrub_to, begin_gesture, end_gesture, split, set_selected_deleted, undo, redo}`.
- Produces: `timeline_ui::ui(ui, rect: Rect, state: &mut AppState, now_s: f64)` drawing `trimLabel` "Timeline", `trimRangeLbl` (range label while trimming, else kept label), `timelineHistory[Undo|Redo]` with the tooltips of notes section 10, `timelineTool` ×4 (pan left, zoom out, zoom in, pan right) and `timelineZoomLabel`, `timelineAction` "Split" / "Move playhead", `timelineActionDanger` "Delete segment" / "Restore segment", then the 116 px track: ruler (5 ticks, `m:ss`, scaled by retained fraction when hiding), filmstrip, segments with deleted shading, trim handles, playhead, minimap with viewport box. Mouse: left press within 9 px of a handle drags it (one undo gesture), else playhead scrub; wheel zooms at the cursor, Shift+wheel pans; a press with `y >= 98` is a minimap press; right-click opens the menu "Split here" / "Delete segment" or "Restore segment" / separator / "Hide deleted segments" (checkable, writes `hide_deleted_timeline_segments`).
- `filmstrip::Filmstrip::request(path, duration_ms, count)` (worker decodes `count = min(120, max(24, 24 + duration_s / 60))` frames at 180×100 through `VideoDecoder`, cancellable), `take_ready() -> Option<Vec<ColorImage>>`, an 8-entry / 20 MB LRU by path; skipped when `timeline_preview_strip_enabled` is false.

- [ ] Step 1: Write the failing tests in `editor_ui.rs` (kittest, `Editor::for_tests`, 1440×900):
  - `click_scrubs_and_selects`: click at 30 % of the track → after 50 ms of ticks `position_ms ≈ 3000` and `selected` is the segment under it.
  - `handle_drag_trims_one_undo_step`: drag the start handle from x of 0.0 to x of 0.2 → `trim_start ≈ 0.2`, `undo.undo.len() == 1`, the range label reads "0:02 → 0:10" during the drag.
  - `wheel_zooms_and_shift_pans`: wheel up over the track → zoom label "125%"; Shift+wheel → `view_start` moved by 0.7 span.
  - `context_menu_targets_click_position`: right-click at 50 % → menu shows "Split here" enabled; choosing it splits at 0.5 even though the selection was segment 0.
  - `minimap_press_centers_view`: zoom 400 %, press the minimap at 80 % → `view_start ≈ 0.8 - span/2`.
  - `filmstrip_loads_after_idle_and_cancels_on_play`: with a 4 s test clip, no request before 300 ms idle; a request after; `toggle_play` cancels it.
  - `fidelity_all.rs::clip_editor_timeline`: the Task 1 state reproduced → `assert_fidelity!(…, "clip-editor-timeline", …)`.
- [ ] Step 2: Run → FAIL.
- [ ] Step 3: Implement the widget (geometry constants from Global Constraints) and the filmstrip worker.
- [ ] Step 4: Run → PASS.
- [ ] Step 5: Commit `feat(ui): editor trim timeline with segments, zoom, minimap and filmstrip`.

---

### Task 6: Sidebar panels

**Files:**
- Create: `crates/fthrust-ui/src/editor/sidebar.rs`
- Modify: `crates/fthrust-ui/tests/editor_ui.rs`, `crates/fthrust-ui/tests/fidelity_all.rs`

**Interfaces:**
- Consumes: `Editor` methods from Task 3, `widgets::{accordion, combo, slider::editor_slider, check, input::line_edit, buttons}`.
- Produces: `sidebar::ui(ui, rect, state, now_s)` drawing in a vertical scroll: `shareBtn` (pushes `Share`), the quality combo `WheelSafeComboBox[…]` ("Very fast" / "High quality" / "Lossless · MKV" ↔ `fast` / `quality` / `lossless` in `export_quality`), `exportBtn` (pushes `Export`), "Export jobs"; then the six panels in order with their default open state (notes section 10): CLIP SPEED (`speedCombo` from `SPEED_PRESETS`, `pitchToggle` with the two hints), EDIT TOOLS (`cropBtn` "Set crop"/"Edit crop", `quickCropBtn` with its two tooltips and disabled state, `saveQcBtn` with the 1800 ms "saved", `stretchBtn`, `resetStretchBtn`, `cropInfoLbl` "w × h"), AUDIO MIX (MASTER row bound to `master_volume`, one row per `player.sources()` entry with gain %, slider 0..100, MUTE/MUTED, tooltip `{label} gain for this open clip only.`), COLOR & EFFECTS (six sliders with `+d` labels and "Reset look"; each drag one undo gesture), CLIP DETAILS (Title read-only via `format::title`, Tag and Description line edits with their placeholders, flushed on Enter or focus loss), FILE DETAILS (Created, Video Quality, Video Bitrate, Total Bitrate, Duration, Size, Location with "Loading…" / "Unavailable" from a background `probe_video_metadata`), and `deleteBtn` "Delete" or, for linked imports, "Linked original — protected" (disabled, with its tooltip). `cropBtn` and `stretchBtn` open the Task 9 dialogs (until then they push nothing).

- [ ] Step 1: Write the failing tests:
  - `fidelity_all.rs::clip_editor_covers_every_widget`: every widget name in `clip-editor.json` except `qt_scrollarea_*` has a drawn marker, and `assert_fidelity!` passes.
  - `fidelity_all.rs::clip_editor_sidebar`: all panels expanded, MASTER + "Desktop" + "Mic" rows → `assert_fidelity!(…, "clip-editor-sidebar", …)`.
  - `editor_ui.rs::speed_change_is_one_undo_step_and_applies_rate`: choose "0.5×  SLOW" → `editor.speed == 0.5`, one undo entry.
  - `quality_combo_persists`: choose "Lossless · MKV" → `export_quality == "lossless"`.
  - `details_flush_on_focus_loss`: type a tag, move focus → the store's entry has the tag and an empty description; typing nothing changes nothing.
  - `linked_import_delete_is_protected`: an editor for a path under an imported root → the button reads "Linked original — protected" and is disabled.
- [ ] Step 2: Run → FAIL.
- [ ] Step 3: Implement `sidebar.rs`.
- [ ] Step 4: Run → PASS.
- [ ] Step 5: Commit `feat(ui): editor sidebar panels`.

---

### Task 7: Sessions, clip navigation and delete

**Files:**
- Create: `crates/fthrust-ui/src/editor/sessions.rs`, `crates/fthrust-ui/tests/editor_sessions.rs`, `tools/uidrive/scenarios/editor.sh`
- Modify: `crates/fthrust-ui/src/services/mod.rs` (handle `Request::OpenClip`), `crates/fthrust-ui/src/state.rs` (`editor_sessions` filled from `Sessions`; `editor_requested` / `editor_close_requested` consumed), `crates/fthrust-ui/src/shell/nav_rail.rs` (tooltip `{filename}\n` + "Active editor" | "Paused — click to restore" + `\nRight-click to close`), `crates/fthrust-ui/tests/fidelity_all.rs`

**Interfaces:**
- Consumes: `Editor::{open, capture, restore, close, requests}`, `Library::adjacent`, `ClipReadinessRegistry`, `classify_media_path`, `delete_owned_media`, `ClipMetadataStore::clear_editor_draft`.
- Produces: `sessions::Sessions { active: Option<usize>, entries: Vec<Session> }`, `Session { path: PathBuf, state: Option<SessionState> }`; `open(&mut self, path, state: &mut AppState) -> bool` (`false` when refused; refusals queue their message on `state.dialogs`) (existing session for the path restores it; else parks the active one; a fourth session → message "Three editors are open" / "Close a sidebar editor before opening another clip."; a missing file → "CLIP NOT FOUND" / `{name} was moved or deleted outside the app.` and a library rescan); `park(state)`; `restore(n, state)` (missing file → "CLIP NOT FOUND" / "The editor clip is no longer available." and removal); `close(n, state)`; `navigate(step, state)` (Tab+Right/Left: `Library::adjacent`, no wrap, reuses the editor, merges with an existing session for the destination); `sidebar_step(step, state)` (Shift+Down/Up over `[Clips, Settings] + sessions`, wrapping); `delete_active(state)` (the order of checks in notes section 9 steps 2–5; step 1 export check waits for 6c); `summaries() -> Vec<SessionSummary>`. When `clip_editor_enabled` is false, `OpenClip` is ignored with one log line: the reference's `BasicClipViewer` is not ported (ruling recorded in `docs/editor.md` and `CHANGES.md`).

- [ ] Step 1: Write the failing tests in `editor_sessions.rs` (real test clips, manual-clock factory):
  - `fourth_session_is_refused`: open three clips (each parks the previous) → a fourth `open` queues the "Three editors are open" message and leaves three sessions.
  - `reopening_a_parked_clip_restores_it`: edit clip A, open B, open A again → A's snapshot and undo depth are back, two sessions remain.
  - `restore_of_deleted_clip_removes_session`: park A, delete its file, `restore(1)` → the message and A gone from `summaries()`.
  - `navigation_moves_within_library_without_wrap`: three clips in the library; at the last one `navigate(1)` does nothing; `navigate(-1)` loads the middle clip in the same editor with its own draft.
  - `rapid_clip_navigation_keeps_one_player`: five `navigate(1)` calls without ticking between them → afterwards exactly one `PlaybackService` alive (count via the factory), the editor on the last clip, every left clip's draft flushed.
  - `delete_owned_clip_confirms_and_clears_draft`: `delete_active` → a Question "Delete Clip" with `Delete {basename}?\nThis cannot be undone.`; answering yes removes the file, the draft and the session; for a clip under an import root → the "Linked Original Protected" message and nothing deleted.
  - `fidelity_all.rs::clip_editor_rail` → `assert_fidelity!(…, "clip-editor-rail", …)`.
- [ ] Step 2: Run → FAIL.
- [ ] Step 3: Implement `sessions.rs` and the wiring; write `tools/uidrive/scenarios/editor.sh`: open the first library clip (double-click its card), wait for `ctrlBar`, press Space, wait until `timeLabel` changes, split with Ctrl+C, park with Shift+Escape, wait for `editorSidebarButton[1]`, restore it, close with Escape; shoot each step.
- [ ] Step 4: Run the tests → PASS; run `tools/uidrive/bench.sh run tools/uidrive/scenarios/editor.sh` → PASS and look at the shots.
- [ ] Step 5: Commit `feat(ui): editor sessions, clip navigation and delete`.

---

### Task 8: Editor keyboard shortcuts and fullscreen

**Files:**
- Create: `crates/fthrust-ui/src/editor/keys.rs`
- Modify: `crates/fthrust-ui/tests/editor_sessions.rs`

**Interfaces:**
- Consumes: `NavigationBindings::from_settings`, `is_category_enabled("Clip editor")`, Task 7's `Sessions`.
- Produces: `keys::handle(ctx, state, now_s)` run once per frame while the editor is active, mapping the bound combos of `editor_split`, `editor_delete`, `editor_restore`, `editor_crop` (quick crop), `editor_export`, `editor_minimize`, `editor_close`, `editor_next`, `editor_previous`, `editor_play`, `editor_undo`, `editor_redo`, `editor_redo_alt`, plus `sidebar_next` / `sidebar_previous` and fixed F11 (only while the category is enabled). Escape exits fullscreen first, else closes. Shortcuts do nothing while a text field has focus or a dialog is open. `Tab+Right`-style chords use the existing chord matcher from the hotkey/navigation code.

- [ ] Step 1: Write the failing tests (kittest key events):
  - `default_bindings_drive_the_editor`: Space toggles play; Ctrl+C splits; Ctrl+D then Ctrl+W delete and restore; Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z undo and redo; Shift+Escape parks; Escape closes.
  - `escape_leaves_fullscreen_first`: F11 then Escape → still open, not fullscreen; Escape again → closed.
  - `shortcuts_ignored_while_typing`: focus the Tag field, press Ctrl+D → nothing deleted.
  - `disabled_category_disables_keys`: `set_category_enabled("Clip editor", false)` → Space does nothing, F11 does nothing.
  - `rebinding_is_respected`: bind `editor_split` to Ctrl+K → Ctrl+K splits and Ctrl+C does not.
- [ ] Step 2: Run → FAIL.
- [ ] Step 3: Implement `keys.rs`.
- [ ] Step 4: Run → PASS.
- [ ] Step 5: Commit `feat(ui): editor keyboard shortcuts and fullscreen`.

---

### Task 9: Crop and stretch dialogs; docs

**Files:**
- Create: `crates/fthrust-ui/src/editor/dialogs.rs`, `docs/editor.md`
- Modify: `crates/fthrust-ui/tests/editor_ui.rs`, `crates/fthrust-ui/tests/fidelity_all.rs`, `CHANGES.md` (the two copy rulings and the unported `BasicClipViewer`), `docs/superpowers/plans/2026-10-22-fthrust-roadmap.md` (6b done; 6c gains the live crop/stretch/colour preview)

**Interfaces:**
- Consumes: the poster frame (`VideoDecoder::frame_at` at the playhead, decoded on a worker before the dialog opens), `Editor::commit`.
- Produces: `dialogs::CropDialog` and `dialogs::StretchDialog` (modal over the editor layer, playback paused while open and resumed after if it was playing). Crop: a draggable, resizable rectangle in source pixels over the frame; accept stores the crop as one undo step and refreshes `cropBtn` / `cropInfoLbl`. Stretch: the crop-aware preview with a draggable side handle, ratio clamped 0.25–4.0, accept is one undo step. Behaviour and copy follow cv:3084 and cv:2905.

- [ ] Step 1: Write the failing tests:
  - `fidelity_all.rs::clip_editor_crop` and `clip_editor_stretch` against the Task 1 renders.
  - `editor_ui.rs::crop_accept_is_one_undo_step_and_pauses`: playing editor, open crop → paused; drag the rect, accept → crop set, one undo entry, playing again; cancel leaves no undo entry.
  - `stretch_ratio_clamps`: drag the handle far right → ratio 4.0; accept → `stretch == 4.0`.
- [ ] Step 2: Run → FAIL.
- [ ] Step 3: Implement the dialogs; write `docs/editor.md` (layer model, sessions, drafts, keys, what 6c adds); update `CHANGES.md` and the roadmap.
- [ ] Step 4: Run the full gate: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check`, and every bench scenario (`smoke`, `save_clip`, `record`, `selftest`, `editor`) → all pass.
- [ ] Step 5: Commit `feat(ui): crop and stretch dialogs; docs: editor`.
