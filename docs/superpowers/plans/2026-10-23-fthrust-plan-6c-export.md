# FTHRust Plan 6c: Export and Share Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Export and share an edited clip the way the reference does: the export queue with its jobs window, the share picker and share window with size presets, the watermark, and the live crop/stretch/colour preview on the editor's video surface.

**Architecture:** `fthrust-core::export` builds everything that is a string or a number: the reference's filter chains and filter graphs (verbatim), the audio mix graph, the watermark snippets, output names, expected durations and the size-preset plan. `fthrust-media::export` runs a graph in-process: it demuxes the clip, decodes the inputs the graph names, feeds `buffer`/`abuffer` sources, pulls the sink pads, encodes and muxes into a staged file, validates it and commits it. There is no FFmpeg command-line binary (spec section 4: FFmpeg is the dynamically loaded LGPL library build). `fthrust-ui::services::export` is the queue (FIFO admission, parallel limit, statuses, cancel, shutdown). The editor, the Export jobs window, the share dialogs and the preview are UI on top.

**Tech Stack:** Rust 2024, `ffmpeg-next` 9 (avformat, avcodec, avfilter `Graph`), the existing `encoder` candidates, egui 0.37 and egui-wgpu (a paint callback with a WGSL port of `color.frag`).

**Spec:** `docs/superpowers/specs/2026-10-22-fthrust-design.md` (5.7 "Share clip, export preset selector, Export with a jobs list, transactional export to `<root>/Exported`", 5.8 watermark, 6.4 playback/preview, 7 `export_presets.json` and the clips-root layout). Reference digest: `docs/superpowers/notes-export-reference.md` (section numbers below are its sections); the editor digest `docs/superpowers/notes-editor-reference.md`.

## Global Constraints

- No FFmpeg CLI: every encode, filter and probe runs in-process through `ffmpeg-next`; the UI thread never decodes, filters or encodes.
- Filter strings are the reference's verbatim (digest 10, 10a, 10b, 14): number formats (`{:.6f}`, `{:.3f}`, `{:.2f}`, `{:.4f}`), label names (`[v{i}]`, `[a{i}]`, `[amixed]`, `[vbase]`, `[vout]`, `[aout]`, `[aspeed]`, `[fthr_watermark…]`) and the order of the chain.
- Quality options: Very fast `fast`, High quality `quality`, Lossless · MKV `lossless` (MKV, FFV1 `-level 3 -coder 1 -context 1`, FLAC). Other qualities write MP4 with H.264 and AAC at 320k (editor) or the preset's rate (share). Encoders: the existing candidates for H.264 (NVENC, VAAPI, then software), with the reference's quality targets (hardware CQ/QP 23 fast, 18 quality; software libx264 `superfast` CRF 18 or the bitrate the reference uses for libopenh264: 16000k fast, 32000k quality); GOP 30, no B-frames, BT.709 limited range. A failed hardware attempt retries once in software with status `Software fallback · software`.
- Queue: default one export at a time; `parallel_exports` true allows `parallel_export_limit` clamped 2–4; strict FIFO admission across exports and shares; the newest 50 records kept; inactivity timeout 120 s; cancel wait 2 s; shutdown joins for at most 12 s.
- Status strings: `Queued`, `Preparing`, `Exporting · NVIDIA hardware` / `Exporting · AMD hardware` / `Exporting · Intel hardware` / `Exporting · lossless` / `Exporting · software` / `Exporting · stream copy`, `Software fallback · software`, `Cancelling`, terminal `Completed` / `Failed` / `Cancelled` / `Timed_Out` (the reference's enum title-casing, kept for parity).
- Progress: output time over the speed-adjusted expected duration `max(0.1, kept_seconds / speed)`, capped at 99 % while running, 100 only after completion.
- Output: editor `{clips root}/Exported/{stem}_export_{12 hex}.mp4|.mkv`; share `{clips root}/Shared/{stem}_share_{HH-MM-SS}_{8 hex}.mp4` (`_discord_` for the Discord preset), `_2`, `_3`… on collision. Staged as `.{stem}.partial.{32 hex}{ext}` in the same folder, validated (a video stream and a decodable first frame, 15 s bound), then renamed; a staged file never survives a cancel, a failure or app exit.
- Strings: every user-visible string is the reference's (digest 9, 12, 17), except the rulings recorded in `CHANGES.md` (Task 8).
- Exported and Shared stay out of the library grid (the plan 5 scan already excludes them).
- Fidelity: every new screen has a Qt render before it is built (Task 1) and an `assert_fidelity!` test.
- Out of scope: webcam picture-in-picture and Gary mode are capture-side (digest 15) and move to a new plan 6d; uploading is plan 7.

## Review Focus

1. The source clip is deleted or renamed while its export runs: the export fails with a readable status (or finishes from the open file), the staged file is removed, nothing half-written appears in `Exported`. Test in Task 3.
2. Quitting during an export: the job is cancelled within the 12 s shutdown bound and no `.partial` file is left. Test in Task 4.
3. A clip with no audio, a manifest track that does not decode, or every source muted: the export still produces a playable file (silent or without an audio stream as the graph dictates) instead of failing. Test in Task 3.
4. The disk fills or the folder is read-only mid-export: `Failed` with the error in the row tooltip, staged file removed, the queue moves on. Test in Task 3.
5. A share size preset the clip cannot meet even at the lowest ladder rung: the plan still produces a file (the reference's floors of 64/120 kbps) and the validator's `Output is … MB, above the … MB target` is the visible failure, not a crash. Test in Task 2 (plan maths) and Task 6 (UI path).

---

## File structure

```
tools/render-qt-reference.py                     + export-jobs, export-jobs-empty, share-picker, share-window-{exporting,ready,failed}
crates/fthrust-core/src/export/mod.rs            re-exports, QUALITY_OPTIONS, statuses
crates/fthrust-core/src/export/filters.rs        video chain, atempo/pitch chains, speed text, mix graph
crates/fthrust-core/src/export/graph.rs          ExportSpec → GraphPlan (concat / single range / stream copy)
crates/fthrust-core/src/export/watermark.rs      the four watermark snippets
crates/fthrust-core/src/export/naming.rs         export/share names, staged names, durations
crates/fthrust-core/src/export/plan.rs           size-preset planning and its summary
crates/fthrust-core/tests/export_model.rs
crates/fthrust-media/src/export/mod.rs           run(GraphPlan) in-process: demux, decode, graph, encode, mux
crates/fthrust-media/src/export/encode.rs        encoder choice per quality, software fallback
crates/fthrust-media/src/export/validate.rs      staged validation and commit
crates/fthrust-media/tests/export_run.rs
crates/fthrust-ui/src/services/export.rs         the queue
crates/fthrust-ui/src/editor/export_ui.rs        Export button flow, guards, jobs label
crates/fthrust-ui/src/dialogs/export_jobs.rs     Export jobs window
crates/fthrust-ui/src/editor/share.rs            share picker and share window
crates/fthrust-ui/src/editor/preview.rs          live crop/stretch/colour preview
crates/fthrust-ui/src/editor/preview.wgsl        port of ui/qml/color.frag
crates/fthrust-ui/tests/export_queue.rs
crates/fthrust-ui/tests/export_ui.rs
tools/uidrive/scenarios/export.sh
docs/export.md
```

---

### Task 1: Qt reference renders for the export screens

**Files:**
- Modify: `tools/render-qt-reference.py`, `tools/test_render_qt_reference.py`
- Create: `docs/reference/qt-screens/{export-jobs,export-jobs-empty,share-picker,share-window-exporting,share-window-ready,share-window-failed}.{png,json}`

**Interfaces:** Produces the six screens later tasks test against. `export-jobs` shows two fake `ExportTask`s (one `Exporting · software` at 42 %, one `Completed`) in `ExportJobsDialog` at its default 720×400; `export-jobs-empty` the empty state; `share-picker` the `ShareModeDialog` (330×350) with one custom preset; the share window (340×242) in its three states, driven by the window's own status slots with a stubbed export thread.

- [ ] Step 1: Add assertions to `tools/test_render_qt_reference.py`: each JSON exists; `export-jobs` contains the texts "Cancel selected", "Open output folder" and a row text containing "Exporting · software"; `export-jobs-empty` contains "No export jobs yet. Start an export from a clip."; `share-picker` contains "Share as" and "10 MB  ·  Discord"; the share-window screens contain "SHARE CLIP" and respectively "Exporting...", "Ready", "Failed".
- [ ] Step 2: Run `.venv-reference/bin/pytest -q tools/test_render_qt_reference.py` → FAIL (files missing).
- [ ] Step 3: Extend the script (patch the export thread and ffprobe as the editor render patches its media; never write into `reference/`); render; revert any unrelated screens that change with the host (as in plan 6b).
- [ ] Step 4: Run the pytest → PASS; look at each PNG.
- [ ] Step 5: Commit `docs(reference): Qt renders for export jobs and share`.

---

### Task 2: Export model in `fthrust-core`

**Files:**
- Create: `crates/fthrust-core/src/export/{mod,filters,graph,watermark,naming,plan}.rs`, `crates/fthrust-core/tests/export_model.rs`
- Modify: `crates/fthrust-core/src/lib.rs`

**Interfaces (Produces):**
- `filters::speed_text(f64) -> String`; `atempo_chain(speed) -> String`; `pitch_changed_chain(speed) -> String`; `video_filter_chain(crop, &Effects, stretch, ensure_even: bool, speed) -> Vec<String>` (digest 10 items 1–8); `MixSource { audio_index: Option<u32>, available: bool, role: MixRole { Base, Delta, Direct }, gain_percent: u8, muted: bool }`; `mix_filter(&[MixSource], master_percent: u8, label: &str) -> (Vec<String>, Option<String>)` (digest 10 audio mix).
- `graph::ExportSpec { ranges_s: Vec<(f64, f64)>, crop, effects, stretch, speed, preserve_pitch, sources: Vec<MixSource>, master_percent, has_manifest_tracks: bool, watermark: Option<WatermarkStyle>, audio_bitrate_kbps: u32, extra_video_filters: Vec<String>, force_video_encode, force_audio_encode }` → `graph::plan(&ExportSpec) -> Result<GraphPlan, String>` where `GraphPlan { input_seek_s: Option<f64>, input_duration_s: Option<f64>, filter_graph: Option<String>, video_map: MapRef, audio_map: Option<MapRef>, encode_video: bool, encode_audio: bool, audio_filter: Option<String>, shortest: bool }`, `MapRef { Label(String), Stream { video: bool, index: u32 } }` — the 10a/10b/10c rules, including "segments shorter than 1 ms dropped" and the error `at least one non-empty segment is required`.
- `watermark::WatermarkStyle { bg, accent, text, divider, stats_dim: String /* 0xRRGGBB */, font_path, icon_path }`; `watermark::filters(base, output) -> [String; 4]` (digest 14 verbatim with path escaping).
- `naming::{export_name(stem, hex12, lossless) -> String, share_name(stem, hms, hex8, discord, lossless) -> String, staged_name(final, hex32) -> PathBuf, unique(path) -> PathBuf /* _2, _3 */, speed_adjusted_duration(kept_s, speed) -> f64}`.
- `plan::{SizePreset { name, target_size_mb, target_fps, width: Option<u32>, height: Option<u32>, audio_kbps, safety_margin }, MediaFacts { duration_s, width, height, fps, size_bytes, video_codec, audio_codec, audio_kbps }, plan_export(&SizePreset, &MediaFacts) -> ExportPlan { video_kbps, audio_kbps, fps: Option<f64>, scale: Option<(u32,u32)>, reencode_video, reencode_audio, summary: String }, discord_preset() -> SizePreset /* 10 MB on Linux */}`.
- `mod.rs`: `QUALITY_OPTIONS: [(&str, &str); 3]` (moved here from the editor sidebar, which re-exports it).

- [ ] Step 1: Write the failing tests, each comparing exact strings taken from the reference (run the Python helpers in `.venv-reference` once to capture expected values into the test as literals; note the command in a comment):
  - `speed_and_pitch_chains`: `atempo_chain(0.25) == "atempo=0.5,atempo=0.5"`, `atempo_chain(3.0)` per digest (stages of 2 then the rest), `atempo_chain(1.0) == ""`, `pitch_changed_chain(1.5) == "aresample=48000,asetrate=72000,aresample=48000"`, `speed_text(1.25) == "1.25"`, `speed_text(2.0) == "2"`.
  - `video_chain_order_and_format`: crop (odd w/h forced even), curves, hue, colorbalance, unsharp, vignette, stretch scale+setsar, setpts — each value formatted as the reference's (digest 10).
  - `mix_graph`: two direct sources (one muted) and master 80 → the exact snippets and output label; no available source → `([], None)`.
  - `concat_graph_two_ranges`: the full `-filter_complex` string for two ranges with a mix, speed 1.5, preserve pitch, no watermark, and its maps (`[vout]`, `[aspeed]`), `shortest == true`.
  - `single_range_stream_copy`: one range, no filters, no mix → `filter_graph == None`, `encode_video == false`, `encode_audio == false`, seek and duration set.
  - `single_range_filtered`: one range with crop and speed 2 → the graph and `audio_filter == Some("atempo=2")` when there is no mix.
  - `empty_edit_is_an_error`: ranges all under 1 ms → `Err("at least one non-empty segment is required")`.
  - `watermark_snippets`: the four snippets for a given style, including the `x_expr` and the path escaping of `C:\a:b'c` → `C/a\:b\'c` (digest 14).
  - `names`: `export_name("clip", "0123456789ab", false) == "clip_export_0123456789ab.mp4"`; lossless `.mkv`; `share_name` with `discord`; `unique` appends `_2`, `_3`; staged name format.
  - `plan_maths`: a 30 s 1080p60 clip at 25 Mb/s with the Discord 10 MB preset → the reference's `video_kbps`, scale rung and summary string; a clip that already fits → summary "Original already fits; no re-encoding needed."; an impossible target → floors 64/120 kbps applied.
- [ ] Step 2: Run `cargo test -p fthrust-core --test export_model` → FAIL (no module).
- [ ] Step 3: Implement the modules.
- [ ] Step 4: Run → PASS; clippy clean.
- [ ] Step 5: Commit `feat(core): export filter graphs, names and size-preset planning`.

---

### Task 3: In-process export runner in `fthrust-media`

**Files:**
- Create: `crates/fthrust-media/src/export/{mod,encode,validate}.rs`, `crates/fthrust-media/tests/export_run.rs`
- Modify: `crates/fthrust-media/src/lib.rs`

**Interfaces:**
- Consumes: `GraphPlan`, the encoder candidates (`encoder_policy`, `capabilities`), `tests/support::write_test_clip`.
- Produces: `export::Quality { Fast, Quality, Lossless }`; `export::Job { source: PathBuf, output: PathBuf, plan: GraphPlan, quality: Quality, video_bitrate_kbps: Option<u32>, max_rate_kbps: Option<u32>, expected_duration_s: f64, target_size_mb: Option<f64> }`; `export::run(&Job, cancel: &AtomicBool, on_status: &mut dyn FnMut(&str), on_progress: &mut dyn FnMut(f64)) -> ExportResult { state: ExportState { Completed, Failed, Cancelled, TimedOut }, detail: String }`. It writes the staged file, runs the graph (inputs `[0:v]`, `[0:a:N]` bound to `buffer`/`abuffer` sources created from the decoders' parameters; outputs to `buffersink`/`abuffersink`), encodes, muxes, validates (`validate::check(path, cancel)`: a video stream and one decoded frame, 15 s bound) and renames. Stream copy when the plan says so (packets remuxed with the seek and duration). The inactivity clock (120 s) restarts on every output frame; a hardware encoder failure retries once in software (status `Software fallback · software`); any end other than `Completed` removes the staged file.
- `encode::choose(quality, bitrate) -> Vec<EncoderChoice>` — first the hardware candidates the capability cache reports for H.264 (NVENC → "Exporting · NVIDIA hardware", VAAPI on AMD/Intel → "Exporting · AMD hardware"/"Exporting · Intel hardware" by the device vendor), then software; lossless → FFV1 only.

- [ ] Step 1: Write the failing tests (4 s synthetic clips with two audio tracks):
  - `export_two_ranges_with_mix_and_speed`: ranges 0–1 s and 2–3 s, speed 2, a mix → output exists, duration ≈ 1.0 s (±0.1), one video and one audio stream, staged file gone, progress reached ≥ 99 then 100 via the caller.
  - `stream_copy_single_range`: one range, no filters → status "Exporting · stream copy", duration ≈ the range, video codec unchanged.
  - `lossless_writes_mkv_ffv1_flac`.
  - `cancel_removes_staged`: cancel after the first progress callback → `Cancelled`, no file in the folder.
  - `missing_source_fails_cleanly`: source deleted before the run → `Failed` with a readable detail, no files left (Review Focus 1).
  - `read_only_folder_fails_cleanly`: output dir made read-only → `Failed`, detail mentions permission (Review Focus 4).
  - `no_audio_and_all_muted`: a video-only clip, and a clip with every source muted → both `Completed` and playable (Review Focus 3).
  - `hardware_failure_falls_back`: an injected failing first encoder (test hook) → status sequence includes `Software fallback · software`, `Completed`.
- [ ] Step 2: Run → FAIL.
- [ ] Step 3: Implement.
- [ ] Step 4: Run → PASS; clippy clean.
- [ ] Step 5: Commit `feat(media): in-process export runner with validation and software fallback`.

---

### Task 4: Export queue

**Files:**
- Create: `crates/fthrust-ui/src/services/export.rs`, `crates/fthrust-ui/tests/export_queue.rs`
- Modify: `crates/fthrust-ui/src/services/mod.rs` (own the queue; `prepare_exit` calls `shutdown`), `crates/fthrust-ui/src/state.rs`

**Interfaces:**
- Produces: `ExportQueue::configure(&SettingsStore)`; `submit(source, dir, quality, job_builder: Box<dyn FnOnce(&Path /*staged*/) -> Job + Send>, expected_s) -> TaskId`; `run_share(...)` (same slots, not listed); `tasks() -> Vec<TaskView { id, source, output, quality, status, progress, detail, done }>`; `cancel(id)` (sets the status to `Cancelling`); `counts() -> (pending, failed, completed)`; `shutdown(timeout)`; `active_for(source) -> bool`. Workers are threads; the UI reads snapshots.

- [ ] Step 1: Failing tests: `fifo_one_at_a_time` (three jobs with a fake runner: second starts only after the first ends), `parallel_limit_clamped` (setting 9 → 4; off → 1), `records_pruned_to_newest_50_plus_running`, `cancel_marks_cancelling_then_cancelled`, `shutdown_cancels_within_bound` (a blocked fake runner observes cancel; returns before 12 s; Review Focus 2), `counts_match_reference_label_rules`.
- [ ] Step 2: Run → FAIL. Step 3: Implement. Step 4: PASS; clippy.
- [ ] Step 5: Commit `feat(ui): export queue`.

---

### Task 5: Editor export, jobs window and guards

**Files:**
- Create: `crates/fthrust-ui/src/editor/export_ui.rs`, `crates/fthrust-ui/src/dialogs/export_jobs.rs`, `crates/fthrust-ui/tests/export_ui.rs`
- Modify: `editor/sidebar.rs` (Export button, `Export jobs · n pending|failed|completed` label refreshed every second), `editor/keys.rs` (Ctrl+E submits), `editor/sessions.rs` (delete/park/navigate guards), `settings/performance.rs` ("View export jobs" opens the window), `fidelity_all.rs`

**Interfaces:** `export_ui::export_now(state)` builds the `ExportSpec` from the editor (kept ranges in seconds, crop, effects, stretch, speed, pitch, the mix rows, master volume, watermark from settings and the theme's capture-card colours) and submits it; an empty edit raises the error-bar `EXPORT FAILED` / `The edit has no kept segments.`. Guards: delete → "Export in progress" / "Finish or cancel this clip’s exports before deleting the source." (curly apostrophe); park → "Export in progress" / "Finish or cancel the export before minimizing this editor."; navigate → "… before switching clips.". The jobs window is a non-modal in-window panel titled "Exports" (720×400) with the hint, the empty state, rows (filename, `{quality label} · {status}`, percent, 110×8 bar, tooltip = detail or output), "Cancel selected" and "Open output folder" (xdg-open of the folder through the portal OpenURI, else `xdg-open`).

- [ ] Step 1: Failing tests: fidelity `export_jobs` and `export_jobs_empty`; `export_button_submits_kept_ranges` (a split and a deleted middle → the submitted spec has two ranges); `empty_edit_shows_error`; `jobs_label_counts`; `delete_park_navigate_blocked_while_exporting`; `ctrl_e_submits`.
- [ ] Step 2: FAIL. Step 3: Implement. Step 4: PASS.
- [ ] Step 5: Commit `feat(ui): editor export with the jobs window and export guards`.

---

### Task 6: Share picker and share window

**Files:**
- Create: `crates/fthrust-ui/src/editor/share.rs`
- Modify: `editor/sidebar.rs` (Share opens the picker), `crates/fthrust-ui/tests/export_ui.rs`, `fidelity_all.rs`

**Interfaces:** The picker (330×350, modal over the editor): quality combo ("Share quality"), the hint, "Share clip", "10 MB  ·  Discord", custom presets from `ExportPresetStore` with "Use"; Lossless with a size preset → the message "Choose a quality" / "Lossless cannot meet a fixed file size. Choose Very fast or High quality for this preset."; choices persist `export_quality` / `selected_export_preset`. Share windows (340×242 each, stacked from the top-right of the editor, non-modal, any number) run through `ExportQueue::run_share`: title "SHARE CLIP" or `SHARE  ·  {PRESET}`, badge "Exporting..." / "Ready" / "Failed", a poster frame cropped and stretched like the edit, the filename bar ("Preparing...", the plan summary, the filename, or the error). Instead of the drag zone (winit cannot start a file drag on Wayland), the ready window offers "Copy path" and "Open folder" (ruling recorded in `CHANGES.md`). Closing cancels and removes the staged file.

- [ ] Step 1: Failing tests: fidelity `share_picker`, `share_window_{exporting,ready,failed}`; `lossless_with_preset_is_refused`; `share_writes_into_shared_with_name_format`; `preset_size_overrun_fails_visibly` (Review Focus 5); `closing_share_cancels`.
- [ ] Step 2: FAIL. Step 3: Implement. Step 4: PASS.
- [ ] Step 5: Commit `feat(ui): share picker and share windows`.

---

### Task 7: Live crop, stretch and colour preview

**Files:**
- Create: `crates/fthrust-ui/src/editor/preview.rs`, `crates/fthrust-ui/src/editor/preview.wgsl`
- Modify: `editor/layer.rs` (video surface uses the preview), `crates/fthrust-ui/tests/video_texture.rs` or a new `tests/preview.rs`

**Interfaces:** `preview::Uniforms::from_edit(crop, src_size, &Effects, stretch, color_enabled) -> Uniforms` (digest 16 mapping: exposure ×0.003, contrast 1+×0.006, saturation 1+×0.01, temperature ×0.003, sharpness ×0.015, vignette ×1.8/255; crop area normalised); an `egui_wgpu::CallbackTrait` drawing the video texture through the shader when any effect is non-zero, else plain; display rect `(src_w × stretch) / src_h` fitted into the surface; crop veil (BG at 0.32), 3 px accent border and the `CROP  ·  {w} × {h}` pill (22 px, 12 px bold accent, 6 px from the crop corner, min y −28). Gates: `clip_editor_live_preview` off → original video; `editor_color_preview_enabled` off → no colour effects, crop and stretch still shown.

- [ ] Step 1: Failing tests: `uniform_mapping_matches_reference` (pure); `display_rect_follows_stretch`; `crop_overlay_geometry` (veil rects, border, pill position for a crop near the top edge); `shader_output_matches_cpu_formula` (headless wgpu: render a 4×4 test texture with known effects, read back, compare with the CPU formula of digest 16 within 2/255; skip without an adapter); `gates_disable_preview`.
- [ ] Step 2: FAIL. Step 3: Implement (WGSL port of `color.frag`: grade, saturation with Rec.709 luma, temperature, 5×5 binomial sharpen, vignette inside the crop). Step 4: PASS.
- [ ] Step 5: Commit `feat(ui): live crop, stretch and colour preview`.

---

### Task 8: Bench scenario, docs and roadmap

**Files:**
- Create: `tools/uidrive/scenarios/export.sh`, `docs/export.md`
- Modify: `CHANGES.md`, `docs/editor.md` (6c section becomes done), `docs/superpowers/plans/2026-10-22-fthrust-roadmap.md` (6c done; new row 6d "Capture overlays: webcam picture-in-picture with the camera overlay editor, Gary mode layer-shell overlay, compositing into saved clips"), `.github/workflows/ci.yml` (add `export.sh` to the uidrive job)

- [ ] Step 1: Write `export.sh`: save a clip, open it, split and delete a middle segment, press Ctrl+E, open the jobs window, wait for "Completed", check a file exists under `$BENCH/clips/Exported/` with no `.partial` files; then Share → "Share clip", wait for "Ready".
- [ ] Step 2: Run it → FAIL until wired (it is written against Tasks 5–6); then PASS.
- [ ] Step 3: Docs: `docs/export.md` (pipeline, graph rules, queue, share, preview, differences); `CHANGES.md` rulings (no CLI; Linux encoders; share drag replaced; reference quirks fixed: the editor shows completion through the jobs label as the reference does — keep; the `FFMPEG NOT FOUND` dropped title cannot occur).
- [ ] Step 4: Full gate: fmt, clippy, `cargo test --workspace`, `cargo deny check`, every bench scenario.
- [ ] Step 5: Commit `feat: export bench scenario; docs: export`.
