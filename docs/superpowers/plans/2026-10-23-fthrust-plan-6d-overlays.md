# FTHRust Plan 6d: Capture Overlays Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Burn the webcam picture-in-picture and the image layers into saved clips after each save, and show the Gary mode overlay while the microphone is loud, the way the reference does, in one re-encode instead of the reference's chain.

**Architecture:** `fthrust-media::camera` records the selected V4L2 camera through libavdevice into a 90 s ring of MJPEG packets with capture timestamps, lists devices with their V4L2 names, and writes the clip's window as an MJPEG Matroska file with real timestamps. `fthrust-core::finalize` turns the settings (image layers, camera rect) and the clip's size into one filter graph using `movie=` sources, so the plan 6c runner can composite everything into a staged file with audio copied; the UI's finalize service runs it after the engine commits the clip, holds the clip in the existing `Finalizing` readiness state, renames the result over the clip and rebinds its audio manifest. A UI-side PipeWire capture stream measures the microphone level for the Audio meter and Gary; Gary draws on a layer-shell overlay surface like the capture card.

**Tech Stack:** Rust 2024, `ffmpeg-next` 9 (avdevice v4l2 and lavfi inputs, avformat Matroska muxing, the plan 6c export runner), `pipewire` 0.10, the existing layer-shell overlay in `fthrust-ui::overlay`, egui 0.37.

**Spec:** `docs/superpowers/specs/2026-10-22-fthrust-design.md` (5.8 "webcam picture-in-picture with the camera overlay editor, Gary mode overlay. These composite into the saved or exported clip"; out-of-window surfaces "capture card and Gary overlay use layer-shell"). Reference digest: `docs/superpowers/notes-overlays-reference.md` (section numbers below are its sections).

## Global Constraints

- No FFmpeg CLI and no new crates: camera capture uses libavdevice's `v4l2` input (ruling against the spec's `nokhwa`: no extra dependency or bindgen, real device names from `avdevice_list_input_sources`, MJPEG packets stored as delivered with timestamps); tests use the `lavfi` input (`testsrc`) as a synthetic camera.
- Settings keys and defaults are the reference's (digest 2, 17): `camera_enabled` false, `camera_device_index` 0, `camera_overlay_rect` {0.72, 0.64, 0.25, 0.34}, `image_overlay_enabled` false, `image_overlays` [], `gary_mode_enabled` false, `gary_min_level` 55, `gary_max_level` 85, `gary_image_path` null. The device index is the position in the listed devices.
- Camera ring: 90 s by capture timestamp; MJPEG requested (`input_format=mjpeg`), raw formats encoded to MJPEG at quality 85 equivalent; nothing is written to disk until a save.
- Geometry (digest 8, 17): rect clamp sizes 0.08–0.92 then positions, 4 decimals; pixels `max(64, int(W*w)) & ~1` × `max(48, int(H*h)) & ~1` at `int(W*x)`, `int(H*y)`; camera letterboxed in black (`scale…decrease,pad…`), images `fit` (transparent pad) or `fill` (centre crop), opacity `clamp(n/100, 0.10, 1.0)` as `colorchannelmixer=aa={:.3f}`; z-order image layers in list order, camera on top.
- One pass per clip: images and camera in one graph, one video re-encode with the export runner's encoder choice at the clip's bitrate class (High quality), every audio stream and the container/stream metadata copied, staged in the clip's folder as `.fthr-finalize-*`, renamed over the clip, manifest rebound once. Base clip kept on every failure.
- Camera timing: frames placed by their capture timestamps relative to the save instant (`clip start = save instant − duration`), not at a fixed 30 fps (fixes digest 8 quirks 1–3); the last frame holds if the camera stream ends early.
- Strings are the reference's: `CAMERA UNAVAILABLE` + digest 5 messages with action `OPEN CAMERA SETTINGS`; `CAMERA OVERLAY FAILED` / `FFmpeg error. Clip saved without camera overlay.`; warnings `Camera overlay was requested but the camera was unavailable.`, `Camera overlay could not be rendered; the base clip was retained.`, `Image overlay source is missing: {name}`, `Image overlay failed; the base clip was retained.`; `CLIP SAVED WITH WARNING` (last warning); `CLIP STILL FINALIZING` / `Playback, editing and upload become available after optional processing has finished.`
- Gary (digest 9–11): level `clamp(rms*4)` of channel 0 after `mic_volume` gain, 0 when older than 0.35 s; intensity linear between thresholds; 33 ms tick, rise 0.18, fall 0.065; picture `0.55 + 0.45·i` of a 58 %×76 % box, centred, opacity `i`; 1-in-10 `gatze.png` per appearance; click-through, no focus, on the output of the main window. Without layer-shell Gary is disabled (spec).
- Out of scope: the manual game crop (stubbed to `None` in the reference, digest 16) and the Windows keyboard overlay.

## Review Focus

1. The camera is unplugged or busy while recording or at save: the clip still saves, a warning is recorded, no `.fthr-finalize-*` directory is left. Test in Tasks 2 and 4.
2. An image layer file is missing, unreadable or corrupt: missing ones are dropped with their warning; a corrupt one does not lose the other layers or the camera (it is dropped after a failed pass and the pass retried once without it). Test in Task 4.
3. A save happens while a previous clip is still finalizing, or the app quits mid-finalize: clips finalize one at a time in order, quitting cancels and keeps the base clip. Test in Task 4.
4. Multitrack clips: every audio stream, its title and the manifest survive the rewrite (the manifest hash matches the new file). Test in Task 3.
5. Gary with no microphone, PipeWire missing or no layer-shell: nothing crashes, the overlay stays hidden. Test in Task 5.

---

## File structure

```
crates/fthrust-core/src/finalize.rs          rect clamp, layer normalisation, the burn graph
crates/fthrust-core/src/gary.rs              thresholds, intensity, step
crates/fthrust-media/src/camera/{mod,ring,devices}.rs   capture ring, device list, segment writer
crates/fthrust-media/src/export/run.rs       metadata copy for the rewrite
crates/fthrust-ui/src/services/camera.rs     recorder lifecycle, device scan, preview frame
crates/fthrust-ui/src/services/finalize.rs   post-save worker and readiness
crates/fthrust-ui/src/services/mic_level.rs  PipeWire level stream
crates/fthrust-ui/src/overlay/gary.rs        Gary layer-shell drawing
tools/uidrive/scenarios/overlays.sh          bench
docs/overlays.md
```

### Task 1: Core finalize graph and Gary maths

**Files:** Create `crates/fthrust-core/src/finalize.rs`, `crates/fthrust-core/src/gary.rs`, `crates/fthrust-core/tests/finalize_model.rs`; Modify `crates/fthrust-core/src/lib.rs`.

**Interfaces:**
- Produces: `finalize::clamp_rect(&Value, default: Rect) -> Rect` (`Rect{x,y,w,h}: f64`), `finalize::image_layers(&SettingsStore) -> Vec<ImageLayer{path, opacity: u8, fill: bool, rect}>` (enabled only, digest 17 normaliser), `finalize::camera_rect(&SettingsStore) -> Rect` (legacy migration digest 4), `finalize::pixels(rect, (w,h)) -> (x, y, w, h)`, `finalize::plan(layers: &[ImageLayer], camera: Option<&Path>, clip: (u32,u32)) -> Option<GraphPlan>` (video `Encode`, audio `Copy` of all streams, `None` when nothing to burn); `gary::{clamp_thresholds, intensity_for_level, step_intensity}`.

- [ ] Step 1: Failing tests: `rect_clamp_matches_reference` (size first, 4 decimals, bad fields fall back per field), `legacy_camera_rect` (medium bottom-right = {0.73, 0.6467, 0.25, 0.3333}), `layers_normalised` (global flag AND layer flag, opacity floor 10, fill only for "fill", broken rect falls back to its slot), `pixels_truncate_and_floor` (64×48 minimum, even sizes), `graph_images_then_camera` (exact text: `movie=` with escaped paths, `format=rgba,scale…pad…:color=0x00000000,colorchannelmixer=aa=0.720`, fill chain, overlays chained `[0:v]→[layer0]…`, camera `scale…decrease,pad…` black then `overlay=x:y:eof_action=repeat`, last label `[vout]`), `nothing_to_burn_is_none`, `gary_maths` (thresholds clamp, intensity endpoints and midpoint, step lands exactly).
- [ ] Step 2: FAIL. Step 3: Implement. Step 4: PASS; clippy.
- [ ] Step 5: Commit `feat(core): post-save overlay graph and Gary maths`.

### Task 2: Camera ring and devices

**Files:** Create `crates/fthrust-media/src/camera/{mod,ring,devices}.rs`, `crates/fthrust-media/tests/camera.rs`; Modify `crates/fthrust-media/src/lib.rs` (`ff::device::register_all()` in `init`).

**Interfaces:**
- Produces: `camera::list_devices() -> Vec<Device{path: PathBuf, name: String}>` (v4l2 capture nodes, `Camera N` when unnamed); `camera::Source{format: &'static str, url: String, options: Vec<(String,String)>}` with `Source::v4l2(path)` and `Source::synthetic(w,h,fps)` (lavfi); `CameraRecorder::start(Source) -> Result<CameraRecorder, String>` (thread `fthr-camera`), `is_running()`, `latest_jpeg() -> Option<(u64 /*seq*/, Vec<u8>)>`, `stop()`; `write_segment(&self, end: Instant, duration_s: f64, path) -> Result<usize /*frames*/, String>` (MJPEG in Matroska, pts = capture time − (end − duration), frames before 0 dropped).

- [ ] Step 1: Failing tests (synthetic source): `ring_keeps_ninety_seconds` (a test-only clock override shortens time: ring drops old entries), `segment_has_real_timestamps` (record ~1.2 s at 15 fps, write the last 1.0 s: ~15 frames, first pts ≥ 0, last pts ≤ 1.0, decodable MJPEG), `latest_jpeg_advances`, `bad_device_fails_cleanly` (`/dev/nonexistent` → `Err`, no thread left), `stop_joins` (stop returns within 2 s and `is_running` is false), `list_devices_does_not_panic` (whatever the machine has).
- [ ] Step 2: FAIL. Step 3: Implement. Raw frames (testsrc, YUYV webcams) go through an `mjpeg` encoder (qscale ≈ quality 85); MJPEG packets are copied. A read error stops the ring and marks it not running (digest 14 hardening).
- [ ] Step 4: PASS; clippy. Step 5: Commit `feat(media): camera ring over libavdevice`.

### Task 3: Runner rewrite support

**Files:** Modify `crates/fthrust-media/src/export/run.rs`, `crates/fthrust-media/tests/export_run.rs`.

**Interfaces:** `Job` gains `copy_metadata: bool` (container and per-stream tags such as audio titles copied to the output).

- [ ] Step 1: Failing tests: `rewrite_keeps_tracks_and_titles` (3-track clip through a `finalize::plan` graph with one image layer: 3 audio streams with the same titles, video re-encoded, duration kept), `camera_movie_overlay_burns` (synthetic camera segment over a clip: a pixel inside the camera box differs from the base clip, outside it matches within tolerance).
- [ ] Step 2: FAIL. Step 3: Implement. Step 4: PASS. Step 5: Commit `feat(media): metadata-preserving rewrites for post-save overlays`.

### Task 4: Finalize service and camera service

**Files:** Create `crates/fthrust-ui/src/services/{finalize,camera}.rs`, `crates/fthrust-ui/tests/finalize.rs`; Modify `services/mod.rs` (save completion hands the clip over; `RefreshCameras` handled; camera start/stop on `camera_enabled` and device changes; shutdown), `services/save.rs` (`engine_committed(path, needs)`), `settings/visuals.rs` (live camera frame in the placement box at 10 fps while Visuals is shown), `library/page.rs` (opening a finalizing clip shows `CLIP STILL FINALIZING`), `state.rs`.

**Interfaces:** `Finalizer::submit(clip, save_instant: Instant, duration_s, settings snapshot)`; one worker, FIFO; `poll(&mut AppState)` applies readiness and toasts; `shutdown(timeout)` cancels and keeps base clips. `CameraService` owns the `CameraRecorder`, scans devices on a thread (the device in use is listed from state, never reopened), exposes the preview JPEG.

- [ ] Step 1: Failing tests: `nothing_enabled_is_ready_immediately`, `image_layer_burns_and_rebinds_manifest` (manifest hash equals the new file), `missing_image_warns_and_keeps_others`, `corrupt_image_retries_without_it`, `camera_unavailable_records_warning`, `saves_finalize_in_order`, `quit_mid_finalize_keeps_base_clip` (no `.fthr-finalize-*` left), `last_warning_is_shown`, `camera_open_failure_turns_it_off` (setting false, `CAMERA UNAVAILABLE` with the digest 5 text), `refresh_keeps_selection_and_running_device`.
- [ ] Step 2: FAIL. Step 3: Implement. Step 4: PASS; clippy. Step 5: Commit `feat(ui): post-save overlays and the camera service`.

### Task 5: Mic level and Gary

**Files:** Create `crates/fthrust-ui/src/services/mic_level.rs`, `crates/fthrust-ui/src/overlay/gary.rs`, `crates/fthrust-ui/tests/gary.rs`; Modify `services/mod.rs` (level stream while the Audio page is visible or Gary is on; the 33 ms Gary tick), `overlay/mod.rs` (a second layer-shell surface: full output, empty input region, no keyboard), `settings/audio.rs` (Choose / Default image wiring, `Image not supported` check).

**Interfaces:** `MicLevel::start(target: Option<String>, gain: f32) -> Result<MicLevel, String>`, `level() -> (f32 /*level*/, f32 /*peak*/)` (0 after 0.35 s stale); `GaryState::tick(level, enabled, thresholds) -> f32` with the cat roll from an injectable RNG; `Overlay::gary(intensity, image)`.

- [ ] Step 1: Failing tests: `level_is_rms_times_four` (feed blocks to the level accumulator), `stale_level_is_zero`, `gary_rises_and_falls_at_reference_rates`, `cat_rolls_once_per_appearance` (seeded RNG: exactly one cat in ten appearances), `gary_geometry` (fit box 58 %×76 % scaled by 0.55+0.45·i), `disabled_or_no_audio_hides`.
- [ ] Step 2: FAIL. Step 3: Implement. Step 4: PASS; clippy. Live check on Hyprland through the bench only (never the user's screens).
- [ ] Step 5: Commit `feat(ui): mic level meter and Gary mode overlay`.

### Task 6: Bench, docs, roadmap

**Files:** Create `tools/uidrive/scenarios/overlays.sh`, `docs/overlays.md`; Modify `CHANGES.md`, roadmap (6d shipped), `.github/workflows/ci.yml`.

- [ ] Step 1: `overlays.sh`: settings with one image layer (`assets/fthr_logo.png`) enabled, save a clip, wait for the card to leave `Finalizing`, check the clip's pixel in the layer box changed (a small `uidrive` helper or ffprobe-free check through the app's thumbnail) and no `.fthr-finalize-*` remains.
- [ ] Step 2: Docs and CHANGES rulings (libavdevice instead of nokhwa; one pass instead of four; real camera timing; corrupt layer retried without it; all warnings kept in the readiness record, last one shown).
- [ ] Step 3: Full gate: fmt, clippy, `cargo test --workspace`, `cargo deny check`, every bench scenario.
- [ ] Step 4: Commit `feat: overlays bench scenario; docs: overlays`.
