# Clip editor: Python reference notes

Paths are relative to `reference/FTHR_UI/`. `cv` = `ui/clip_viewer.py`. Line numbers are approximate to the line quoted.

## 1. Timeline model (`TrimSlider`, cv:1045)

**Representation.** `TimelineSegment(start_pct, end_pct, deleted=False)` (`ui/timeline_segments.py:8`). All positions are fractions 0..1 of source duration, not milliseconds. A fresh timeline is `[Segment(0.0, 1.0)]` (cv:1066). Segments are contiguous, ordered, and cover the whole clip. Deleting never removes a segment, it only flips `deleted`. The Python `TimelineSegments` list subclass only gives a `version` counter for cache invalidation; the Rust port does not need it.

**Trim handles are separate from segments.** `start_pct` (default 0.0) and `end_pct` (default 1.0) are independent fields. Kept ranges (`kept_ranges`, cv:1432) = every non-deleted segment clipped to `[start_pct, end_pct]`, dropping any piece of 0.0001 or less. These are the ranges that play and export. Milliseconds are `int(pct * duration_ms)` (`_kept_segments_ms`, cv:7806). Seconds are ms / 1000.

**Minimum gap constant.** `minimum = max(0.001, 250 / duration_ms)`. It applies to splitting (cv:1423 area) and to trim-handle drags (cv:1583). Start is clamped to `[0, end - minimum]`. End is clamped to `[start + minimum, 1]`.

**Split** (`split_at_playhead`, cv:1411). It uses the playhead segment, found by `_segment_index_at`. It fails (returns False) if there is no segment, the segment is deleted, or the playhead is within `minimum` of either segment edge. On success the segment is replaced by two, `[start, pct]` and `[pct, end]`. `selected_segment` becomes the right-hand half (`index + 1`). `segments_changed` and `selection_changed(sel, False)` are emitted. On failure the Split button text becomes "Move playhead" and reverts to "Split" after 1200 ms (cv:7753).

**Delete / restore** (`toggle_selected_deleted`, cv:1433). Deleting is refused if there is only 1 segment or only 1 non-deleted segment remains. Restoring is always allowed. After a toggle the viewer (cv:7763) checks the player position. If it is no longer inside a kept range, it seeks to the next kept start, else the first kept start.

**Selection.** `selected_segment` is an index. Clicking the track sets it to the segment under the click (`_segment_index_at`). Clicking the minimap sets it to `max(0, index at pct)`. `selection_changed(index, deleted)` drives the button label (see section 9). `_segment_index_at`: first segment with `start <= pct <= end`. With hide-deleted on it skips deleted segments and segments where `pct == end`, with a fallback to the last non-deleted segment whose end is within 1e-9.

**Hide deleted segments** (display only). A setting `hide_deleted_timeline_segments` (default False) is stored through `sm.set` and `sm.save_settings` (cv:7793). When on, `_source_to_display` and `_display_to_source` (cv:1131-1153) compress out deleted spans:
- Over the non-deleted ranges, `offset[i]` is the cumulative kept length, and display = `offset / total_kept`.
- Source-to-display uses a `bisect_right` over range ends.
- Display-to-source uses a `bisect_right` over cumulative ends with +1e-9.
- Playback, seeking and the model always use source time. Only drawing and mouse mapping use display time.
- Tick labels show `pct * duration_ms * retained` where `retained` is the total kept fraction when hidden, else 1.0 (cv:1436).
- Toggling resets `view_start_pct = 0` and `ensure_visible(playhead)`.

**Playhead skipping deleted segments during playback.** This is not done in the timeline. `ClipViewer._update_playhead` (cv:7245) runs on `_ph_timer`, interval **80 ms**, while playing. It compares the player position `pos` with the kept ranges. The current range is the one where `start - 25 <= pos < end - 25` (25 ms slack). If there is none:
- If a later kept range starts after `pos`, it calls `setPosition(next.start)` and `mixer.seek`.
- Otherwise (past the last range) it calls `player.pause()` and `setPosition(ranges[0].start)`. This is end-of-edit behaviour: pause and rewind to the first kept frame. It does not loop.

`_toggle_play` (cv:7116-7128) does the same on Play. If `position` is not in a kept range `[start, end - 60)`, it seeks to the next range start, or `playable[0]` start.

**Zoom.** `zoom_factor` starts at 1.0. `max_zoom_factor = max(8.0, min(2048.0, duration_ms / 5000.0))` (cv:1074), so about a 5 s window for long files and 8x for short ones. The limit is clamped in `set_zoom` (cv:1160). `view_span_pct = 1 / zoom`. The anchor stays fixed in display space; `view_start_pct` is clamped to `[0, 1 - span]`.
- Buttons call `_zoom_timeline(1.25)` for zoom in and `0.8` for zoom out (cv:5041/5029). The label is `f"{int(round(zoom*100))}%"`, initial "100%".
- Mouse wheel without Shift zooms by x1.25 (wheel up) or x0.8 (wheel down), anchored at the cursor (cv:1627).
- **Shift+wheel** pans by `+-0.7 * span` (`pan`, cv:1170). The pan buttons use the same function, direction -1 or +1.
- `ensure_visible(pct, align_start)` recenters (offset = 0.5 * span, or 0.08 * span when `align_start`) only when the point is outside the view.

**Minimap and viewport.** A strip at `y = 103, height = 7` (overview_y, overview_h) shows the full clip, deleted ranges, and a viewport box of width `max(3, span*tw)`. Any press with `y >= 98` is a minimap press (cv:1537). It centers the view on the click (`view_start = display(pct) - span/2`), sets the playhead and selection, and emits `seek_requested`. The right-click menu also maps y >= 98 through the minimap.

**Geometry constants.** Widget fixed height 116. Horizontal padding 14 (track width = `w - 28`). Track `ty=22, th=70`. Ruler has 5 ticks, labelled with `m:ss`, font 7. Filmstrip slots = `max(4, min(14, tw // 92))` (cv:1470).

**Mouse interaction** (`mousePressEvent`, cv:1530; left button only):
- A press within **9 px** (measured in display space) of the trim start or end handle begins a trim drag. Start has priority. It emits `range_edit_started` and, on release, `range_edit_finished`.
- Otherwise it is a playhead drag. The first click seeks and selects the segment, and moves keep scrubbing (`seek_requested` on each move). While `dragging == 'playhead'` the viewer does not overwrite the playhead from the player.
- Dragging a handle emits `range_changed(start, end)`.
- Right-click context menu (cv:1640): targets the click pct (not the stale selection), sets the playhead and selection, and seeks first. Entries: "Split here" (disabled unless splittable), "Restore segment" / "Delete segment" (disabled when it would remove the last active segment), a separator, "Hide deleted segments" (a checkable item).

**Scrub debounce.** `_on_seek_requested` (cv:7281) stores `_pending_seek_ms = clamp(int(pct*duration_ms), 0, duration_ms)`. It restarts a single-shot **50 ms** `_seek_timer` (a true debounce). It sets the playhead immediately, and `_flush_pending_seek` (cv:7328) applies `player.setPosition`. If the media is not ready yet, the seek stays pending and is applied on the media-ready status. The paused-seek fallback timer is **300 ms** (it decodes a still frame if the native output missed the seek; backend detail).

**Thumbnails.** `count = min(120, max(24, 24 + int(duration_s/60)))` (cv:6264). They are 180x100 RGB frames via ffmpeg `fps=count/duration`. They load 300 ms after the player is idle (**1500 ms** after pause, 300 ms after stop) and are cancelled on play. LRU cache is 8 entries / 20 MB, and the strip can be disabled by setting `timeline_preview_strip_enabled` (default True).

## 2. `EditorSnapshot` (cv:372) and undo/redo

Frozen dataclass fields (all):
- `trim_start: float`
- `trim_end: float`
- `segments: tuple[(start, end, deleted)]`
- `selected_segment: int`
- `crop_rect: (x, y, w, h) | None` (ints, source pixels)
- `stretch_ratio: float`
- `effects: tuple[(key, int)]` (sorted by key)
- `speed_rate: float = 1.0`
- `preserve_pitch: bool = True`

Playback position, zoom and view are excluded. Equality is dataclass equality.

`_capture_editor_state` (cv:6819) reads the live widgets/fields. `_commit_editor_change(before)` (cv:6836):
- No-op if `_restoring_state` or `before == current`.
- Pushes `before` onto `_undo_stack`; if it is longer than **100**, drops the oldest.
- Clears `_redo_stack` and updates the button enabled states.
- Schedules the draft save.

`_undo` / `_redo` (cv:6900/6908) push the current state to the other stack and call `_restore_editor_state(state)`. The restore sets all fields, resets effect sliders, speed combo and pitch toggle (signals blocked), re-applies the playback rate, and refreshes the UI. Then, if the player position is outside every kept range, it seeks to the first kept start. It persists the draft unless `persist=False`.

**Compound gestures are one undo step.** Trim drag: `_begin_trim_edit` (on press) captures a snapshot, `_finish_trim_edit` (on release) commits (cv:6922). Effect slider: `_begin_effect_adjustment` on press and `_finish_effect_adjustment` on release (cv:6932). That release also forces an immediate preview refresh. A slider change with no press (keyboard/click) takes its own snapshot (cv:7834). Reset look, Reset stretch, Split, Delete/Restore, crop accept, quick crop and speed/pitch changes each commit one step.

Undo and Redo are buttons labelled "Undo" and "Redo", disabled when the stack is empty.

## 3. Draft persistence

**What is saved.** One draft per clip path, written by `ClipMetadataManager.set_editor_draft` (`core/clip_metadata_manager.py:117`). `save_state.py` is unrelated: it is the capture-save state machine. Storage is the sqlite file `~/.fthr/clip_metadata.sqlite3`, table `entries(path TEXT PRIMARY KEY, value TEXT NOT NULL)`. `value` is JSON (compact separators) of the whole per-clip entry, with keys `tag`, `description` and `editor_draft`. The legacy `~/.fthr/clip_metadata.json` is read only if the sqlite file is absent. Writes use `PRAGMA synchronous=FULL` and `BEGIN IMMEDIATE`, upserting only changed paths.

`editor_draft` JSON:
```
{"version":1,
 "file":{"mtime_ns":int,"size":int},
 "updated_at":time.time(),
 "state":{trim_start, trim_end, segments:[[s,e,deleted]...],
          selected_segment, crop_rect:[x,y,w,h]|null, stretch_ratio,
          effects:{exposure,contrast,saturation,temperature,sharpness,vignette},
          speed_rate, preserve_pitch}}
```
(the `state` keys are from `_editor_snapshot_to_draft`, cv:402).

**Validity on load.**
- `get_editor_draft` returns None unless `version == 1` and `file` equals the current `{mtime_ns, size}` of the clip (stale if the file changed).
- `_editor_snapshot_from_draft` (cv:420) rejects (returns None, so defaults are used) when:
  - any float is non-finite;
  - not `0 <= trim_start < trim_end <= 1`;
  - `stretch_ratio` is outside 0.25..4;
  - `speed_rate` is outside 0.25..2;
  - segments are not a list of 1..500 entries;
  - a segment is not `[start, end, deleted]`, or is non-contiguous (tolerance 0.0001), or does not start at 0 and end at 1, or has `end <= start`;
  - every segment is deleted.

Lenient handling: `selected_segment` is clamped. `crop_rect` is kept only if x and y are 0..100000 and w and h are 2..100000, else it is dropped. Effects are clamped per key. `preserve_pitch` defaults to True unless it is a bool.

**Cap.** `_EDITOR_DRAFT_LIMIT = 250` drafts. The oldest by `updated_at` have `editor_draft` removed (the entry is deleted entirely if it is then empty), in the same transaction.

**When saved.** `_schedule_editor_draft_save` sets `_draft_dirty` and restarts a single-shot **450 ms** timer (cv:4560). It is skipped while restoring, closing, or `_discard_editor_draft`. `_flush_editor_draft` skips the write if the snapshot equals `_last_persisted_snapshot`. Triggers: every committed change, trim change (`_on_trim_changed`), effect change, restore with persist. Final flushes: `_close`, `closeEvent`, `capture_session_state` (parking).

**When loaded.** `_restore_persisted_editor_draft` runs in clip init (after the UI exists), before the session restore. It calls `_restore_editor_state(persist=False)` and seeds `_last_persisted_snapshot`. The undo stacks are NOT persisted in the draft.

**When cleared.** Only `clear_editor_draft(path)` in `_delete_clip` after a successful file removal (cv:8517; it sets `_discard_editor_draft`). **Export does not clear the draft** and there is no other clear. Renaming a clip moves the whole entry (`rename_path`). A `edited_duration(path, source_duration)` helper (used by the library) returns `duration * kept fraction` computed from the draft, falling back to the source duration on any invalid data.

## 4. Speed

`SPEED_PRESETS` (cv:819), (label, rate):
`('0.25×  SLOW',0.25) ('0.5×  SLOW',0.5) ('0.75×  SLOW',0.75) ('1×  NORMAL',1.0) ('1.25×  FAST',1.25) ('1.5×  FAST',1.5) ('2×  FAST',2.0)`. Two spaces after the `×`. The default index is 3 (1x). The panel is "CLIP SPEED" (expanded by default), with the row label "Playback speed". The combo is 30 px high.

`_normalise_speed` clamps to 0.25..2.0 (non-finite or non-positive becomes the fallback 1.0). The combo stores the rate as item data and `_on_speed_changed` commits an undo step (cv:6980).

**Pitch.** The "Preserve pitch" checkbox defaults to True.
- Preserve: audio filter is the `atempo` chain (`_atempo_chain`, cv:849): stages of `atempo=2` while remaining > 2, `atempo=0.5` while < 0.5, then `atempo=<_speed_text>` for the rest. `_speed_text` is `f"{v:.6f}"` with trailing zeros and the dot stripped.
- Not preserved: `aresample=48000,asetrate=<round(48000*speed)>,aresample=48000` (`_pitch_changed_chain`, cv:869). Pitch follows speed.
- Video: `setpts=PTS/<speed>` appended last in `_video_filter_chain` (cv:972).
- Hint under the toggle: preserve -> "Audio stays at its original pitch while speed changes."; off -> "Audio pitch follows the selected playback speed."

**Preview.** `_apply_playback_rate` (cv:6947) sets `player.setPlaybackRate(speed)`, pitch compensation, and the audio mixer rate.

**Time label and timeline are NOT speed-adjusted.** The label is `f"{m:ss} / {m:ss}"` from the player position over the source `duration_ms`. The timeline is source time. Speed only changes the exported/played duration. The export queue is given `_speed_adjusted_duration(kept_total_s, speed) = max(0.1, d / speed)` as the expected duration (for progress). The sidebar "kept" label is also source time (see 6).

## 5. Crop, quick crop, stretch, Color & effects

**Crop.** `_crop_rect = (x, y, w, h)` ints in source pixels, or None. The Set crop dialog (CropDialog, cv:3084) is modal and pauses playback while open, resuming afterwards. On accept the rect is stored and an undo step is committed. The button text is "Edit crop" (property active=true) when a crop is set, else "Set crop". `crop_info_lbl` shows `f"{w} × {h}"` or empty. Export uses `crop={w & ~1}:{h & ~1}:{x}:{y}`. After the metadata probe, the crop is clamped into the real source size, and dropped if the width or height is under 2. A validation of w and h >= 2 happens in the draft loader.

**Quick crop.** Settings key `quick_crop` = `{"x","y","w","h","src_w","src_h"}` (all ints), written by `_save_quick_crop` (cv:7957) and `sm.save_settings()`. The "Save as quick crop" button is enabled only when a crop is set. After saving it shows "saved" for 1800 ms. Apply (`_apply_quick_crop`, cv:7939): scale `sx = src_w_now / saved.src_w` and `sy` likewise, `x=int(x*sx)`, `y=int(y*sy)`, `w=min(int(w*sx), src_w-x)`, `h=min(int(h*sy), src_h-y)`. It applies only if `w > 4 and h > 4`, as one undo step. The "Quick" button is disabled with tooltip "No quick crop saved yet" when absent. Its tooltip when present: `Saved: {w}×{h} ({src_w}×{src_h} source)`. The shortcut Ctrl+Q clicks it.

**Stretch.** `_stretch_ratio` is a horizontal width multiplier, clamped to 0.25..4.0 (default 1.0). The StretchDialog (cv:2905) is modal and pauses playback, shows a crop-aware preview and a draggable side handle, and commits one undo step on accept. "Reset" (62x32) sets 1.0 as an undo step. Export: if `abs(ratio-1) > 0.005`, `scale=trunc(iw*ratio/2)*2:trunc(ih/2)*2,setsar=1`, else even-size scaling if needed.

**Color & effects** (panel "COLOR & EFFECTS", collapsed by default). `_effects` has 6 int keys, default 0 (cv:4518): `exposure -100..100`, `contrast -100..100`, `saturation -100..100`, `temperature -100..100`, `sharpness 0..100`, `vignette 0..100`. Each is a horizontal slider; the value label shows `f"{v:+d}"` or `"0"`. Slider tooltip: `Adjust {title.lower()}`. "Reset look" (cv:7847) sets all sliders to 0 as one undo step. Reset stretch, crop and speed are separate. Live preview debounce is **70 ms** (`_preview_update_timer`); release forces it immediately. Preview is disabled when setting `clip_editor_live_preview` is False (default True; this sets neutral edit state) or when `editor_color_preview_enabled` is False (default True; only the colour effects are skipped).

FFmpeg filter chain for export (`_video_filter_chain`, cv:925), order: crop, curves (exposure/contrast), hue, colorbalance, unsharp, vignette, stretch scale + setsar, speed `setpts`.
- `curves=all=` with 3 points at x = 0, 0.5, 1, where `brightness = exposure*0.003`, `contrast_gain = 1 + contrast*0.006`, and y = clamp((x-0.5)*gain+0.5+brightness) (`:.3f` formatting).
- `hue=s=<1+saturation*0.01:.3f>`.
- `colorbalance=rs=<t*0.003>:bs=<-t*0.003>` for temperature.
- `unsharp=5:5:<sharp*0.015>:5:5:0.0`.
- `vignette=PI/<20 - v*0.15 :.2f>`.

## 6. Clip details / file details

**CLIP DETAILS** (collapsed by default):
- "Title": read-only label, the file stem, truncated to 36 chars + "…" if longer than 38. It is not editable.
- "Tag": a line edit with placeholder "e.g. Gaming, Highlight …".
- "Description": a line edit with placeholder "Short clip description …".

Saving: `editingFinished` on either edit calls `_flush_clip_details` (cv:6809). It is a no-op unless one of the edits `isModified()`. Then `metadata_manager.set(path, tag=text.strip(), description=text.strip())` (both fields are written), and `setModified(False)`. There is no debounce; it fires on Enter or focus-out. It is also flushed on `capture_session_state`. Storage: the `tag` and `description` keys of the same sqlite entry as the draft.

**FILE DETAILS** (collapsed by default), key/value rows:
- "Created": `strftime('%b %d, %Y  %H:%M')` (two spaces) of the file mtime.
- "Video Quality": `f"{w}x{h}, {format_fps(fps)}"`, shown as "Loading…" until the probe finishes, then "Unavailable" if the probe fails.
- "Video Bitrate" and "Total Bitrate": `format_bitrate(bps)`, same Loading… / Unavailable rule.
- "Duration": `m:ss`.
- "Size": `f"{MB:.1f} MB"`.
- "Location": parent dir; if longer than 36 chars it becomes "…" + last 34.

Metadata comes first from `ui.clip_grid.get_cached_clip_metadata` (tuple: duration_s, w, h, fps, video_bitrate, total_bitrate). On a cache miss it starts with 1920x1080, a 30 s duration placeholder, and a background probe.

## 7. Editor sessions (`main.py`, `ui/editor_sessions.py`)

`EditorSession(clip_path, button, state: dict|None, window_geometry, window_maximized=True)`. `MAX_EDITOR_SESSIONS = 3`.

**Open** (`main.py:~8990-9034`). It first checks the file exists; else the error "CLIP NOT FOUND" / `{name} was moved or deleted outside the app.` and a library rescan. If `clip_editor_enabled` is False it opens `BasicClipViewer` instead. If a session already exists for the path (case-normalised), it restores that. Otherwise it closes any active viewer (flushing its draft). Then if there are already 3 sessions it shows the message box "Three editors are open" / "Close a sidebar editor before opening another clip." and stops. Otherwise it creates a session and shows it.

**Park** (`_park_active_editor`, main.py:4454). It is refused (box "Export in progress" / "Finish or cancel the export before minimizing this editor.") if an export thread is alive. Otherwise it stores window geometry and maximized state, then `session.state = viewer.capture_session_state()`. Then it tears down the player (`_teardown_player(wait_for_workers=False)`) and hides the viewer, keeping one reusable shell in `_parked_editor_shell`. Parking happens on Shift+Escape (editor_minimize), the minimize button (tooltip "Pause editor in sidebar"), switching to the Clips/Settings page (`_set_main_page`), or restoring another session.

**State that survives** (`capture_session_state`, cv:8841): `snapshot` (EditorSnapshot), `position_ms` (the pending seek if any, else `max(player.position, last_stable)`), `undo` and `redo` stacks (tuples), `master_volume`, `source_volumes`, `source_mutes`, timeline `zoom` and `view_start`. Before capture it flushes clip details, trim edit, effect adjustment and the draft, forces `_play_when_ready = False`, and pauses video and the mixer. Restore (`restore_session_state`, cv:8867) restores all of that, paused, seeking once the media loads, with the time label showing the saved position.

**What is released:** the QMediaPlayer, audio output, ffmpeg audio mixer, timers, decoder workers, and (after 15 s hidden, `_idle_memory_timer`) the timeline thumbnails, poster and the GPU preview surface. Dormant sessions hold only the `EditorSession` data (state dict, path, geometry, a sidebar button). Not retained: playing state (always restored paused), thumbnails, selection of expanded panels.

**Restore:** the sidebar button click or `_navigate_sidebar`. If the file is gone or not accessible: "CLIP NOT FOUND" / "The editor clip is no longer available." and the session is removed. Otherwise the active editor is parked and the session is shown (maximized, or its old geometry if it was not maximized).

**Sidebar buttons:** 78x78 `QToolButton`, text `f"Editor {n}\n{stem elided}"` (1-based). Tooltip: `{filename}\n` + ("Active editor" | "Paused — click to restore") + `\nRight-click to close`. Right-click menu "Close editor". Closing the active one calls `_close`, others are just removed. `_editor_finished` removes the session unless it still has a `state` (parked).

**Sidebar navigation** (`Shift+Down` / `Shift+Up`, actions `sidebar_next` / `sidebar_previous`): `destinations = [0 (Clips), 1 (Settings)] + sessions`, wrapping modulo. **Switch panel**: Tab+S (`panel_switch_requested`).

**Tab+Right / Tab+Left (`editor_next` / `editor_previous`)**: emits `clip_navigation_requested(+1/-1)` -> `_navigate_editor_clip` (main.py:9758). `clip_grid.adjacent_clip(path, step)` (ui/clip_grid.py:2565) walks `navigation_paths()` = all files in the library's displayed order (section by section, as laid out in the grid; includes unmaterialised cards). From the current index it steps by +-1 until it finds a file that has a video extension (`_VIDEO_EXTS`), is ready (readiness checker), is not in `_mutation_paths`, and `os.path.isfile`. It does NOT wrap and returns None at the ends (then nothing happens). The same editor window is reused: it flushes the old session state (`capture_session_state`), `load_clip(new)`. If another session already holds the destination, that session's state is restored and the old session is removed. Otherwise the current session's `clip_path` is rewritten in place. Refused while an export is running ("Export in progress" / "Finish or cancel the export before switching clips."). A 250 ms burst rule only controls the slide animation. Clip load failure restores the previous clip ("Could not open clip").

## 8. Playback control

**Controls bar:** `time_label` `"{m:ss} / {m:ss}"` (`_fmt`: `s=int(ms/1000)`, `f"{s//60}:{s%60:02d}"`, so minutes are unbounded, no hours). The initial text is `0:00 / {dur}`. The play button toggles between "PLAY" / "PAUSE" text (or icons when available) and "Preparing…" while queued.

**Lifecycle states** (`PlayerLifecycleState`, cv:201): PREPARING, READY, PLAY_QUEUED, PLAYING, PAUSED, FAILED, STOPPED, CLOSING, CLOSED.

**Status label texts** (`_playback_status_lbl`, tooltip "Loading the media pipeline and inspecting the clip audio."): "Preparing clip…" (initial and when not ready), "Preparing · play queued", "Preparing audio…" (mixer setup, cv:7644), "Ready" (when ready; the clear-after-900 ms code compares to "READY" so the "Ready" text is never cleared, a latent bug), "BUFFERING…", "CLIP UNAVAILABLE", "Playback failed". Ready = media loaded AND audio preparation done. Audio preparation has a **1000 ms** deadline timer, after which it proceeds with container audio. The stall deadline is **15 s**, after which the state is FAILED with the status "Playback failed". The playback proxy prepare timer is 650 ms, the initial preview frame timer 300 ms.

**Toggle** (`_toggle_play`, cv:7065): ignored if closing/failed/closed, and re-entrancy guarded. If it wants to play but is not ready: it sets `_play_when_ready = True`, state PLAY_QUEUED, status "Preparing · play queued", and the button is checked showing "Preparing…". A second click while queued cancels the queue. When ready, play starts via the deferred toggle. Before starting: stop the seek timer and flush the pending seek; cancel thumbnail loading; on resume from pause record a resume anchor (guard 0.75 s, and a position report below the anchor by more than 250 ms is hidden); if the position is outside the kept ranges, jump to the next kept start.

**End of clip:** Qt goes to StoppedState at EndOfMedia -> button shows PLAY, thumbnails are scheduled after 300 ms. At the end of the last kept range during play, the playhead timer pauses and rewinds to the first kept start (section 1). Autoplay: `ClipEditorWindow.replace_clip` is called with `autoplay=True` for sessions (calls `start_autoplay`, which toggles play unless already queued/playing); the page itself defaults to `autoplay=False`. A restored session is never autoplayed (the `session_state` path forces paused).

**Keys.** Defaults (`core/navigation_bindings.py:18-30`, user-rebindable, category "Clip editor", scope application+editor):
- Split: Ctrl+C
- Delete segment: Ctrl+D
- Reinstate segment: Ctrl+W
- Quick crop: Ctrl+Q
- Export now: Ctrl+E
- Minimize: Shift+Escape
- Close: Escape
- Next clip: Tab+Right
- Previous clip: Tab+Left
- Play/pause: Space
- Undo: Ctrl+Z
- Redo: Ctrl+Y, plus alternate Ctrl+Shift+Z
- F11 toggles fullscreen (a fixed `QShortcut`, enabled only while the 'Clip editor' category is enabled).

Escape (`_handle_escape_shortcut`): if the window is fullscreen it exits fullscreen (restoring maximized or the saved normal geometry), otherwise it closes the editor (`_close`). Fullscreen button tooltips: "Maximize or restore" / "Exit fullscreen". The maximize button toggles maximized/normal. Fullscreen exit falls back to maximized if it was maximized before.

Keyboard Delete/Restore (Ctrl+D / Ctrl+W) call `_set_selected_segment_deleted(bool)`, which does nothing if the delete button is disabled or the state already matches.

## 9. Delete button and linked-original protection

**Segment button** (`delete_segment_btn`): label "Delete segment" (object name `timelineActionDanger`), tooltip "Select a split segment, then remove it from the edit", initially disabled. After a selection change it is enabled only when `len(segments) > 1`, and its text becomes "RESTORE SEGMENT" when the selected segment is deleted, else "DELETE SEGMENT" (this is uppercase unlike the initial text; a reference inconsistency).

**Clip Delete button** (sidebar bottom, 36 px high): label "Delete". If `linked_import` is true the button shows "Linked original — protected", is disabled, with tooltip "Remove its import folder from FTHR; the original stays on disk." `_delete_clip` (cv:8446) checks in order:
1. If any pending export task for this source (`export_queue.tasks()`) or share window is running: info box "Export in progress" / "Finish or cancel this clip’s exports before deleting the source." (the apostrophe is the curly U+2019).
2. Ownership: `classify_media_path(path, clips_dir, imported_roots)` where `imported_roots = settings imported_clip_folders`. If `linked_import` or ownership is not `FTHR_OWNED`: info "Linked Original Protected" / "This clip is linked from another folder. FTHR will not delete the original with the generic Delete action."
3. Confirm "Delete Clip": `Delete {basename}?\nThis cannot be undone.`
4. `finish_audio_cleanup(path)` must succeed, else "Clip is closing" / "Wait for the previous editor to finish closing, then retry.".
5. Tear down the player, remove the file, retrying 3 times with 0.2 s sleeps only for a Windows sharing violation; failure dialogs "Delete Failed". Also remove the cached thumbnail and duration cache files, set `_discard_editor_draft`, stop the draft timer, call `clear_editor_draft`, then `accept()` (closing the editor).

## 10. User-visible strings

**Title bar:** window title `FTHR — {filename}`; minimize tooltip "Pause editor in sidebar"; maximize tooltip "Maximize or restore"; close button label "Close".

**Playback:** play/pause "PLAY", "PAUSE", "Preparing…"; the status texts from section 8; time label as above.

**Timeline header:** "Timeline"; range label `m:ss → m:ss` while trimming (cv:7816), or `"{kept m:ss} kept  ·  {n} cut"` with `"cuts"` for any n other than 1 (two spaces around the middle dot); buttons "Undo" / "Redo" with tooltips "Undo last edit  ·  Ctrl+Z" and "Redo edit  ·  Ctrl+Y or Ctrl+Shift+Z"; "Pan timeline left", "Zoom out", "Zoom in", "Pan timeline right"; zoom label `N%`; "Split" (tooltip "Split the selected segment at the playhead"; transient "Move playhead"); "Delete segment" / "DELETE SEGMENT" / "RESTORE SEGMENT". Context menu: "Split here", "Delete segment", "Restore segment", "Hide deleted segments".

**Sidebar actions:** "Share clip"; the quality combo (accessible name "Export and share quality", tooltip "Used for Export and Share. Lossless saves filtered frames in MKV; files can be very large.") with options "Very fast" (`fast`), "High quality" (`quality`, default), "Lossless · MKV" (`lossless`), stored in setting `export_quality`; "Export" then "Export clip" after submit (transient "Export cancelled", "exported", "Export failed"; back to "Export clip" after 3000 ms; cancelled/failed hints in `crop_info_lbl` "Export cancelled" or the first 38 chars of the error + "…" for 6000 ms); "Export jobs" with the suffix ` · {n} pending` / ` · {n} failed` / ` · {n} completed` (refreshed every 1000 ms); "Upload". Export errors: "EXPORT FAILED" with "The edit has no kept segments." (total kept under 0.1 s) or "FFmpeg returned an error. The output file may be incomplete.". Exports go to `{clips_dir}/Exported`, audio at 320k.

**Panels (order, with default expanded state):** "CLIP SPEED" (expanded; "Playback speed", "Preserve pitch" with tooltips "Playback and export speed. Audio follows this rate." and "Keep voices and music at their original pitch while changing speed."), "EDIT TOOLS" (expanded; "Set crop"/"Edit crop", "Quick", "Save as quick crop", "saved", "Stretch video" with tooltip "Open a crop-aware preview and drag either side", "Reset"), "AUDIO MIX" (expanded; rows with `%` values and "MUTE"/"MUTED" buttons, slider range 0..100, tooltip `{label} gain for this open clip only.`, master label "MASTER", `master_volume` setting default 80), "COLOR & EFFECTS" (collapsed; Exposure, Contrast, Saturation, Temperature, Sharpness, Vignette, "Reset look"), "CLIP DETAILS" (collapsed; Title, Tag, Description), "FILE DETAILS" (collapsed; Created, Video Quality, Video Bitrate, Total Bitrate, Duration, Size, Location). Then "Delete" / "Linked original — protected" at the bottom.

**Settings keys used by the editor:** `quick_crop`, `master_volume`, `hide_deleted_timeline_segments`, `timeline_preview_strip_enabled`, `export_quality`, `clip_editor_live_preview`, `editor_color_preview_enabled`, `clip_editor_enabled`, `imported_clip_folders`, plus export-side `bitrate_level`, `resolution`, `watermark_enabled`.

**Reference quirks worth not copying blindly:** the "READY" vs "Ready" status mismatch (section 8), the "Delete segment" vs "DELETE SEGMENT" casing (section 9), and `edited_duration` in the metadata manager takes the whole draft but reads the state keys, so it only works because `get_editor_draft` returns the `state` dict.
