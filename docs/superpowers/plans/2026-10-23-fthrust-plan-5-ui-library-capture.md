# FTHRust Plan 5: Library and Capture UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `fthrust` window becomes a working recorder: the Clips page with day sections of clip cards, thumbnails and library watching; the engine is launched, supervised and restarted by the UI; save-clip, manual recording and screenshots work from hotkeys delivered over the private socket and the GlobalShortcuts portal; game detection and focus pause run on Wayland toplevels; the capture-card notification and sounds appear over other windows; one instance at a time; diagnostics events flow for every subsystem.

**Architecture:** `fthrust-ui` gains three module trees. `library/` holds the Clips page (immediate-mode grid that paints only the visible rows, cards, rename/delete, thumbnails through a worker pool). `services/` is a tokio runtime on a background thread; every service (engine supervisor, save state, recording, screenshots, hotkeys, game detection, sounds, instance activation) talks to the UI only through `services::Outbox` events drained by `services::tick(state)` once per frame and `services::Inbox` requests the UI sets as `AppState` flags. `overlay/` draws the capture card on a wlr-layer-shell surface with egui over wgpu (desktop notification fallback). `fthrust-core` gains the pure ports (clip readiness, screenshot staging, game rules, Hyprland config, instance lock); `fthrust-media` gains the library thumbnail cache and a sound decoder; `fthrust-wayland` gains the toplevel-list and layer-shell clients.

**Tech Stack:** egui 0.37 / eframe 0.37 (existing), egui-wgpu 0.37 + raw-window-handle 0.6 (overlay surface), wayland-protocols-wlr 0.3 (`layer_shell`, `foreign_toplevel`), ashpd 0.13 (`screenshot`, `global_shortcuts`, `notification` features), tokio 1 (workspace), notify 8 (inotify; licence CC0-1.0 added to `deny.toml`), pipewire 0.10 (sound playback), ffmpeg-next 9 (sound decode, thumbnails), nix 0.31 (`flock`, `statvfs`), `fthrust-core`, `fthrust-media`, `fthrust-ipc`, `fthrust-wayland`, `fthrust-portal`.

**Spec:** `docs/superpowers/specs/2026-10-22-fthrust-design.md` sections 5.1, 5.2, 5.4, 5.5, 5.6, 5.10, 6.1, 6.4 (out-of-window surfaces, game detection), 7, 9 "UI", 11 items 7, 8, 12, 13. Reference: `docs/reference/reference-ui-inventory.md` sections 1.3, 1.4, 1.10, 4.5, 4.6, 5.2–5.6, 6.1–6.4, 6.7, 6.9, 6.11; `reference/FTHR_UI/{main.py, ui/clip_grid.py, ui/capture_card.py, core/hotkey_manager.py, core/game_detector.py, core/focus_monitor.py, core/screenshot_save.py, core/wayland_screenshot_portal.py, core/clip_readiness.py, core/single_instance.py, core/instance_activation.py}`.

## Global Constraints

- Everything in plan 4's Global Constraints still holds (tokens only source of colours, `#![forbid(unsafe_code)]` in `fthrust-ui`, fidelity gate rules, gate commands, commit after every task). Branch `plan-5-ui-library-capture` from `main`.
- "The UI launches the engine, as today." The engine binary is `fthrust-engine`, found at `$FTHRUST_ENGINE`, then next to the running executable, then `target/{debug,release}/fthrust-engine` in development. Missing → status `NO ENGINE`, `ENGINE NOT FOUND` (Error 003) keyed `engine-launch`.
- "Startup handshake, generation counters, save state machine timeouts (1 s acknowledgement, 60 s completion with a non-verdict "slow" notice) and the health states … keep the reference's semantics and status strings exactly." Status strings in this plan are copied from `main.py` and are the only ones allowed.
- "Socket path (always on, and the only path on Hyprland): a private Unix socket at `$XDG_RUNTIME_DIR/fthrust/hotkey.sock` … The client command is `fthrust --hotkey <action>` instead of `echo | nc -U`." Managed file `~/.config/hypr/fthr-hotkeys.conf`, `source = …` appended to `hyprland.conf` once, Lua variant honoured, `hyprctl reload` only when the file changed.
- "Screenshots: Screenshot portal, never a window grab." Files `<root>/Screenshots/screenshot_from_%Y%m%d_%H%M%S_%f.png`, `_01` suffixes, staged `.<stem>.<rand>.png.partial` in the same directory, published with a no-replace rename.
- "Game detection on Wayland: … Rules match on title and app-id (`title_contains` and a new `app_id` field)." Compositors without a toplevel protocol show "Game detection unavailable".
- "Capture card and Gary overlay use layer-shell, with desktop notification fallback." Card geometry and timings: full 340×116, compact 320×76, scale `clamp(screen_h/1080, 0.65, 1.0)`, margin 16 from the top-right of the output, slide in 450 ms OutBack, hold 2400 ms, slide out 350 ms InCubic, shimmer 550 ms starting 150 ms after show.
- "Single instance: a second launch activates the first." Lock `~/.fthr/fthr.lock` (`flock`, fail-open), activation socket `$XDG_RUNTIME_DIR/fthrust/instance.sock`, message `activate\n`, 350 ms timeouts.
- On-disk contract (spec 7): clip names `<prefix>_clip_from_%d%b%Y_%H-%M-%S.mp4` with `_2`, `_3` suffixes; recordings `FTHR_recording_%Y-%m-%d_%H-%M-%S[_N].mp4`; thumbnails `~/.fthr/thumbnails/<md5>.jpg` + `.meta-v3` + `.failed`; `hotkeys.json`, `hotkeys-portal-cleared.json`; settings keys exactly as the reference (`capture_mode`, `target_window_name`, `game_detection_*`, `notification_monitor`, `capture_card_enabled`, `notification_sounds_enabled`, `sound_volume_*`, `imported_clip_folders`, `clips_directory`, `recording_directory`).
- Fidelity screens owned by this plan: `library`, `library-small`, `record-hover`, `clip-card`, `clip-card-hover`, `clip-card-feedback` (`tests/fidelity_all.rs` ignores are removed as each lands).
- Tests never touch the real `~/.fthr`, `~/.config/hypr` or `$XDG_RUNTIME_DIR`: every function that reads them takes a base path (`FthrPaths`, `home: &Path`, `runtime_dir: &Path`).

## Review Focus

1. A clip saved while the clips root is on a filesystem that fails mid-write (disk full): the engine's `SaveResult(Err)` becomes `CLIP WAS NOT SAVED` with the `OPEN FOLDER` action, the status returns to the health text after 3 s, and the library never shows a card for the missing file. Test in Task 10.
2. The engine process dies between two status events: the next tick reports `ENGINE STOPPED` (Error 006, keyed `engine-process-exit`, `RESTART ENGINE` action), the Record button and Save are refused with `CAPTURE UNAVAILABLE`, and a restart from the error bar brings a new generation up without the old incident lingering. Test in Task 9.
3. A hotkey socket client that connects and sends nothing (or 300 bytes of junk): the server drops it within 500 ms, never blocks other clients, and logs "Unknown command". Test in Task 13.
4. The notification monitor named in `notification_monitor` is unplugged: the card falls back to the highest-refresh output, and with no layer-shell at all a desktop notification is sent instead of silently dropping the clip confirmation. Test in Task 15.
5. A library root with 5 000 clips: the first scan batch paints within one frame, thumbnails are only requested for visible cards, and scrolling never allocates one texture per card. Test in Task 7.

---

## File structure

```
crates/fthrust-core/src/
  clip_readiness.rs        ClipReadinessRegistry (states, warnings, can_access)
  screenshot_save.rs       reserve_screenshot_paths, publish_staged_png, PNG validation limits
  game_rules.rs            CustomGameRule {title_contains, app_id, exe_* kept}, normalise, matching, blacklist, crop profile
  single_instance.rs       InstanceLock (flock), ACTIVATE message, activation socket path
  hotkeys/hyprland.rs      bind line, managed conf, Lua block, source line, socket_command
crates/fthrust-media/src/
  thumbnail/library.rs     cached library thumbnail: key, meta-v3, failed marker, jpg 85, prune
  audio_decode.rs          decode_to_pcm(path) -> Pcm (48 kHz stereo f32)
crates/fthrust-wayland/src/
  toplevels.rs             wlr-foreign-toplevel-management client → Vec<Toplevel{title, app_id, fullscreen, activated, id}>
  layer_shell.rs           LayerSurface (overlay layer, anchor, margins, size, configure)
crates/fthrust-ui/src/
  library/{mod,model,layout,page,card,thumbs,scan}.rs
  services/{mod,runtime,engine,status,save,recording,screenshot,hotkeys,portal_shortcuts,game,focus,sounds,instance,diagnostics}.rs
  overlay/{mod,surface,capture_card}.rs
  shell/toolbar.rs (Record states, Source/Game labels), shell/mod.rs (actions), main.rs (--hotkey, instance)
crates/fthrust-ui/tests/
  core ports: readiness.rs, screenshot_save.rs (in core), game_rules.rs, instance.rs, hyprland.rs
  library_layout.rs, library_model.rs, clip_card.rs, library_page.rs, services_engine.rs, services_save.rs,
  services_recording.rs, services_screenshot.rs, services_hotkeys.rs, services_game.rs, overlay_card.rs, sounds.rs
tools/check-assets.sh (unchanged), docs/capture-and-library.md
```

---

### Task 1: Core ports — clip readiness, screenshot staging, game rules, instance lock

**Files:** `crates/fthrust-core/src/{clip_readiness,screenshot_save,game_rules,single_instance}.rs`, `lib.rs`; tests `crates/fthrust-core/tests/{clip_readiness,screenshot_save,game_rules,single_instance}.rs`.

**Interfaces (Produces):**
- `clip_readiness::{ReadinessState { EngineCommitted, Finalizing, Ready, ReadyWithWarning, FinalizationFailed }, ClipReadinessRegistry::new(), engine_committed(&mut self, path, needs_finalization: bool), ready(path), record_warning(path, msg), complete(path), finalization_failed(path, msg, base_clip_usable), state(path) -> ReadinessState (untracked existing file → Ready, untracked missing → FinalizationFailed), can_access(path) -> bool, warnings(path) -> Vec<String>, forget(path)}`; keys are `canonicalize`d when the file exists else normalised absolute paths.
- `screenshot_save::{ScreenshotPaths { final_path, staged }, ScreenshotError { code: &'static str, detail: String }, reserve_screenshot_paths(dir, now: OffsetDateTime) -> Result<ScreenshotPaths, ScreenshotError>, publish_staged_png(staged, final_path) -> Result<(), ScreenshotError>, validate_png_bytes(bytes) -> Result<(u32, u32), ScreenshotError>, MAX_PNG_BYTES = 64 MiB, MAX_PNG_SIDE = 16384, MAX_PNG_PIXELS = 100_000_000}`. Codes: `OUTPUT_DIRECTORY_UNAVAILABLE`, `OUTPUT_COLLISION`, `WRITE_FAILED`, `IMAGE_ENCODE_FAILED`, `INVALID_IMAGE`.
- `game_rules::{CustomGameRule { title_contains: String, app_id: String, exe_name: String, exe_path: String, folder_path: String, crop: Option<CropProfile> }, normalise_rules(&Value) -> Vec<CustomGameRule>, matching_rule<'a>(rules: &'a [CustomGameRule], title: &str, app_id: &str) -> Option<&'a CustomGameRule>, is_blacklisted(rules, title, app_id) -> bool, to_value(&[CustomGameRule]) -> Value}` (unknown keys dropped, empty rules dropped, `title_contains` match is case-insensitive substring, `app_id` match is case-insensitive equality).
- `single_instance::{InstanceLock::acquire(lock_file: &Path) -> LockResult { Acquired(InstanceLock), Held, Unavailable }, ACTIVATE: &[u8] = b"activate\n", activation_socket_path(runtime_dir) -> PathBuf (`<runtime>/instance.sock`)}`; drop releases the lock.

- [ ] Tests (values from `reference/tests/test_clip_readiness.py`, `test_screenshot_save.py`, `test_game_detector.py`, `test_single_instance.py` — port each assertion that is not Windows-only): `readiness_untracked_existing_is_ready`, `readiness_finalizing_then_warning_is_ready_with_warning`, `screenshot_reserve_names_and_collision_suffix_01` (`screenshot_from_20261023_101500_123456.png`, second call → `_01`), `screenshot_publish_refuses_existing` (`OUTPUT_COLLISION`), `screenshot_png_limits` (65 MiB → `INVALID_IMAGE`), `game_rules_round_trip_and_match` (rule `{"title_contains": "overwatch"}` matches title "Overwatch 2", `{"app_id": "steam_app_2357570"}` matches app id case-insensitively, blacklist wins), `instance_lock_second_acquire_is_held` (two locks on one temp file), `instance_lock_fail_open_on_unwritable_dir`.
- [ ] RED → implement → GREEN → commit `feat(core): clip readiness, screenshot staging, game rules, instance lock`.

---

### Task 2: Core Hyprland hotkey config and socket command

**Files:** `crates/fthrust-core/src/hotkeys/hyprland.rs` (make `hotkeys.rs` a directory module), tests `crates/fthrust-core/tests/hyprland.rs`.

**Interfaces (Produces):**
- `hotkeys::hyprland::{to_hyprland_bind(combo) -> (String, String)` (`"Ctrl+Shift+S"` → `("CTRL SHIFT", "S")`, `"F9"` → `("", "F9")`), `socket_command(action, client_exe: &Path, sock: &Path) -> String` = `"<client_exe>" --hotkey <action>`, `bind_line(action, combo, client_exe, sock) -> String` = `bind = CTRL SHIFT, S, exec, "<exe>" --hotkey save_clip`, `bind_actions(&HotkeyBindings) -> Vec<String>` (the six socket actions then custom clips ordered by seconds; only those with a key), `write_managed_conf(home, &HotkeyBindings, client_exe, sock) -> io::Result<bool>` (writes `<home>/.config/hypr/fthr-hotkeys.conf` with the two header comment lines from the reference, atomic via `.conf.tmp`, returns `true` when content changed), `ensure_source_line(home) -> io::Result<()>` (appends `\n# FTHR Clips hotkeys (auto-added)\nsource = <home>/.config/hypr/fthr-hotkeys.conf\n` unless `hyprland.conf` is missing or already mentions `fthr-hotkeys.conf`, `fthr_hotkey.sock` or `hotkey.sock`), `write_lua_block(path, &HotkeyBindings, client_exe, sock) -> io::Result<bool>` (managed block between `-- FTHR Clips hotkeys begin (managed)` and `-- FTHR Clips hotkeys end (managed)` with `hl.bind("CTRL + SHIFT + S", hl.dsp.exec_cmd("…"), { description = "FTHR Clips: save_clip" })` lines; an incomplete block is an error), `apply(home, &HotkeyBindings, client_exe, sock) -> io::Result<Applied { changed: bool, lua: bool }>` (Lua variant when `<home>/.config/hypr/hyprland.lua` or `custom/keybinds.lua` exists)}`.
- `hotkeys::setup_instructions(compositor, client_exe, sock) -> String` moved here from `fthrust-ui::popups::hotkeys` with the client command instead of `nc` (KDE heading, GNOME `bash -c '<cmd>'` lines, generic heading).

- [ ] Tests: `bind_translation_matches_reference` (table of 6 combos), `managed_conf_written_once_and_unchanged_returns_false`, `source_line_appended_once`, `source_line_skipped_when_user_has_socket_binds`, `lua_block_replaced_in_place_and_incomplete_block_refused`, `apply_prefers_lua_when_present`.
- [ ] RED → implement → GREEN → update the UI popup to call the core function (hotkey-popup fidelity still 0 failures) → commit `feat(core): Hyprland managed hotkey config`.

---

### Task 3: Media — library thumbnail cache and sound decoding

**Files:** `crates/fthrust-media/src/thumbnail/library.rs`, `src/audio_decode.rs`, `lib.rs`; tests `crates/fthrust-media/tests/{thumbnail_cache,audio_decode}.rs`.

**Interfaces (Produces):**
- `thumbnail::library::{CachedThumbnail { jpg: PathBuf, meta: MetaV3, duration_s: f64 }, cache_key(path) -> io::Result<String>` (`md5("{canonical}|{size}|{mtime_ns}|aspect-v4")` via `fthrust_core::thumbnails::thumbnail_key`), `load_or_build(cache_dir, path, cancel: &AtomicBool) -> Result<Option<CachedThumbnail>, MediaError>` (cache hit when `.jpg` and `.meta-v3` exist and parse; a `.failed` marker younger than 30 s → `Ok(None)` without probing; otherwise probe (`probe_video_metadata`), write meta, `extract_frame(path, 0.0, 640, 360)` → JPEG quality 85 via temp `*.tmp-<pid>-<tid>.jpg` + rename; decode failure writes the `.failed` marker; afterwards `fthrust_core::thumbnails::prune` at most every 30 s), `image_thumbnail(path) -> Result<RgbaImage>` (PNG/JPEG decoded and fitted within 640×360)}`.
- `audio_decode::{Pcm { rate: u32 = 48000, channels: u16 = 2, samples: Vec<f32> }, decode_to_pcm(path) -> Result<Pcm, MediaError>}` using libswresample to 48 kHz stereo f32 interleaved; capped at 30 s of audio.

- [ ] Tests (fixtures: `crates/fthrust-media/tests/fixtures/` synthetic clip from plan 2, `assets/sounds/clip_captured.wav`): `thumbnail_cache_miss_writes_jpg_meta_and_hit_skips_probe` (second call returns in < 50 ms and does not change mtime), `thumbnail_failed_marker_short_circuits_for_30s` (a text file named `.mp4` → `Ok(None)` and `.failed` exists; second call before 30 s does not touch the file), `decode_wav_to_48k_stereo` (`clip_captured.wav` → `rate 48000`, `channels 2`, `samples.len() % 2 == 0`, peak > 0.05), `decode_rejects_non_audio` (a PNG → `Err`).
- [ ] RED → implement → GREEN → commit `feat(media): library thumbnail cache and sound decoding`.

---

### Task 4: Wayland — toplevel list and layer-shell surfaces

**Files:** `crates/fthrust-wayland/src/{toplevels,layer_shell}.rs`, `lib.rs`, `Cargo.toml` (`wayland-protocols-wlr` features already include `client`); tests `crates/fthrust-wayland/tests/{toplevels_model,layer_shell_live}.rs`.

**Interfaces (Produces):**
- `toplevels::{Toplevel { id: u32, title: String, app_id: String, fullscreen: bool, activated: bool, minimized: bool }, ToplevelTracker::new() , apply(&mut self, ev: ToplevelEvent) (Title/AppId/State/Closed/Done per the protocol; a toplevel is published only after `done`), snapshot(&self) -> Vec<Toplevel> (stable id order), active(&self) -> Option<&Toplevel>}`; `ToplevelWatcher::connect() -> Result<ToplevelWatcher, ConnectError>` binding `zwlr_foreign_toplevel_manager_v1` (version ≤ 3) on its own connection and thread, `available() -> bool`, `snapshot() -> Vec<Toplevel>`, `changes() -> &Receiver<()>` (one message per `done` batch), `close()`.
- `layer_shell::{Anchor { TopRight }, LayerSurface::create(conn: &WaylandConnection, output: Option<&wl_output::WlOutput>, size: (u32, u32), margins: (i32, i32, i32, i32), namespace: &str) -> Result<LayerSurface, LayerError>` (`zwlr_layer_shell_v1` layer `overlay`, anchor top|right, exclusive zone 0, keyboard interactivity none, waits for the first `configure` and acks it), `available(conn) -> bool`, `set_margins(&mut self, right: i32, top: i32)`, `wl_surface(&self) -> &WlSurface`, `display_handle()/window_handle()` returning `raw_window_handle::{WaylandDisplayHandle, WaylandWindowHandle}` for wgpu (via the connection's backend pointers; this module is the one `unsafe`-free wrapper, the handles are built with `wayland_backend::client::Backend::display_ptr` / `ObjectId::as_ptr` which are safe accessors)}`.

- [ ] Tests: `toplevel_tracker_publishes_only_after_done` (Title then State then Done → one toplevel; State alone → no change), `toplevel_fullscreen_and_activated_states_parse` (protocol state values `maximized 0, minimized 1, activated 2, fullscreen 3`: a state array `[2, 3]` → `activated && fullscreen`, `[]` → neither), `toplevel_closed_removes`, `#[ignore] layer_shell_live_creates_and_configures` (on this machine: a 340×116 overlay surface at top-right of the highest-refresh output is configured within 1 s).
- [ ] RED → implement → GREEN → commit `feat(wayland): foreign toplevel list and layer-shell surfaces`.

---

### Task 5: Library model, layout and scan service

**Files:** `crates/fthrust-ui/src/library/{mod,model,layout,scan}.rs`, `src/state.rs`; tests `tests/{library_layout,library_model}.rs`.

**Interfaces (Produces):**
- `library::layout::{CARD_W = 360, CARD_MIN_W = 300, CARD_MAX_W = 440, CARD_BODY_H = 88, GRID_SPACING = 12, H_MARGIN = 48, SECTION_SPACING = 20, metrics(viewport_width: f32) -> (usize, Vec<f32>)}` — the exact port of `ClipGrid._layout_metrics`; `card_height(width) = (width * 9 / 16).round() + 88`.
- `library::model::{Filter { All, Clips, Screenshots, Imported }, Sort { Newest, Oldest }, heading(Filter) -> &str` (`ALL CLIPS`, `CLIPS`, `SCREENSHOTS`, `IMPORTED CLIPS`), `section_label(mtime) -> String` (`%a, %b %d` local time), `game_name(path, clips_root) -> String` (`Desktop` when the parent is the root), `title_from_path(path) -> String` (stem), `Library { records: Vec<LibraryRecord>, filter, sort, sections: Vec<Section { label, game: Option<String>, paths: Vec<PathBuf> }>, count, empty: Option<EmptyState { title, detail: Option<String> }> }`, `Library::rebuild(&mut self, screenshot_key: &str)` (groups by section label in sorted order; empty titles `No clips yet` / `No screenshots yet` + `PRESS <key> TO TAKE A SCREENSHOT` / `No imported clips`), `upsert(path, imported)`, `remove(path)`, `rename(old, new)`, `navigation_paths(videos_only) -> Vec<PathBuf>`, `adjacent(path, step) -> Option<PathBuf>`}`.
- `library::scan::{ScanService::new(outbox: Outbox)` on the services runtime: `request(ScanKind { Foreground, Watcher, Timer })` coalesced (an active scan is cancelled and one follow-up retained), runs `fthrust_core::library_index::scan(&ScanRoots, cancel, on_batch)` on `spawn_blocking`, emits `Event::ScanBatch(Vec<LibraryRecord>)` for the first batch and `Event::ScanDone { records, imported, watched_dirs, elapsed_ms }`; a `notify` watcher on the clips root, every game subfolder and every import root (`watched_dirs`, ≤ 256), events coalesced for 200 ms; a 30 s timer refresh}`.
- `AppState` gains `library: Library`, `readiness: ClipReadinessRegistry`, `thumbnails: HashMap<PathBuf, ThumbState>`, `library_scroll: f32`, `selected_clip: Option<PathBuf>`, `rename_draft: Option<(PathBuf, String)>`.

- [ ] Tests: `layout_metrics_match_reference` (viewport 1298 → `(3, [409, 409, 408])`; 1006 → `(3, [312, 311, 311])`; 400 → `(1, [352])`; 2560 → `(6, [409, 409, 409, 409, 408, 408])`), `sections_group_by_day_and_name_game_folder`, `empty_state_texts_per_filter`, `upsert_rename_remove_keep_order`, `adjacent_skips_images_and_unready` (readiness registry says Finalizing).
- [ ] RED → implement → GREEN → commit `feat(ui): library model, layout and scan service`.

---

### Task 6: Clip card widget

**Files:** `crates/fthrust-ui/src/library/{card,thumbs}.rs`; tests `tests/clip_card.rs`. Reference JSON: `clip-card.json`, `clip-card-hover.json`, `clip-card-feedback.json` (320×268).

**Interfaces (Produces):**
- `library::card::{CardData { path, title, metadata: String, duration_s: Option<f64>, is_video, imported, uploaded, upload_link: Option<String>, ready, feedback: Option<String>, thumbnail: Option<TextureId> }, CardResponse { opened, rename_committed: Option<String>, delete_requested, share_clicked, hovered }, ui(ui, t, icons, width, &CardData, editing: &mut Option<String>, name) -> CardResponse}`. Geometry: card `CARD_BG` r8, thumb `width × round(width·9/16)` with the thumbnail cropped to fill, top corners r8, a `CARD_BG` fade over the bottom 36 % of the thumb, play icon 36 px centred on hover (videos, ready); duration badge `m:ss` 14 px padding 2×9 r4 `SURFACE_1@230` at right 10 / bottom 8 (`--:--` when unknown); `Imported` / `Uploaded` badges stacked bottom-left 8 px inset (`ACCENT` bg, `BG` text, display 12 px, padding 2×7); `Finalizing` badge top-left 8,8 (`WARNING` bg); action feedback label at 8,8 (`SURFACE_3`, 1 px `ACCENT_DIM`, 14 px, padding 4×8); body 88 px margins 10 spacing 8: title 15/600 elided (double-click or F2 → inline edit `SURFACE_1`, 1 px `ACCENT`, 17 px, padding 0×3, Enter commits, Escape cancels), metadata 12/400 `TEXT_DIM` (`Reading clip details…` → `1080p · 60 FPS · 12 Mbps`, or `Screenshot`), share 48×52 (`ACCENT` bg, icon `upload`/`link` 28 px, hover `ACCENT_SOFT` + `ACCENT` icon), delete 48×52 (`DELETE` bg, hover `ERROR_SOFT` + `ERROR` border); disabled buttons `SURFACE_3` + `BORDER`.
- `library::thumbs::{ThumbState { Pending, Ready { texture: TextureHandle, meta: Option<MetaV3> }, Failed }, ThumbPool::new(outbox, cache_dir)` with two blocking workers and a 128-job queue; `request(path, generation, fingerprint)`; results arrive as `Event::Thumbnail { path, generation, result: Option<CachedThumbnail> }`; `cancel_generation()`; textures created on the UI thread from the JPEG (`image` crate), ≤ 96 live textures (an LRU drops the oldest off-screen card's texture)}`.

- [ ] Tests: `clip_card_matches_qt` (fidelity against `clip-card`: markers `cardThumb`, `cardDurationBadge` "0:12" at `(262,147,48,25)`, `cardBody`, `cardTitle` `(10,190,188,32)`, `cardMetadata` `(10,225,188,33)`, `cardShareBtn` `(206,198,48,52)`, `cardDeleteBtn` `(262,198,48,52)`), `clip_card_hover_shows_play_icon` (fidelity `clip-card-hover` + a 36 px icon pixel sample at the thumb centre), `clip_card_feedback_label` (fidelity `clip-card-feedback` with feedback "Link copied" at `(8,8,112,31)`), `rename_enter_commits_escape_cancels` (kittest: F2, type, Enter → `rename_committed == Some("new name")`), `thumb_pool_caps_live_textures_at_96`.
- [ ] RED → implement → GREEN → commit `feat(ui): clip card`.

---

### Task 7: Clips page

**Files:** `crates/fthrust-ui/src/library/page.rs`, `src/pages.rs` (replace `clips_placeholder`), `src/shell/mod.rs` (keyboard navigation); tests `tests/library_page.rs`. Reference JSON: `library.json`, `library-small.json`, `record-hover.json`.

**Interfaces (Produces):**
- `library::page::ui(ui, &mut AppState)`: page margins (24, 18, 24, 24); filter bar 60 tall (`filterBar` fill as the JSON samples, r6, 1 px bottom `SHELL_DIVIDER`), inner margins 12/0 spacing 12: `allCountLabel` 14/600 (`ALL CLIPS  (12)` with two spaces, bare heading when empty), stretch, filter combo 140 wide (`_DropdownCombo[All clips]` at x = right − 292), sort combo 140 wide; 8 px gap; sections 20 apart: header row spacing 10 with date 17/600 display and `·  <game>` 15 `TEXT_DIM`, grid of cards with spacing 12 using `layout::metrics(viewport − 48)` (the viewport is the page width without the 8 px scrollbar when the content overflows); empty state: top margin 96, title 26/600 centred, detail 14 display `TEXT_DIM`, 12 px gap, 56×2 `ACCENT` rule. Only rows intersecting the viewport ± 2 rows are painted and only their cards request thumbnails; smooth scroll via `widgets::scroll::vertical`; scrolling pauses thumbnail requests for 90 ms after the last wheel event.
- Keyboard navigation (`fthrust_core::navigation` actions, honouring category enable flags): `library_scroll_up/down` scroll by one row, `category_next/previous` cycle the filter combo, `sidebar_next/previous` cycle pages/sessions, `switch_panel` toggles Clips/Settings, `toggle_detection` toggles `game_detection_enabled`, `restart_capture` sets `engine_restart_requested`.
- Card actions: open → `state.editor_requested_path = Some(path)` (plan 6 consumes); screenshot click → `state.screenshot_open_requested`; delete → `DialogKind::Question { tag: "delete-clip:<path>" }` ("Delete <name>?\nThis cannot be undone." plus the linked-original paragraph for imports; linked originals refuse the card button with the `Linked Original Protected` message); rename → `fthrust_core::audio_manifest::rename_clip` on a blocking task with `Event::ClipRenamed`; share → `state.upload_requested = Some(path)` or copies the link (`ctx.copy_text`) with feedback `Link copied` / `SFTP location copied` for 2.5 s.

- [ ] Tests: `library_page_matches_qt` (fidelity `library` and `library-small`; `record-hover` with the Record button hovered — the Record marker is compared with `enabled` ignored, see Task 11), `only_visible_rows_request_thumbnails` (5 000 records at 1440×900: ≤ 3 rows × cols requests after the first frame, frame time of the first paint < 16 ms on the CPU side measured with `Instant`), `filter_and_sort_rebuild_sections`, `delete_question_then_file_removed`, `rename_updates_card_and_manifest`.
- [ ] RED → implement → GREEN → remove the `library_small` and `clip_cards` ignores from `fidelity_all.rs` → commit `feat(ui): clips page`.

---

### Task 8: Services runtime, diagnostics, single instance and the hotkey client

**Files:** `crates/fthrust-ui/src/services/{mod,runtime,diagnostics,instance}.rs`, `src/main.rs`, `src/lib.rs` (`App` owns `Services`); tests `tests/{services_runtime,instance}.rs`.

**Interfaces (Produces):**
- `services::{Event, Request, Outbox (mpsc::Sender<Event>), Inbox (mpsc::Receiver<Request>), Services::start(paths, settings snapshot, ctx: egui::Context) -> Services` (tokio multi-thread runtime, 2 workers, on a named thread `fthrust-services`; every `Event` send calls `ctx.request_repaint()`), `Services::tick(&mut self, state: &mut AppState)` (drains events into state, then reads `AppState` request flags, clears them and sends `Request`s), `Services::shutdown(self)` (bounded 3 s)}`.
- `services::diagnostics::{DiagnosticsSession started at launch (`DiagnosticsSession::start(&paths, VERSION)`), emit(subsystem, event, fields: Value)` used by every service; the Troubleshooting export calls the existing `dialogs::diagnostic_report::export`}`.
- `services::instance::{try_become_primary(paths, runtime_dir) -> Primary | Secondary` (lock then an activation listener on `instance.sock`; a `Secondary` sends `activate\n` with a 350 ms timeout and exits 0), `Event::ActivationRequested` → `ViewportCommand::Focus` + un-minimise}`.
- `main.rs`: `fthrust --hotkey <action>` connects to `<runtime>/hotkey.sock`, writes the action, exits 0 (1 if the socket is missing, printing `FTHR Clips is not running`); `fthrust --version`; single-instance check before `eframe::run_native`; on first start logs a warning when `~/.fthr/fthr.lock` is held by a process whose cmdline contains `main.py` or `/dev/shm/FTHR_SharedMemory_v4` exists (spec 7).

- [ ] Tests: `tick_drains_events_and_sends_requests` (an `Event::Status` sets `state.status`; `state.scan_clips_requested = true` → one `Request::Scan`), `secondary_instance_activates_primary` (two `InstanceLock`s on a temp dir; the second sends `activate\n` over the temp socket and the listener yields `ActivationRequested` within 350 ms), `hotkey_client_writes_action` (a temp Unix listener receives `save_clip`), `hotkey_client_without_socket_exits_1`.
- [ ] RED → implement → GREEN → commit `feat(ui): services runtime, diagnostics session, single instance`.

---

### Task 9: Engine supervisor and status

**Files:** `crates/fthrust-ui/src/services/{engine,status}.rs`, `src/shell/toolbar.rs` (encoder label); tests `tests/services_engine.rs`.

**Interfaces (Produces):**
- `services::engine::{discover_engine(env, exe_dir, dev_target) -> Option<PathBuf>, EngineSupervisor` (state: `generation: u32`, `process: Option<Child>`, `client: Option<EngineClient>`, `health: CaptureHealthMonitor`, `tracker: CaptureConfigTracker`, `recovery: RecoveryTracker`, `incidents: HashMap<String, Incident>`, `pending_launch: Option<(CaptureConfig, u32)>`, `restart_pending`, `apply_deadline: Option<Instant>` (30 s), `last_event: Instant`); requests `Start`, `Stop`, `Restart { allow_recording_prepare }`, `Pause`, `Resume`, `Configure(CaptureConfig)`; events `Event::EngineStatus(StatusView)`, `Event::EngineError { title, detail, level, actions, key, dismiss_on_action }`, `Event::IncidentCleared(key)`, `Event::EncoderActive(String)`, `Event::StartupWarning { code, detail }`}`.
- Behaviour (from `main.py`): spawn `<engine> --socket <runtime>/engine.sock` with stderr to `~/.fthr/logs/engine.log` (rotating writer from core) and stdout read for `FTHR_STARTUP_ERROR` / `FTHR_STARTUP_WARNING` lines; connect attempts 60 × 150 ms; on `Hello` → `Configure(EngineConfig { capture: CaptureConfig::from_settings, max_buffer_mb: linux_ring_budget_mb(bitrate, buffer_seconds), backend_override: env })`, status `STARTING CAPTURE`; `Configured { warnings }` → one warning entry each; `StartupError { code, detail }` → `CAPTURE FAILED` (`Error 001`) when the code is a known native code else `ENGINE NOT RESPONDING` (`Error 004`); handshake timeout 9 s → same; `Status` every 500 ms (plus `Heartbeat`) feeds `health.observe(connected, frames_captured, health_flags, capture_generation)`; pending config activates on HEALTHY / CONTENT_SUSPECT (or DEGRADED with ACTIVE|PAUSED); STALLED/FAILED fails the apply; FAILED → `CAPTURE FAILED` keyed `capture-backend` with `RESTART ENGINE`; no event for 3 s → status `DISCONNECTED` (warning); process exit → `ENGINE STOPPED` keyed `engine-process-exit` (detail "The capture engine stopped after the capture failure above. Clips cannot be saved until it is restarted." when exit 0 after FAILED, else "The capture engine exited unexpectedly (code N). Clips cannot be saved until it is restarted."), tracker reset; `Restart` refused while recording (`STOP RECORDING FIRST`), deferred 500 ms while a save is busy, status `APPLYING SETTINGS` / `RESTARTING ENGINE`, stop (`Shutdown`, wait 2 s, SIGTERM + 1 s, SIGKILL + 0.5 s) then 1 s then start; 30 s apply timeout → `SETTINGS APPLY TIMED OUT` keyed `capture-apply-timeout`; recoverable incidents cleared through `RecoveryTracker` with the reference's `advance_recoverable_errors` rules.
- `services::status::{text_for(HealthState) -> &'static str}` = INITIALIZING `STARTING CAPTURE`, HEALTHY `CAPTURING`, DEGRADED `CAPTURE DEGRADED`, CONTENT_SUSPECT `CAPTURE DEGRADED — DARK / UNIFORM`, STALLED `CAPTURE STALLED`, FAILED `CAPTURE FAILED`, RECOVERING `RECOVERING CAPTURE`, STOPPED `CAPTURE STOPPED`; focus pause (DEGRADED with PAUSED) → `PAUSED — GAME UNFOCUSED` warning; styles: active for HEALTHY/DEGRADED/CONTENT_SUSPECT, idle for INITIALIZING/RECOVERING and while applying, warning otherwise; other strings `CONNECTING`, `ENGINE STOPPED`, `NO ENGINE`, `DISCONNECTED`, `MONITOR UNAVAILABLE`, `SETTINGS APPLY FAILED`.
- Settings page wiring: `state.restart_pending` (set by capture/audio/encoder changes in plan 4) → `Request::Restart` when the user presses Apply in the capture popup; `state.active_encoder` from `Event::EncoderActive`.

- [ ] Tests (a scripted `EngineClient` double fed from a `Vec<Event>` and a real spawn of `target/debug/fthrust-engine` with `FTHRUST_SYNTHETIC_BACKEND=1`): `status_table_matches_reference` (8 states + focus pause), `handshake_then_healthy_activates_config` (Hello, Configured, Status(ACTIVE, frames rising) → `CAPTURING`, tracker `Active`), `startup_error_maps_to_error_001_or_004`, `process_exit_pushes_engine_stopped_and_restart_recovers` (synthetic engine killed with SIGKILL → `ENGINE STOPPED` keyed `engine-process-exit`; `Restart` → new generation, incident removed after 3 progressing polls over 3 s), `restart_deferred_while_save_busy`, `apply_timeout_after_30s` (clock injected), `silence_3s_is_disconnected`.
- [ ] RED → implement → GREEN → commit `feat(ui): engine supervisor and status`.

---

### Task 10: Save-clip flow

**Files:** `crates/fthrust-ui/src/services/save.rs`, `src/popups/hotkeys.rs` (nothing visual), `src/shell/toolbar.rs`; tests `tests/services_save.rs`.

**Interfaces (Produces):**
- `services::save::{SaveService` (owns `SaveStateMachine::with_defaults()`, `last_request: Instant`, `pending_paths: HashMap<u64, PathBuf>`); `Request::SaveClip { seconds }` → refuse when recording state is preparing/starting/stopping (`SAVING` for 1 s, no error), engine disconnected → `CAPTURE UNAVAILABLE` (Error 002), `evaluate_save_admission(snapshot, seconds)` reject → card error + `CLIP NOT SAVED` with `RESTART CAPTURE`, 1 s spam guard and `is_busy()` → `SAVING` 1 s; path = `replay_clip_path(root, game, now)` + `unique_path` (`Desktop/desktop_clip_from_…` or `<game>/<game>_clip_from_…` with `target_window_name` sanitised when `capture_mode == "window"`), folder created (failure → `CLIP SAVE FAILED` "Cannot create <dir>: <e>"); `Command::SaveClip { request_id, path, seconds }`, `submit()`; immediate card `show_clip(seconds, fps, "1920×1080")`, status `SAVING`; engine events `SaveStarted` → ACCEPTED, `SaveResult(Ok)` → COMPLETED (file must exist and be non-empty, else `CLIP WAS NOT SAVED` "The capture engine reported success but no clip file was written…"), `SaveResult(Err)` → `CLIP WAS NOT SAVED` with `OPEN FOLDER`, status `SAVE FAILED` for 3 s; tick timeouts → `SAVE SLOW…` (warning) at 60 s notice; completion → `Event::ClipSaved { path, seconds, audio_tracks }`: readiness `engine_committed(path, false)` then `ready`, library `upsert`, sound `clip_captured` (the card already played), diagnostics `clip_save` events with the reference names (`save_requested`, `command_submitted`, `engine_accepted`, `save_slow`, `save_failed`, `mux_completed`)}`.
- Hotkeys: `save_clip` uses the active config's clip length (else `clip_length` setting); `save_custom_clip_<N>` uses N.

- [ ] Tests: `save_builds_reference_path_and_suffixes` (two saves in the same second → `_2`), `save_refused_when_disconnected_pushes_error_002`, `save_rejected_by_admission_when_stopped`, `spam_guard_within_1s_shows_saving_only`, `save_result_error_pushes_clip_was_not_saved_and_status_recovers` (Review Focus 1), `completed_save_upserts_library_and_marks_ready`, `slow_notice_at_60s_then_late_success_accepted`.
- [ ] RED → implement → GREEN → commit `feat(ui): save-clip flow`.

---

### Task 11: Manual recording

**Files:** `crates/fthrust-ui/src/services/recording.rs`, `src/shell/toolbar.rs` (Record button states), `src/state.rs`; tests `tests/services_recording.rs`.

**Interfaces (Produces):**
- `services::recording::{RecordState { Idle, Preparing, Starting, Recording { started: Instant }, Stopping, Finalizing }, RecordingService}`; `Request::ToggleRecording` / hotkeys `start_recording` / `stop_recording`. Start: refuse while a save is busy (`WAIT FOR CLIP SAVE`, warning) or disconnected (`RECORDING COULD NOT START` + `RESTART CAPTURE`); `recording_directory` created, free space via `nix::sys::statvfs` ≥ 512 MiB else `NOT ENOUGH DISK SPACE`; path `recording_path(dir, now)` with `_2`… suffixes, > 255 chars → `RECORDING PATH TOO LONG`; if the active config differs from the recording profile (`fps`, `width`, `height`, `bitrate_kbps` from the `recording_*` settings) → `Preparing` + restart with `allow_recording_prepare` and status `PREPARING RECORDING QUALITY`, then `Command::StartRecording`; `Starting` (status `STARTING RECORDING`, 10 s ack timeout → `MANUAL RECORDING FAILED` "The capture engine did not acknowledge the start request."); `RecordingStarted` → `Recording`; stop → `Command::StopRecording`, `Stopping` (status `FINALIZING RECORDING`); `RecordingStopped { path, bytes }` → `Finalizing`: `probe_video_metadata` on a blocking task, success → `Idle`, library refresh, status `RECORDING SAVED` (active, 2.5 s), card `show_recording_saved(name)`, capture profile restored; failure → `MANUAL RECORDING FAILED` with `OPEN RECORDINGS`, detail suffixed with "Recoverable video fragments remain in <name>." when the file exists.
- Record button (`toolbar::record_button`): enabled on Linux (ruling: the engine supports continuous recording); texts `Record`, `Preparing…`, `Starting…`, `Stop  hh:mm:ss` (two spaces, ticking each second), `Finalizing…`, `Recovering…`; inactive style `ACCENT` bg / `BG` text / `ACCENT` border, hover `ACCENT_SOFT` + `ACCENT` text; active style `DELETE` bg / `TEXT` / `ERROR` border, hover `ERROR_SOFT` + `ERROR`; display 20 px 600, r `RADIUS_MD`, padding 0×12; disabled while not Idle/Recording.

- [ ] Tests: `record_path_and_free_space_checks`, `start_with_different_profile_prepares_then_starts` (scripted engine), `ack_timeout_after_10s_fails_recording`, `stop_finalizes_and_shows_recording_saved` (synthetic engine writes a real fragmented MP4; probe succeeds), `record_button_texts_and_styles` (kittest pixel samples for both styles; `record-hover` fidelity with `enabled` ignored).
- [ ] RED → implement → GREEN → remove the `record_hover` ignore in `fidelity_all.rs` → commit `feat(ui): manual recording`.

---

### Task 12: Screenshots through the portal

**Files:** `crates/fthrust-ui/src/services/screenshot.rs`, `Cargo.toml` (ashpd `screenshot`); tests `tests/services_screenshot.rs`.

**Interfaces (Produces):**
- `services::screenshot::{ScreenshotService { inflight: bool, pending: u8 (≤ 9) }, Request::Screenshot}`: reserve paths in `<clips>/Screenshots`; `ashpd::desktop::screenshot::Screenshot::request().interactive(false).modal(false)` (ruling: non-interactive, the reference's interactive flag opened a chooser on some portals and the hotkey must not) with a 5 s method timeout and 60 s response timeout; response `uri` → local path (must be `file://`), `validate_png_bytes` limits, copy to `staged`, `publish_staged_png`; success → `Event::ScreenshotSaved(path)`: card `show_screenshot`, sound `screenshot_captured`, library refresh; failure → `SCREENSHOT FAILED` "<CODE>: <detail>" error + `DialogKind::Message { "Screenshot Failed", "<CODE>\n\n<detail>" }` (codes `PORTAL_UNAVAILABLE`, `CANCELLED` "Screenshot was canceled in the compositor dialog.", `PORTAL_FAILED`, `TIMEOUT` "Screenshot portal did not answer within 5 seconds." / "The compositor screenshot dialog timed out.", plus the staging codes); queued requests run one after another; shutdown cancels and unlinks the staged file.
- `PortalScreenshot` trait with the ashpd implementation and a test double returning a PNG path or an error.

- [ ] Tests: `screenshot_saves_png_and_refreshes_library` (double returns a 64×64 PNG → final name pattern, `.partial` gone, event emitted), `screenshot_rejects_oversized_png` (`INVALID_IMAGE`), `screenshot_queue_caps_at_ten`, `screenshot_cancel_shows_dialog_and_error`, `#[ignore] screenshot_live_portal` (this machine).
- [ ] RED → implement → GREEN → commit `feat(ui): screenshots via the Screenshot portal`.

---

### Task 13: Hotkey delivery — socket server, Hyprland config, GlobalShortcuts portal

**Files:** `crates/fthrust-ui/src/services/{hotkeys,portal_shortcuts}.rs`, `src/popups/hotkeys.rs` (portal note, copy-config-line button on Hyprland), `Cargo.toml` (ashpd `global_shortcuts`); tests `tests/services_hotkeys.rs`.

**Interfaces (Produces):**
- `services::hotkeys::{HotkeyServer::start(runtime_dir, dispatch: Outbox) -> Result<HotkeyServer, HotkeyError>`: prepares `<runtime>/hotkey.sock` (refuses symlinks, non-sockets, foreign uids; removes only a dead socket of ours; `RuntimeDirError` → `HOTKEY SERVER FAILED` "<e> Hotkeys will not work until this is resolved."), binds with umask 0o177 then `chmod 0600`, `listen(8)`; each client: one `read` of ≤ 256 bytes with a 500 ms timeout, trimmed body is the action id; known actions and `save_custom_clip_<N>` present in `hotkeys.json` dispatch `Event::Hotkey(HotkeyAction)`, anything else logs `Unknown command`; `HotkeyServer::path()`, `close()` unlinks}`.
- `services::hotkeys::apply_compositor_config(home, bindings, client_exe, sock, compositor)`: Hyprland only, calls `fthrust_core::hotkeys::hyprland::apply`; when changed and `HYPRLAND_INSTANCE_SIGNATURE` is set runs `hyprctl reload` (5 s timeout) on a blocking task, failure → `HOTKEY CONFIG ERROR` warning "Hyprland config reload failed. Re-apply manually in Hotkey settings."; runs at start and after every binding change (`state.hotkeys_changed`).
- `services::portal_shortcuts::{PortalShortcuts::start(app_id: "fthrust", bindings) -> Result<…>` (ashpd `GlobalShortcuts`: `create_session`, `bind_shortcuts` with `description` and `preferred_trigger = to_portal_trigger(combo)` for every action including empty combos; `Activated` → `Event::Hotkey` unless the action is in the cleared set or input capture is active; `ShortcutsChanged` → adopt differing triggers into `hotkeys.json`, `Event::DesktopBindingsChanged`, `DESKTOP KEPT ITS SHORTCUT` warning with `EDIT IN DESKTOP SETTINGS` → `configure_shortcuts`; bind cancelled → "Shortcut setup was cancelled in the desktop dialog."; session retried 3× with 1 s and 3 s pauses), skipped on Hyprland unless `FTHR_FORCE_PORTAL_SHORTCUTS=1`; `state.hotkeys_portal_active` reflects it; `open_desktop_shortcuts_requested` → `configure_shortcuts()`}`.
- Warnings when neither path works (not Hyprland, no portal): `GLOBAL HOTKEYS NEED MANUAL SETUP` with the per-desktop hint and the client command (the `nc` branch is gone).
- Hotkeys popup: on Hyprland each row gets a 26 px `copy` icon button that puts that row's `bind = …` line on the clipboard with the row feedback "Config line copied"; the popup fidelity test keeps `compositor = "unknown"`.

- [ ] Tests: `socket_server_dispatches_known_action_and_custom_clip` (temp runtime dir, a client writes `save_custom_clip_90`), `socket_server_drops_silent_and_junk_clients_within_500ms` (Review Focus 3: a silent client and a 300-byte client; a third client's `save_clip` still lands within 1 s), `socket_server_refuses_symlink_path`, `compositor_config_written_on_start_and_change` (`HOME` → temp; `hyprctl` replaced by a stub on `PATH` that records its argv), `portal_adopts_desktop_trigger_and_warns` (a `PortalShortcuts` double emitting `ShortcutsChanged { save_clip: "F10" }` → `hotkeys.json` has `F10` and the warning title), `copy_config_line_button_only_on_hyprland`.
- [ ] RED → implement → GREEN → commit `feat(ui): hotkey socket server, Hyprland config and GlobalShortcuts portal`.

---

### Task 14: Game detection and focus pause on Wayland

**Files:** `crates/fthrust-ui/src/services/{game,focus}.rs`, `src/popups/{source,game_detection}.rs` (running windows, unavailable state), `src/shell/toolbar.rs` (Source / Game labels); tests `tests/services_game.rs`.

**Interfaces (Produces):**
- `services::game::{GameDetector::start(watcher: Box<dyn Toplevels>, outbox)` where `trait Toplevels { fn available(&self) -> bool; fn snapshot(&self) -> Vec<Toplevel>; fn changes(&self) -> &Receiver<()>; }` (`ToplevelWatcher` implements it); `Event::RunningWindows(Vec<String>)` (`"<title> · <app_id>"`) refreshes the Source popup; a toplevel is a game candidate when `fullscreen` or a custom rule matches (`game_rules::matching_rule`), never when blacklisted; `GameCandidate { id, title, app_id }` events `Event::GameAppeared(candidate)`, `Event::GameClosed(id)`}`.
- `services::game::GameFlow` (in the UI thread, from `main.py`): disabled unless `game_detection_enabled`; `game_detection_mode == "prompt"` → card prompt `"<name> detected — [<confirm key>] Record  [<dismiss key>] Dismiss"` with a 15 s timeout (hotkeys `confirm_game_detection` / `dismiss_game_detection`), `auto` → switch; switching writes `capture_mode = "window"`, `target_window_name = <title>`, `target_app_id = <app_id>` (new key, ruling), Source label `<NAME>` upper-cased, card `show_capturing(name)`, engine restart; game closed with `game_detection_fallback_desktop` → desktop source, label `DESKTOP`, card prompt "Game closed — switched back to Desktop", restart; `Event::GameDetectionAvailability(bool)` → Game dropdown label `Game detection unavailable` and `LIMITED FEATURE SUPPORT` warning "Game detection and focus pause need a compositor that exposes its window list (wlr-foreign-toplevel-management). Hotkeys and capture are unaffected." (ruling: replaces the xdotool text).
- `services::focus::FocusMonitor` (from the same toplevel snapshot: the `activated` toplevel's title contains `target_window_name` case-insensitively; started only when `anticheat_detection_enabled` and `capture_mode == "window"`): lost → `Request::Pause` + status `PAUSED — GAME UNFOCUSED`, regained → `Request::Resume` + `STARTING CAPTURE`.

- [ ] Tests (a `FakeToplevels` double): `fullscreen_toplevel_is_candidate_rule_and_blacklist_applied`, `prompt_mode_shows_card_and_confirm_switches` (settings keys written exactly), `auto_mode_switches_and_restarts_engine`, `game_closed_falls_back_to_desktop_when_enabled`, `focus_lost_pauses_and_regained_resumes`, `unavailable_protocol_sets_label_and_warning`.
- [ ] RED → implement → GREEN → commit `feat(ui): game detection and focus pause on Wayland toplevels`.

---

### Task 15: Capture card overlay and sounds

**Files:** `crates/fthrust-ui/src/overlay/{mod,surface,capture_card}.rs`, `src/services/sounds.rs`, `Cargo.toml` (egui-wgpu, raw-window-handle, pipewire, ashpd `notification`); tests `tests/{overlay_card,sounds}.rs`.

**Interfaces (Produces):**
- `overlay::capture_card::{CardKind { Clip { seconds, fps, resolution }, Screenshot, Error { detail }, Upload { filename }, UploadFailed { detail }, RecordingSaved { filename }, Prompt { text }, Capturing { source }, Startup }, CardModel::show(kind, hold_ms: Option<u32>)` (full vs compact per the reference: Screenshot/Error/Prompt/Capturing/RecordingSaved are compact), `CardModel::frame(now) -> Option<CardFrame { x_offset, progress, shimmer, headline, detail, stats }>` (timeline: slide in 450 ms OutBack from `+w+10` to 0, hold, slide out 350 ms InCubic; shimmer `-1` until +150 ms then `elapsed/550·3` for 550 ms; progress `1 − hold_elapsed/hold`), `paint(painter, t: &Tokens, frame, scale)` (bg `CAPTURE_CARD_BG`, 3 px `CAPTURE_CARD_ACCENT` rail, shimmer gradient ±70 px at alpha 14, camera icon 26/20 px at (20,17)/(15,13), headline display 600 15/11 px capitalised and elided at (55,12,w−67,28)/(45,8,…,25), divider `CAPTURE_CARD_DIVIDER` at y 47 and stats columns (`usable = w − 23`, value 11 px 600, label 8 px upper-case `CAPTURE_CARD_STATS_DIM`, separators), compact detail 9 px at alpha 180 at y 33, 2 px progress track/fill at the bottom)}`; the stats for a clip are `(seconds "s", "LENGTH") (fps, "FPS") (resolution, "RESOLUTION")`.
- `overlay::surface::OverlaySurface::new(output_name: Option<&str>) -> Result<OverlaySurface, OverlayError>`: picks the named output else the highest-refresh output, computes `scale = clamp(h/1080, 0.65, 1.0)`, creates the layer surface (Task 4) sized `(w·scale, h·scale)` with margins (16 top, 16 right) and a wgpu surface from its raw handles, an `egui::Context` + `egui_wgpu::Renderer`; `render(&mut self, |painter|)` once per animation frame on a dedicated thread at 60 Hz while a card is live; `hide()` destroys the surface.
- `services::sounds::{SoundPlayer::new()` (a PipeWire playback stream 48 kHz stereo f32 on the services runtime; preloads the six cues through `audio_decode::decode_to_pcm`, theme custom sounds first), `play(key, volume)` (`volume · sound_volume_master / 100`, `notification_sounds_enabled` gate; a new cue stops the previous one), `preview(key)` for the Customize page (`sound_preview_requested`)}`.
- Fallback: when `layer_shell::available` is false, `ashpd::desktop::notification::Notification` with the headline and detail is sent instead and a `warn!` is logged once; `capture_card_enabled` false → sounds only.
- Startup: `Startup` card + `startup` sound once the engine handshake completes.

- [ ] Tests: `card_timeline_values` (at 0 ms off-screen, at 450 ms x = 0, at 2850 ms x = 0, at 3200 ms off-screen, shimmer −1 at 100 ms and 1.5 at 425 ms, progress 0.5 at 450 + 1200 ms), `card_paint_matches_reference_layout` (kittest render of a full card at scale 1: rail 3 px `CAPTURE_CARD_ACCENT`, divider at y 47, headline baseline and stats x positions 13, 119, 225 for three columns), `fallback_output_when_named_output_missing` (Review Focus 4: a fake output list without `DP-9` → the 165 Hz output), `sounds_master_volume_and_gate` (`volume 80`, master 50 → 40; gate off → not played), `#[ignore] overlay_live_shows_card` (this machine).
- [ ] RED → implement → GREEN → commit `feat(ui): capture card overlay and sounds`.

---

### Task 16: Shell wiring, error-bar actions, docs, gate

**Files:** `crates/fthrust-ui/src/shell/{mod,toolbar,error_bar}.rs`, `src/lib.rs`, `src/settings/{clip,audio}.rs` (apply → restart), `tests/fidelity_all.rs`, `docs/capture-and-library.md`, `CHANGES.md`, `README.md`, roadmap row 5.

- Error-bar action ids and what they do: `restart` → `Request::Restart`, `restart-capture` → same, `open-folder` → `xdg-open <clips root>`, `open-recordings` → `xdg-open <recording dir>`, `open-settings` → Settings page, `edit-game-crop` → Game detection popup, `open-audio-settings` → Settings/Audio, `configure-shortcuts` → portal `configure_shortcuts`, `learn-more` → opens the docs page.
- `App::ui` order: `services.tick(state)` → `shell::ui` → `dialogs` (already) → `overlay` frame request; `App::on_exit` runs the reference's shutdown order: cancel save, stop recording (finalise the file without UI), stop screenshot jobs, stop game detection, close hotkey server and portal, stop sounds, close the card, stop the engine, release the instance lock, flush diagnostics.
- `docs/capture-and-library.md`: how the engine is found and supervised, the status table, hotkey delivery on Hyprland and portal desktops, screenshot and recording paths, the capture card and its fallback, how to run the ignored live tests.
- `CHANGES.md` "Fixed": screenshots use the Screenshot portal (no more black images); game detection runs on Wayland through the compositor's toplevel list; manual recording works on Linux; hotkey binds no longer depend on `nc`. "Added": the library and capture UI.

- [ ] `fidelity_all` runs every screen without ignores except `clip-editor` (plan 6); full gate (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check`, `tools/check-assets.sh`); the app is started by hand on this machine once (`cargo run -p fthrust-ui --release`): the engine comes up to `CAPTURING`, F9 saves a clip that appears in the library with a thumbnail, the card slides in, F12 saves a screenshot, Record writes a recording. Findings go into the ledger as rulings or fixes before the final review.
- [ ] Commit `chore(ui): capture and library wiring, docs, roadmap`.

---

## Self-review notes

- Spec 5.2 "drag to share window" and the share button's upload path depend on the uploader (plan 7); the card exposes `upload_requested`, and drag-out of a file from a winit window is not available on Wayland (ruling to be recorded in the ledger at Task 7).
- Spec 6.4 names `ext-foreign-toplevel-list`; this plan uses `wlr-foreign-toplevel-management` because it carries fullscreen and activated state (needed for detection and focus pause) and Hyprland implements it; `ext-foreign-toplevel-list` has no state. Recorded as a ruling at Task 4.
- Names used across tasks: `services::{Event, Request, Outbox, Services::tick}` (Task 8) are consumed by Tasks 5, 9–15; `library::Library` (Task 5) by 6, 7, 10; `ClipReadinessRegistry` (Task 1) by 5, 7, 10; `LayerSurface` (Task 4) by 15; `ToplevelWatcher` (Task 4) by 14; `hyprland::apply` (Task 2) by 13.
