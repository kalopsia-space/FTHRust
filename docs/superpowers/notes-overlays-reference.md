# Webcam picture-in-picture and Gary mode: Python reference notes

Paths are relative to `reference/FTHR_UI/` unless stated. `cr` = `core/camera_recorder.py`, `co` = `core/camera_overlay.py`, `ce` = `ui/camera_overlay_editor.py`, `gm` = `core/gary_mode.py`, `go` = `ui/gary_overlay.py`, `sm` = `core/settings_manager.py`, `main` = `main.py`. Tests are under `reference/tests/`. Line numbers are approximate to the line quoted.

## 1. Summary (read this first)

- The webcam IS composited into saved clips. It is a post-processing step, not part of the native capture engine: after the engine has closed and published the clip file, Python runs one extra FFmpeg re-encode (`MainWindow._apply_camera_overlay`, main:8320) that overlays a separately recorded webcam video onto the finished clip. The user-facing name is "Burn camera overlay into clips".
- The native engine sources (`reference/FTHRcapture`, `FTHRcapture_common`, `FTHRcapture_linux`) contain no camera, webcam or overlay-rect code. Grep for camera/webcam/overlay over their `.c/.cpp/.h/.hpp` (excluding `third_party`) matches only `encoded_video_config.cpp` (unrelated) and Wayland protocol headers. The only link between camera and engine is wall-clock: the save instant is `time.monotonic()` and the camera ring uses the same clock (section 8).
- The camera has no live compositing anywhere: not in the live screen capture, not in the viewer, not in playback. The clip viewer plays the baked-in result. The Settings "Visual Overlays" page shows a preview-only mock-up (section 6).
- Gary mode is unrelated to clips. It is a click-through, always-on-top, full-screen image that fades in as the microphone gets loud. It never touches recordings.
- Test-vs-source drift: `test_linux_camera_refresh.py`, `test_camera_provider_failures.py` and `docs/linux.md` ("Camera refresh") describe a hardened `CameraRecorder` (see section 12) that the source in this snapshot does NOT implement. Treat the source as the behaviour and the tests as intent.

## 2. Settings keys and defaults

Defaults in `sm` (`default_settings`): Gary at sm:188-193, camera at sm:269-275. Read with `sm.get(key, fallback)`.

| Key | Default | Used by |
|---|---|---|
| `camera_enabled` | `False` | master switch; checkbox "Burn camera overlay into clips"; recorder auto-starts at app launch when true (main:3457); `_apply_camera_overlay` returns immediately when false |
| `camera_device_index` | `0` | OpenCV index; combo `itemData`; saved on every combo change |
| `camera_overlay_rect` | `{x:0.72, y:0.64, w:0.25, h:0.34}` | normalized fraction of the clip frame (top-left origin); the only geometry actually used at burn time |
| `camera_position` | `'bottom-right'` | legacy; only used to derive a rect when `camera_overlay_rect` is missing |
| `camera_size` | `'medium'` | legacy, as above |
| `gary_mode_enabled` | `False` | checkbox "Enable Gary Mode" |
| `gary_min_level` | `55` | percent, 1..95 |
| `gary_max_level` | `85` | percent, min+5..100 |
| `gary_image_path` | `None` | custom image; `None` = bundled `assets/gary.png` |
| `settings_visual_previews_enabled` | `True` (from `core/ui_performance.py:19`) | label "Animated settings previews": when off the 100 ms camera preview timer is stopped and the live frame never shows (main:11807). Setting text: "Off: stop the moving camera and keyboard previews in Settings. This does not change what appears in recordings. After turning this back on, reopen Visuals to see the previews." |
| `input_overlay_preview_background` | `''` | path of the picture used as the fake "clip" behind the placement preview; empty = bundled `assets/preview_desktop.png` |
| `audio_capture_enabled` | `True` | Gary also requires this (section 10) |
| `mic_volume` | `100` | percent gain, `/100` fed to the mic recorder that also drives Gary |
| `error_notifications_enabled` | `True` | gates the "CAMERA UNAVAILABLE" bar (section 5) |

Not camera but shares the same preview: `image_overlay_*` / `image_overlays` (image layers) and the Windows-only keyboard overlay. Out of scope here except where they share the canvas (section 6).

Load-time migration (sm:394): if the loaded file has no `camera_overlay_rect`, it is computed with `legacy_overlay_rect(camera_position, camera_size)` (section 4). A `camera_overlay_rect` that exists is used as is (clamped at use).

## 3. Device discovery (`cr.list_devices`, cr:50)

Class methods, no instance needed. `is_available()` (cr:46) is true only if `import cv2` and `import numpy` both succeeded; otherwise `list_devices` returns `[]`.

1. Windows-style named scan first: run the bundled FFmpeg (`core.ffmpeg_tools.get_ffmpeg_exe()`) with `-hide_banner -list_devices true -f dshow -i dummy`, timeout 12 s, UTF-8 with `errors='replace'`, `creationflags=CREATE_NO_WINDOW` if it exists else 0. Output (stderr + "\n" + stdout) is matched with `r'"([^"]+)"\s+\(video\)'`; names are de-duplicated preserving order (`dict.fromkeys`); result is `[{'index': i, 'name': name}]` with `i` the position in that list. Any exception prints `[Camera] Named device scan failed: {exc}` and falls through.
2. Fallback (this is what runs on Linux; dshow does not exist there, FFmpeg exits non-zero, no names match): for `index` in `range(8)`: `cv2.VideoCapture(index)`, `isOpened()`, `release()` always (exceptions swallowed). Each opened index yields `{'index': index, 'name': f'Camera {index + 1}'}`. Missing indices are not invented (comment: "do not invent four entries"). Order is by index, so `/dev/video0..7` as OpenCV's default backend numbers them. There is no V4L2-specific code, no `/dev/v4l/by-id`, no card-name lookup, no capability check, so UVC metadata nodes can show up as extra "Camera N" entries if OpenCV opens them.
3. Cost: up to 8 synchronous probes plus the FFmpeg spawn (up to 12 s). The UI therefore runs it on a thread (section 6).

Names the user sees on Linux: "Camera 1" ... "Camera 8". Names on Windows come from DirectShow. Camera numbering (`index + 1`) in the label vs `index` in data is off by one by design.

## 4. Geometry helpers (`co`)

- `DEFAULT_OVERLAY_RECT = {x:0.72, y:0.64, w:0.25, h:0.34}` (camera). `DEFAULT_IMAGE_OVERLAY_RECT = {x:0.76, y:0.04, w:0.20, h:0.24}`. `MIN_OVERLAY_SIZE = 0.08`.
- `clamp_overlay_rect(rect, default=None)` (co:27): non-dict input becomes `{}`; each of `w,h,x,y` parsed with `float()`, falling back to the default's value on `TypeError/ValueError`. Then `w = max(0.08, min(w, 0.92))`, same for `h`; `x = max(0, min(x, 1 - w))`, `y = max(0, min(y, 1 - h))`; every value `round(v, 4)`. Order matters: size is clamped first, then position uses the clamped size. NaN is not handled (`min/max` with NaN is order dependent).
- `legacy_overlay_rect(position='bottom-right', size='medium')` (co:60): `w = {'small':0.20, 'medium':0.25, 'large':0.33}.get(size, 0.25)`; `h = w * 1.3333` (4:3 camera on a 16:9 canvas, expressed in normalized units, so `h` is larger than `w`); `margin = 0.02`; `x = margin if 'left' in position else 1 - w - margin`; `y = margin if position.startswith('top') else 1 - h - margin`; result clamped. Defaults: medium bottom-right = `{x:0.73, y:0.6467, w:0.25, h:0.3333}`.
- Legacy position/size combos still exist as setters (`_on_camera_pos_changed` keys `bottom-right, bottom-left, top-right, top-left`; `_on_camera_size_changed` keys `small, medium, large`, main:11795-11803) but nothing in the current Visuals page connects widgets to them. Dead code; only the load-time migration above matters.
- Image layer helpers (`new_image_overlay_layer`, `image_overlay_layers`) exist in the same file; they are image-overlay, not camera. Start positions cycle through `(0.76,0.04) (0.04,0.04) (0.04,0.72) (0.76,0.72) (0.40,0.38)`, size 0.20x0.24, opacity 100, fit `'fit'`.

## 5. Recording ring (`cr`, class `CameraRecorder`)

Process-wide singleton (`__new__`, `_instance`). Constants: `KEEP_SECONDS = 90`, `DEFAULT_FPS = 30`, `JPEG_QUALITY = 85` (comment: about 150-300 KB per frame at 1080p versus about 6 MB raw).

State: `_cap` (cv2.VideoCapture), `_device_index`, `_lock` (guards the ring), `_op_lock` (serializes start/stop so rapid toggles cannot interleave open/release), `_chunks` deque of `(monotonic_ts, jpeg_ndarray)`, `_running`, `_thread`, `_latest_frame` (raw BGR numpy array of the newest frame).

- `start(device_index=0) -> bool` (cr:94): blocking. Returns False if OpenCV unavailable. Under `_op_lock`: if already running on the same index and `_cap.isOpened()`, return True without reopening (avoids visible freezes). Otherwise `_stop_locked()`, store index, `cv2.VideoCapture(index)` (default backend, no resolution/FPS/FOURCC requested, so whatever the driver defaults to), return False if not opened, else set `_running`, start daemon thread `fthr-camera` running `_capture_loop`, return True.
- `start_async(device_index, on_result)` (cr:122): daemon thread `fthr-camera-open` calls `start()` then `on_result(ok)` (exceptions from the callback swallowed). The callback runs on the worker thread.
- `stop()` (cr:135): under `_op_lock`: `_running=False`, join the capture thread (timeout 2 s), release the cap, clear the ring under `_lock`, `_latest_frame=None`.
- `_capture_loop` (cr:155): while `_running`: break if cap is gone or not opened; `ret, frame = cap.read()`; on failure sleep 0.033 s and retry (no give-up, no reopen); on success `ts = time.monotonic()` taken AFTER the blocking read returns, store raw frame as `_latest_frame`, `cv2.imencode('.jpg', frame, [IMWRITE_JPEG_QUALITY, 85])`; if encode fails skip the chunk; append `(ts, buf)` under the lock and pop from the left while `chunk_ts < ts - 90`. There is no FPS cap or sleep: the ring is filled at whatever rate the driver delivers. Memory is therefore bounded by 90 s x camera rate x JPEG size, not by a frame count.
- `extract_segment(end_time, duration_sec)` (cr:175): copies the buffers whose `start <= ts <= end` where `start = end - duration`, under the lock, then `cv2.imdecode(..., IMREAD_COLOR)` each outside the lock; drops undecodable; returns the list or `None` if empty.
- `write_segment(path, end_time, duration_sec, fps=30)` (cr:183): extract, take `(h, w)` of the first frame, `cv2.VideoWriter(path, fourcc('mp4v'), fps, (w, h))`; False if no frames or writer not opened; skips frames whose shape differs; releases; True. MPEG-4 part 2 in an `.mp4`. The writer always stamps frames at exactly `fps` (the caller passes 30.0), regardless of the real capture rate (section 8 quirk).

Triggers:
- App start (main:3457, inside capture start): if `CameraRecorder.is_available()` and `camera_enabled`: `start_async(camera_device_index)`. Failure callback emits `_show_camera_unavailable('Could not open the selected camera. Confirm it is connected and not in use by another app.')` via `_ui_call`.
- Settings "Burn camera overlay into clips" checked (`_on_camera_toggled`, main:11675): set `camera_enabled=True`, save, `_set_camera_overlay_enabled(False)` (preview shows disabled until the open succeeds), `_start_camera_open(idx)`.
- Unchecked: bump `_camera_start_generation`, clear pending index, `camera_enabled=False`, save, `CameraRecorder().stop()`, stop the preview timer, mark preview disabled, clear the camera pixmap.
- Device combo change (`_on_camera_device_changed`, main:11695): ignore index <0 or `None` data; save `camera_device_index`; if enabled, `_start_camera_open(idx)` (recorder restarts on the new device).
- App shutdown (main:9545): `CameraRecorder().stop()`; failure prints `[Shutdown] Camera release failed: {error}` and shutdown continues; then marks `CameraStopped`.
- Settings page hidden: bumps the generation counter (so late open results are ignored) and stops the preview timer, but does NOT stop the recorder.

Async-open protocol on the settings page (`_start_camera_open`, main:11707): increments `_camera_start_generation`, records `_camera_pending_device_index`, stops the preview timer and clears the preview, then calls `start_async`. The result (`_camera_start_result` signal, `(generation, device_index, ok, diagnostic)`) is dropped if the generation differs, the device index is not the pending one, it is not the saved `camera_device_index`, or `camera_enabled` is false. On success: preview marked enabled and (unless background UI is paused) the preview timer starts. On failure:
1. `camera_enabled=False`, save.
2. Un-check "Burn camera overlay into clips" with signals blocked.
3. Stop the preview timer, mark preview disabled, clear the frame.
4. Emit `camera_unavailable` with: `Could not open camera device {device_index}. Confirm it is connected and not in use by another app. The camera overlay was turned off; choose another camera or try again.` plus ` ({diagnostic})` when a diagnostic exists (whitespace-collapsed, max 80 chars), whole string truncated to 240.
Diagnostics: `str(exc)[:80]` for exceptions from `is_available`/`start_async`, or `Camera capture is unavailable in this installation.` when OpenCV is missing.
The selected device index stays saved on failure (`test_camera_settings_failures.py::test_device_change_failure_keeps_selected_device_but_disables_camera`); the rect and legacy position/size are untouched.

User-visible failure surfaces:
- `MainWindow._show_camera_unavailable(detail)` (main:9068): returns silently if `error_notifications_enabled` is false. Otherwise `error_bar.push('CAMERA UNAVAILABLE', detail[:240] or 'Camera could not be opened.', 'warning', [('OPEN CAMERA SETTINGS', self._toggle_settings_page)])`. Comment: "without adding a public error code". The settings page's `camera_unavailable` signal is wired to this (main:4045).
- Burn-time: `CAMERA OVERLAY FAILED` / `FFmpeg error. Clip saved without camera overlay.` (level `warning`, via `push_error`, only when FFmpeg returns non-zero). The other burn-time problems only record a clip-readiness warning (section 8), no toast.

## 6. Settings UI: "Visuals" tab, section "Visual Overlays"

Settings categories (main:10261): `General, Clip, Audio, Visuals, Customize, Performance, Shortcuts`; Visuals is stack index 3. `_make_visuals_page` returns `_make_visuals_page_unified_legacy` (main:~12970-13320), the live page. `_make_visuals_page_legacy` (main:13325) is an older stacked layout with a section header "Camera Overlay" and a separate `CameraOverlayEditor`; only reachable if swapped in. There is no modal "overlay editor dialog": the editor is an inline widget in this page. Everything below is the live page.

Layout: section header `Visual Overlays`; a two-column row with spacing 16: left `overlayOptionsPanel` (fixed width 380, transparent) and right `overlayPreviewPanel` (stretch 1, left margin `Sizes.ROW_GAP`). The page is in a scroll area. Left column, camera part:

- Uppercase accent label `Camera` (option header; the texts are rendered through `label_uppercase`).
- Checkbox `Burn camera overlay into clips` (`camera_check`), checked from `camera_enabled`.
- Row (spacing `Sizes.SPACE_3`): label `Device` (fixed width 58), combo `camera_device_combo` (stretch 1, `_DropdownCombo`), icon button `refresh.png` (label "Refresh", objectName `micRefreshBtn`, fixed 26x26, tooltip `Re-scan video devices`).
- 18 px spacing, then the `Image layers` and keyboard sections (out of scope), then `Preview background`, a read-only path field showing `DEFAULT · DESKTOP SCREENSHOT` when none is set else the path, and buttons `Choose image` (outline, min height 36) and `Use default` (secondary, min height 36). Choose opens a file dialog titled `Choose preview background`, start dir = parent of the current choice else home, filter `Images (*.png *.jpg *.jpeg *.webp *.bmp *.gif);;All files (*)`; an unreadable image is silently ignored (no message).

Device combo behaviour (`_populate_camera_devices`, main:11620):
- Clears, adds the single disabled item `Scanning cameras…` (with signals blocked), and runs `CameraRecorder.list_devices()` on a daemon thread `fthr-camera-device-scan`. If a scan thread is already alive, the call returns without starting another (refresh while scanning is a no-op, shared scan).
- Result: `devices` list, or on exception `([], 'Camera discovery failed; try Refresh')`, or when OpenCV is missing `([], 'Camera capture unavailable')`. Delivered through `_camera_devices_result` (queued to the UI thread); a destroyed page is tolerated.
- Apply (`_apply_camera_devices`): with signals blocked, clear; no devices -> single disabled item with the error text or `No cameras detected`; otherwise enabled, one item per device (display `name`, data `index`), select the item whose data equals the saved `camera_device_index`, else item 0. Because signals are blocked, a refresh never saves or restarts the recorder (test: `test_refresh_preserves_saved_selection_and_suppresses_change_signal`). If the saved index is missing, item 0 is shown but the saved setting is left unchanged until the user picks something.
- The page triggers a first scan on construction (`_populate_camera_devices()` inside the page build), so the first Visuals visit starts it; `test_settings_camera_discovery.py` requires that a slow scan not block the UI or tab switching.

Preview panel:
- Header row: title `Composite preview` (uppercase label, `Fonts.SIZE_LABEL`, tracking 2) at left; at right a dim micro label `Path(bg).name` or `DEFAULT · DESKTOP`, tooltip = full background path or the bundled default path.
- `UnifiedOverlayPreview` (section 7), expanding both ways.
- Live frames: `_camera_preview_timer`, interval 100 ms (10 fps), runs only when the Visuals tab (index 3) is current, the page is visible, camera enabled and open OK, and not "background UI paused". `_update_camera_preview` (main:11805): bail if `settings_visual_previews_enabled` is false (also stops the timer), bail if preview widget hidden or `latest_frame` is None; downscale with `cv2.resize(..., INTER_AREA)` by `scale = min(1, 640/w, 360/h)` (never upscale), BGR to RGB, build a QImage `Format_RGB888` from `rgb.tobytes()`, convert to QPixmap, `set_camera_pixmap`. Comment: the preview widget scales in the painter.
- Hide/pause: `hideEvent` stops the camera preview timer and bumps the start generation; `showEvent` / category switch to index 3 restarts it if `camera_enabled`; `set_background_ui_paused(True)` stops it.

Rect persistence: `_on_input_overlay_rects_changed(rects)` (main:12056) runs on mouse release only: if `'camera'` in rects, `sm.set('camera_overlay_rect', clamp_overlay_rect(rect, DEFAULT_OVERLAY_RECT))`, then image layers and keyboard are saved, then settings written. There is no reset-to-default button for the camera rect and no numeric entry; the only way to change it is dragging.

## 7. Placement editor widgets (`ce`)

Three classes. Live page uses `UnifiedOverlayPreview`; `OverlayPlacementEditor` is its base; `CameraOverlayEditor` is a thin compat wrapper (`OverlayPlacementEditor('CAMERA', DEFAULT_OVERLAY_RECT)` plus `set_frame = set_overlay_pixmap`) used only by the legacy page.

Canvas geometry (both): `_canvas_rect` = the largest 16:9 rectangle that fits inside the widget minus a 1 px margin, centred: `canvas_w = min(avail_w, avail_h * 16/9)`, `canvas_h = canvas_w * 9/16`. All rects are normalized to this canvas. A 16:9 clip is always assumed; a non-16:9 real clip gets the same normalized rect applied to its own width/height at burn time, so the box shape changes with the clip aspect.

`OverlayPlacementEditor` (base): min size 360x220, hint 520x320, expanding horizontally; mouse tracking on. Signal `rect_changed(dict)` emitted on mouse release.
- Empty text default `ENABLE {LABEL} TO PREVIEW`. Label stored uppercased.
- Cursor: inside the resize handle `SizeFDiagCursor`, inside the box `SizeAllCursor`, else default.
- Handle hit area: `QRectF(box.right-18, box.bottom-18, 24, 24)` (checked before the box, so it wins). Only the left button starts a gesture.
- Move: `x += dx`, `y += dy` where `dx = (px - start_px) / canvas_w`, `dy = (py - start_py)/ canvas_h`, from the rect at press time (not incremental). Then `clamp_overlay_rect`.
- Resize (base class only): `w = clamp(w0 + dx, 0.10, 0.88)`, `h = _height_for_width(w) = w * canvas_w / (canvas_h * overlay_aspect)`; aspect locked to the overlay pixmap's aspect (default 4/3) so the box tracks the picture. Then `clamp_overlay_rect`.
- Draw: fill widget with `Colors.BG`; stage = clip placeholder or background; 1 px `BORDER_HI` canvas outline. Overlay content (fit-scaled pixmap, or sample text, or accent tint alpha 28 enabled / 12 disabled with the empty text in 8 pt body font), then header.
- Header (`_draw_overlay_header`): rectangle outline 2 px (3 px when selected) in `ACCENT` when enabled else `TEXT_MUTED`; a 20 px tall bar across the top filled `BG` alpha 185; label text 8 pt bold display font at `left+8`, colour `TEXT` enabled else `TEXT_DIM`; a 13x13 solid square in the bottom-right corner as the visible resize handle (the grab area is bigger than the square, see above).
- Placeholder when no background: canvas `SURFACE_2`, lower part from 57% of the height `SURFACE_1`, two `BORDER`-coloured blocks (x 8%, y 14%, 26%x30%; x 66%, y 20%, 20%x24%), top-left text `CLIP PREVIEW` in `TEXT_MUTED`. Background image drawn "cover" (centre crop to canvas ratio) then a `BG` alpha 34 veil.
- Font helper: display family if installed, else `Oswald`, else `DejaVu Sans`.

`UnifiedOverlayPreview` (min size 640x360, hint 820x500; signal `rects_changed(dict)`): holds `_rects` keyed `'camera'`, optionally `'keyboard'`, and `'image:N'`.
- Hit order: `camera`, `keyboard` (if present), image layers in reverse (topmost first). Camera therefore wins over anything it overlaps. Per kind: handle region first, then box. First hit wins; selecting an image layer sets `_selected_image`.
- Move is as above. Resize here is FREE-FORM, not aspect locked: `w += dx`, `h += dy` then `clamp_overlay_rect` (so min 0.08 and max 0.92 per axis; the 0.10/0.88 limits of the base class do not apply). The camera box can therefore be any shape; the burn step letterboxes (section 8).
- Draw order (paint region culling): stage, image layers (index order), keyboard, camera on top. Camera box: content = `box.adjusted(2, 22, -2, -2)`; live frame drawn "fit" (letterboxed, centred) into it; with no frame: accent tint alpha 22 enabled / 10 disabled and centred text `CAMERA PREVIEW` (`TEXT_DIM`, 8 pt); header label `CAMERA`, state from `set_overlay_enabled('camera', bool)`.
- `set_camera_pixmap` repaints only the camera region (box plus 3 px margin).
- Image/background decoding is cached (`_cached_overlay_pixmap`: tiers of 128 px, max 4,000,000 px, LRU of 12 entries, `QImageReader` with auto-transform); a 15 s idle timer after hide drops hidden previews. Not needed for a camera-only Rust port.

Mock "clip" behind the boxes: user-chosen image, else the bundled `assets/preview_desktop.png` (1919x1079 RGBA). It is only a visual aid; it does not change geometry.

## 8. How the camera reaches a saved clip (`_apply_camera_overlay`, main:8320)

Where it runs. Three call sites, all after the clip file is final and after other post steps, in this order: `_apply_crop`, `_apply_image_overlay`, `_apply_keyboard_overlay`, `_apply_camera_overlay` (so the webcam is on top of images and keyboard, and its rect is relative to the already-cropped frame). Sites: `_finalize_combined_audio_clip` path (main:7766), the mic-mux path `_post_process_mic` (main:8037; comment: failures in the mic mix are non-fatal, "watermark/crop/camera still apply"), and the `finalize` route worker (main:8544, emits `emit_event('clip_save', 'camera_overlay_stage_completed', elapsed_ms=...)`). `select_post_route(..., camera=any(camera_enabled, image_overlay_enabled), ...)` is passed at main:7134 but the argument does not change the route selected in this snapshot (`'mic'` or `'finalize'` only).

Time base. `mic_end_time = time.monotonic()` is taken at the accepted save instant (main:6841) and passed as `clip_end_time`/`mic_end_time`; the same value is used to extract microphone and camera. The requested `duration_seconds` is the clip length. No latency compensation, no offset setting.

Algorithm:
1. Return if `camera_enabled` is false.
2. If `CameraRecorder` is unavailable or `is_running()` is false: `_record_finalization_warning(clip_path, 'Camera overlay was requested but the camera was unavailable.')`; return. (No toast.)
3. `tmp_dir = mkdtemp(prefix='.fthr-finalize-', dir=dirname(clip_path))`; `cam_path = tmp_dir/camera.mp4`.
4. `write_segment(cam_path, clip_end_time, duration_sec, 30.0)`. On False: warning `Camera overlay could not be rendered; the base clip was retained.`, `rmdir(tmp_dir)`, return.
5. `clip_w, clip_h = _clip_dimensions(clip_path, ffmpeg)` (main:8044): runs `ffmpeg -i clip` (timeout 30), regex `(\d{3,5})x(\d{3,5})` on stderr (first match; can mis-hit if another WxH string precedes the video stream), fallback `1920x1080` with log `[Post] Could not probe clip dimensions: {exc}`.
6. `rect = clamp_overlay_rect(camera_overlay_rect)`; if the stored value is not a dict, `legacy_overlay_rect(camera_position, camera_size)` first.
7. Pixel geometry: `cam_w = max(64, int(clip_w * rect.w)) & ~1`; `cam_h = max(48, int(clip_h * rect.h)) & ~1` (forced even); `overlay_x = max(0, int(clip_w * rect.x))`; `overlay_y = max(0, int(clip_h * rect.y))` (not forced even; truncation, no rounding).
8. FFmpeg command (timeout 120 s, `capture_output`, no-window flag on Windows):
   ```
   ffmpeg -y -i {clip} -i {cam_path}
     -filter_complex "[1:v]scale={cam_w}:{cam_h}:force_original_aspect_ratio=decrease,pad={cam_w}:{cam_h}:(ow-iw)/2:(oh-ih)/2[cam];[0:v][cam]overlay={x}:{y}[vout]"
     -map [vout] -map 0:a? -map_metadata 0 -map_chapters 0 -movflags use_metadata_tags
     {_cfr_video_args(clip)}  {software_video_args()}
     -c:a copy {tmp_dir}/output.mp4
   ```
   - The camera is scaled to fit inside the box preserving aspect (letterbox/pillarbox) and padded with FFmpeg's default pad colour (black), centred. So the webcam's own 4:3 or 16:9 shape shows inside a black rectangle of exactly the box size. No rounding, border, shadow, mirror, or opacity.
   - `-map 0:a?` keeps all audio streams (optional), re-encoded never (`-c:a copy`); metadata and chapters preserved.
   - `_cfr_video_args` (main:7769): probes the clip's average FPS; returns `['-fps_mode','cfr','-r', fps]` where integers print without a decimal and others as `%.6f` trimmed; empty list if unprobeable.
   - `software_video_args()` (`core/ffmpeg_tools.py:193`): always the SOFTWARE H.264 path, never NVENC/VAAPI (libopenh264 at 16 Mbit/s `-b:v 16000k` when that is the available encoder, else x264 CRF 18), plus seekable and SDR colour options. The burn therefore always re-encodes the whole clip in software.
9. On return code 0: `os.replace(output.mp4, clip_path)`, `rebind_manifest_after_media_replace(clip_path)`, log `[Camera] Overlay applied to {basename}`.
   On non-zero: log `[Camera] ffmpeg failed: {last stderr line or "(no stderr)"}`, warning `Camera overlay failed; the base clip was retained.`, toast `CAMERA OVERLAY FAILED` / `FFmpeg error. Clip saved without camera overlay.` (level `warning`, via `_ui_call`).
   On exception (including `subprocess.TimeoutExpired`): log `[Camera] Error: {e}` plus the same retained-clip warning; no toast.
10. `finally`: remove `camera.mp4` and `output.mp4` if present, `rmdir(tmp_dir)` best effort.

The base clip stays valid on every failure. The clip card/readiness flow holds the clip as "finalizing" until post steps are done (comment at main:7155, "camera/image/crop post-processing is still rewriting it").

Quirks of the burn (not to copy blindly):
- The camera video is written at 30 fps from however many frames the ring held. A webcam that delivers 15 fps makes the PiP play at 2x speed and end early; one that delivers 60 fps plays at half speed. No retiming by timestamps is done even though every JPEG has a timestamp. The overlay filter keeps the last frame when the camera stream ends (`overlay` default `eof_action=repeat`), so a short camera stream freezes rather than disappears.
- The camera segment starts at its first available frame, not at the clip start; if the camera was opened mid-window, the PiP starts at clip t=0 anyway, misaligned.
- Frame timestamps are taken after the blocking `read()` returns, so camera latency (typically 50-150 ms) is not compensated.
- `-fps_mode cfr -r` is only applied when the source probe succeeds.
- Heavy: 10-30 s worth of 1080p software re-encode per clip, 120 s timeout, and all other overlays each do their own full re-encode first (generation loss).
- `max(64, ...)`/`max(48, ...)` minimums apply to pixel size even if the rect is tiny; `x + cam_w` can exceed the frame on odd clamps (FFmpeg crops it).

## 9. Gary mode: pure maths (`gm`)

- `DEFAULT_MIN_LEVEL = 55`, `DEFAULT_MAX_LEVEL = 85`, `MIN_THRESHOLD_GAP = 5`.
- `clamp_thresholds(min, max)` (gm:11): `int()` each (fallback to the defaults on `TypeError/ValueError`); `low = max(1, min(low, 95))`; `high = max(low + 5, min(high, 100))`. Note: `low <= 95` and `high >= low + 5` can make `high` exceed 100 only if low>95, which the first clamp prevents.
- `intensity_for_level(level, min, max)` (gm:27): clamp thresholds; `percent = clamp(level, 0, 1) * 100`; if `percent <= low + 1e-9` return 0; if `percent >= high - 1e-9` return 1; else `(percent - low) / (high - low)`. Linear, no curve. Non-numeric level returns 0.
- `step_intensity(current, target, rise_step=0.18, fall_step=0.065)` (gm:41): clamp both to [0,1]; `delta = target - current`; `step = max(0.001, rise_step if delta > 0 else fall_step)`; if `abs(delta) <= step` return target (lands exactly), else `current +/- step`. Attack is about 5.6 frames from 0 to 1; release about 15.4 frames, at the 33 ms tick: roughly 0.18 s up, 0.5 s down.

## 10. Gary mode: runtime loop (`main`)

Setup (`_init_gary_mode`, main:3618): `GaryOverlay(Path(__file__).parent / 'assets' / 'gary.png', anchor=self)`; state `_gary_current_intensity=0`, `_gary_enabled=False`, thresholds defaults, `_gary_recorder_active=False`; `_sync_gary_settings()`; `QTimer` interval 33 ms (about 30 Hz) running `_update_gary_mode`, always started and left running while the app lives (stopped in shutdown, main:9514, together with `set_intensity(0)` and stopping the mic recorder if Gary started it).

`_sync_gary_settings(restart_recorder=True)` (main:6268), called at startup, when the settings page emits `gary_settings_changed` (main:3602), and after theme/app-style changes (main:5151): read `gary_mode_enabled`, clamp thresholds, `overlay.set_image(gary_image_path)`; `audio_enabled = audio_capture_enabled` (default True); `should_run = enabled and audio_enabled`.
- If should_run: `recorder_running = MicRecorder.is_available() and MicRecorder().is_running()`. If `restart_recorder` or not running: on win32 stop+start a dedicated PortAudio stream (`_start_gary_mic_recorder`, log `[Gary] Recorder started (device=..., gain=...)` or `[Gary] Recorder failed to start`); on other platforms (Linux) call `_start_mic_recorder()` only if not already running. That is the same `MicRecorder` singleton used for mic-in-clips (main:6193): if `sounddevice` is missing it pushes the error `MIC NOT FOUND` / `No recording device detected. Check audio settings or permissions.` (warning, action `OPEN AUDIO SETTINGS`); if start fails: `MIC NOT FOUND` / `Microphone failed to start. Clips will have no mic audio.` There is no Gary-specific error message. Device = `_resolved_mic_device_index()` (saved `mic_device_name`, unique name match, else system default), gain = `mic_volume / 100`.
- Else: intensity 0, `overlay.set_intensity(0)`, stop the mic recorder only if Gary started it (`_gary_recorder_active`).

`_update_gary_mode` per tick: if not enabled, or audio capture disabled, or `MicRecorder` unavailable: if intensity non-zero, zero it and hide; return. Else `level = MicRecorder().latest_level()`; `target = intensity_for_level(level, min, max)`; `current = step_intensity(current, target)`; `overlay.set_intensity(current)`.

Mic level source (`core/mic_recorder.py:117, 141`): each audio callback computes `rms = sqrt(mean(samples^2))` of channel 0 after gain, `normalized_level = clamp(rms * 4.0, 0, 1)`. `latest_level()` returns 0.0 if the last update is older than 0.35 s (device stalled), else the last value (instantaneous, no smoothing, no peak hold). So the threshold percentages are percent of `rms*4`, i.e. full scale is rms 0.25 (about -12 dBFS), a post-gain value. The settings page meter bar uses the same level (`Live level` meter), which is why the thresholds are placed on it.

## 11. Gary overlay window (`go`, `GaryOverlay`)

- Window flags: `Qt.Tool | FramelessWindowHint | WindowStaysOnTopHint | WindowTransparentForInput | WindowDoesNotAcceptFocus`, no parent. Attributes: `WA_TranslucentBackground`, `WA_ShowWithoutActivating`, `WA_TransparentForMouseEvents`. Click-through, never takes focus.
- Images: `_default_image` (constructor arg; the app passes `assets/gary.png`, 3072x3072 RGBA), `_cat_image = default.with_name('gatze.png')` (2294x2160 RGBA, tested to exist and have alpha, `test_gary_overlay.py::test_bundled_cat_image_is_available_and_transparent`). `set_image(path)`: candidate `Path(path).expanduser()` or default; if it fails to load and is not the default, fall back to the default; if even the default fails, clear everything, `hide()`, return False. Stores `_base_pixmap`/`_base_image_path`.
- `set_intensity(v)` (go:41): clamp 0..1. When going from 0 to above 0 (a new "appearance"), roll once: `use_cat = cat pixmap valid and random.randrange(10) == 0` (1 in 10, test asserts `randrange(10)` and exactly one cat in ten appearances); the choice is kept until intensity returns to 0. Then store intensity. If `v <= 0` or no pixmap: hide if visible, return. Otherwise fit the widget geometry to the screen, `show()` if hidden, `raise_()`, `update()`.
- Screen choice (`_fit_to_anchor_screen`): screen under the centre of the anchor (main window, `mapToGlobal(rect.center)`), else primary; geometry set to the full screen geometry only if different. So it follows the main window's monitor, not the focused game.
- Paint: `SmoothPixmapTransform`; `scale = 0.55 + 0.45 * intensity`; `max_w = int(width * 0.58 * scale)`, `max_h = int(height * 0.76 * scale)`; pixmap scaled to fit (KeepAspectRatio, smooth) into `max_w x max_h`, centred on the screen; `painter.setOpacity(intensity)`. So the picture grows from 55% to 100% of the 58%x76% box while fading in. Nothing is drawn when intensity <= 0.
- No fade timers in the window; fading is entirely driven by the 33 ms `step_intensity` loop in the main window.

## 12. Gary settings UI strings and widgets

Location: Settings, Audio tab (index 2), below the microphone controls and loopback option (`Plays your microphone back through your speakers in real time so you can judge volume. Stops when you leave this page.`), main:11346.
- Section header: `Gary Mode` (flat section header), 8 px spacing below.
- `QFrame#garyPanel` (transparent, no border) with a horizontal layout, margins 10,4,10,4, spacing 12, containing in order: preview `QLabel#garyPreview` (fixed 76x58, centred, bg `Colors.BG`, 1 px `BORDER_HI` border, radius `RADIUS_MD`); checkbox `Enable Gary Mode`, tooltip `Show the selected image when the microphone crosses the start handle.`; stretch; button `Choose` (`garyImageButton`, pointing-hand cursor); button `Default` (same style: transparent, 1 px `BORDER_HI`, `TEXT`, display font, `SIZE_MICRO`, bold 600, padding 5px 7px, hover border/text `ACCENT`).
- Preview (`_refresh_gary_preview`): effective path = saved custom path if it is an existing file, else the bundled `assets/gary.png`; pixmap scaled to `size - 8x8` keep-aspect smooth; if the image cannot load: text `No image`. Tooltip = the effective path.
- `Choose` (`_choose_gary_image`): file dialog title `Choose Gary Mode image`, start dir = current custom path or home, filter `Images (*.png *.jpg *.jpeg *.webp *.bmp *.gif);;All files (*.*)`. Cancel = nothing. If `QPixmap(path).isNull()`: modal warning `FthrMessageDialog.warning(self, 'Image not supported', 'Choose a valid PNG, JPG, WEBP, BMP, or GIF image.')`. Else store path, refresh preview, save.
- `Default` clears `gary_image_path` (to `None`), refresh preview, save.
- Every change calls `_save_gary_settings` (main:12654): writes `gary_mode_enabled`, `gary_min_level`, `gary_max_level` (clamped), `gary_image_path`, saves settings, emits `gary_settings_changed` which makes the main window re-sync (and, with the default `restart_recorder=True`, restart the mic recorder each time, which a Rust port should avoid).
- Missing audio dependency: red body-style label `Mic features need the "sounddevice" Python package.\n   Install it from your venv:  pip install sounddevice numpy` (shown below the Gary panel when `sounddevice` is not importable; this is the generic mic-features warning).
- Threshold handles live on the mic level meter widget (`_MicLevelMeter`, main:~660-900), not on the Gary panel. See next section.

Mic level meter with Gary handles:
- Fixed height 36, min width 120, mouse tracking. Tooltip: `Drag the amber MIN and white MAX handles to set Gary Mode.` plus ` Gary is currently off.` when the checkbox is off. Signal `gary_thresholds_changed(int, int)`.
- `x(value) = 1 + int((width - 2) * value / 100)`. Press (left): pick the nearer handle (ties choose MIN), size-horizontal cursor, set from x immediately. Drag value = `round(clamp(x - 1, 0, width - 2) * 100 / (width - 2))`; MIN: `max(1, min(value, max - 5))`; MAX: `min(100, max(value, min + 5))`. Hover shows a horizontal-resize cursor within 12 px of either handle. On release the final x is applied and the signal is emitted only if the pair changed since press. Handles cannot cross.
- Drawing: bar with 1 px `HAIRLINE` frame, level fill in `ACCENT`, peak tick 2 px `TEXT` (peak decays on a 60 ms timer), and per handle a 8x5 cap at `x-3, bar_y-4` plus a 2 px line spanning `bar_h + 7`, MIN in `Colors.WARNING` (amber), MAX in `Colors.TEXT` (white). Labels `MIN 55` / `MAX 85` in bold 7 pt body font in a 44x10 rect at the top: MIN right-aligned ending at the handle (`text_x = max(0, x - 48)`), MAX left-aligned (`text_x = min(w - 48, x + 5)`).
- When Gary is toggled off, a drag in progress is cancelled and the cursor reset; the handles still show and can still be dragged when off.

## 13. Complete user-visible strings

Camera: `Burn camera overlay into clips`; `Camera` (section label, uppercase); `Camera Overlay` (legacy page header); `Visual Overlays` (page header); `Device`; `Re-scan video devices` (tooltip); `Refresh` (icon label); `Scanning cameras…`; `No cameras detected`; `Camera capture unavailable`; `Camera discovery failed; try Refresh`; `Camera N` (N = index+1); `Composite preview`; `DEFAULT · DESKTOP`; `DEFAULT · DESKTOP SCREENSHOT`; `Preview background`; `Choose image`; `Use default`; `Choose preview background`; `CAMERA` (box header); `CAMERA PREVIEW`; `CLIP PREVIEW`; `ENABLE CAMERA TO PREVIEW` (legacy editor empty text); `CAMERA UNAVAILABLE` + messages in sections 5; button `OPEN CAMERA SETTINGS`; `CAMERA OVERLAY FAILED` + `FFmpeg error. Clip saved without camera overlay.`; finalization warnings listed in section 8; `Camera capture is unavailable in this installation.`; log prefixes `[Camera]`, `[Gary]`, `[Shutdown]`.
Gary: `Gary Mode`; `Enable Gary Mode`; tooltip `Show the selected image when the microphone crosses the start handle.`; `Choose`; `Default`; `No image`; `Choose Gary Mode image`; `Image not supported`; `Choose a valid PNG, JPG, WEBP, BMP, or GIF image.`; meter tooltip text; `MIN n` / `MAX n`.

## 14. Linux and Wayland specifics

- Camera: no Linux-specific code in the shipped source beyond the OpenCV fallback (section 3). OpenCV's default backend picks V4L2 for integer indices; PipeWire camera portals are not used (a sandboxed or Flatpak build would see no devices). The named scan (`dshow`) is attempted and silently fails on Linux, spending an FFmpeg spawn per refresh.
- `docs/linux.md` "Camera refresh" (and `test_linux_camera_refresh.py`) say that on Linux a running camera must stay in the list WITHOUT being probed (exclusive-access V4L2 devices fail a second open), that refresh must not emit selection changes or restart the recorder, and that failed readers release their capture. The tests also expect `camera_recorder` to import `sys` and branch on `sys.platform == 'linux'`, a list entry `{'index': 4, 'name': 'Camera 5'}` for the active device, exactly 8 probes minus the active one, each released once. The source here does none of the active-device skipping. A Rust port should implement the documented behaviour: do not reopen the device that the ring is already using; list it from state.
- `test_camera_provider_failures.py` documents the intended recorder hardening: any exception from `isOpened`/`read`/`imencode` in the capture thread releases the capture, clears the ring and `latest_frame`, and marks not running (restart works afterwards); constructor/thread-start failures release and return False; `stop()` waits for a blocked read to finish before another device may open (a second `start` returns False while the old thread is alive); a failed JPEG encode keeps the raw preview frame but adds no chunk; the chunk timestamp is recorded before encoding (the test fakes the clock and expects the first chunk ts to be the pre-encode value, 100.0; the source also stamps before `imencode`, so this one matches); the open callback logs `Open-result callback failed: RuntimeError: callback failure` and does not poison state; stop cancels encoding before a JPEG is published. Most of these are NOT in the source.
- Gary overlay on Wayland (the Rust app is Wayland-only): Qt's `WindowStaysOnTopHint`, `setGeometry` to a screen and `raise_()` are advisory under Wayland; a normal toplevel cannot position itself or be forced topmost, and a stay-on-top frameless tool window usually appears as an ordinary window. Reproducing "click-through, always on top, full screen, no focus" needs a layer-shell surface (`wlr-layer-shell`, layer `overlay`, anchored to all edges, keyboard interactivity none, empty input region via `wl_surface.set_input_region` with an empty region, exclusive zone -1) on the output under the main window. This is my recommendation, not something the reference does; the reference has no Wayland branch for Gary, and the screen it picks (`screenAt` of the main window centre) maps to "the output containing the main window".
- Mic for Gary on Linux is the same sounddevice (PortAudio) stream as mic-in-clips; there is no separate Gary stream (that exists only on win32). The Rust app's existing mic capture can feed the same RMS value.
- Camera burn FFmpeg runs through the same bundled/system FFmpeg and `software_video_args()`; no VAAPI path.

## 15. Quirks and things not to copy blindly

1. Whole-clip software re-encode per overlay; four independent re-encodes if crop, images, keyboard and camera are all on. A Rust port should fold them into one filter graph.
2. 30 fps hard-coded for the camera file regardless of real capture rate; use per-frame timestamps (ring has them) to resample or compute the real rate.
3. No latency compensation and no start alignment of camera frames to clip start (section 8).
4. Aspect: the stored rect is normalized to the clip, but the editor assumes 16:9; on ultra-wide or portrait clips the box shape is different from what the user placed, and the black padding hides that.
5. Two resize behaviours: base editor locks the box to the camera's aspect with width limited to 0.10..0.88; the live unified editor lets width and height vary independently with 0.08..0.92. The burn always letterboxes with black bars, so free-form boxes waste space.
6. Handle hit box (24x24 at `right-18,bottom-18`) extends outside the box by 6 px but the drawn square is 13x13.
7. The recorder keeps running whenever `camera_enabled`, even when no clip will be saved, at the camera's full frame rate, JPEG-encoding every frame (CPU and disk-free memory use), for the whole app session, with only a 90 s history. If the camera opens but never delivers frames, the thread loops forever with 33 ms sleeps and `is_running()` stays true, so burn will report "could not be rendered" rather than "unavailable".
8. `camera_enabled` is flipped off and persisted on any open failure; the startup failure path (main:3457) only shows a toast and does NOT clear the setting or the checkbox, so the toast can reappear every launch and clips silently lack the PiP (only a clip-readiness warning is recorded at burn time).
9. `_on_camera_pos_changed` / `_on_camera_size_changed` and the whole legacy page are dead code but remain referenced by tests; do not port them.
10. `_clip_dimensions` parses FFmpeg's human stderr; use ffprobe or the container's real size.
11. `_save_gary_settings` re-syncs and restarts the mic stream on every checkbox, handle drag or image change.
12. `test_alpha_ui_truth.py::test_gary_default_uses_the_approved_generated_logo` expects `'assets' / 'fthr_logo.png'` in main, but Gary's default is `assets/gary.png` (the `fthr_logo.png` reference at main:9379 is the app/tray icon); stale test, ignore.
13. The Gary overlay rolls its 1-in-10 cat image (`gatze.png`) on every appearance (intensity from 0 to above 0), not per session; a custom image still gets the cat roll unless `gatze.png` is missing next to the default image. The cat is looked up as a sibling of the DEFAULT image path, not of the custom one.
14. Mic level for Gary is `clamp(rms * 4)` of post-gain samples, so raising Settings mic gain moves the thresholds' meaning; thresholds are percent of that scale.
15. Window placement follows the main window, so if the main window is minimized or on another output from the game, Gary appears on that other output.

## 16. Manual game crop at save (`_apply_crop`, main:8411)

Status in this snapshot: the Python crop step is effectively DEAD CODE. Where the crop profile should be chosen, the source is stubbed: `_requested_capture_config` (main:5331-5334) has `crop = None` followed by an unused `getattr(self, '_active_game_window', None)`, a no-op `int(...target_hwnd...)` try and a bare `pass`; `save_clip` (main:6842-6844) has `crop_profile = None` plus the same unused `getattr` and `pass`. So `crop_profile` is always `None`, `_apply_crop` returns on its first line, `CaptureConfig.crop_enabled` is always False, and the engine receives `0 0 0 1 1`-style args (argv: `'1'/'0'`, then x, y, w, h as `%.9g`, main:5739-5743). The lookup that used to feed it exists and is tested: `core/game_detector.py:472 crop_profile_for_window(window, rules)`, but nothing in main.py calls it. `test_alpha_ui_truth.py:332 test_legacy_game_crop_is_ignored_on_windows_and_retained_on_linux` expects the opposite (Linux config carries `crop_enabled` with x/y/w/h 0.1/0.2/0.7/0.6 from the active game's rule); that test fails against this source (stale/drift). Treat the data model and the `_apply_crop` maths below as the intent; the wiring is missing.

Where a profile comes from (intended):
- Per-game rule list setting `game_detection_custom_games` (default `[]`, sm:251). Each rule: `title_contains`, `exe_path`, `folder_path`, `exe_name` (at least one non-empty; de-duplicated on casefolded `(title, norm exe_path, norm folder, exe_name)`) plus optional `crop_profile` (`normalise_custom_game_rules`, gd:~395-436). `matching_custom_game_rule` returns the FIRST rule matching the window (title substring casefold; exe path equality or folder prefix; exe_name). So a crop applies only to the game whose rule it is stored on; there is no global crop setting.
- `normalise_crop_profile` (gd:367): non-dict or non-numeric -> `None` (dropped); non-finite -> `None`; `x,y` clamped to 0..1; `w = min(1-x, max(0.01, w))`, `h` likewise; `None` if w or h <= 0 (x or y at 1.0); output `{'enabled': bool(enabled, default True), 'x','y','w','h'}` each `round(.., 6)`. Defaults when a key is missing: x 0, y 0, w 1, h 1.
- `crop_profile_for_window` returns the normalised profile only when `enabled` is true, else `None`.
- Editing UI: `GameCropDialog` (`ui/game_crop_dialog.py:144`), opened by `_edit_crop(index)` (main:2512) in the game-detection settings rule list. Nothing in main.py connects to `_edit_crop` (no button wired in this snapshot). It needs a live preview: `QApplication.primaryScreen().grabWindow(hwnd)` of the active/selected game window (hwnd from the matching active game window, else `target_hwnd`); if the grab is null the status text is `Start or select this game first so FTHR can capture a crop preview.` (error). Dialog: title `Crop profile — {game}` (header `CROP PROFILE · {GAME UPPER}`), 820x590, checkbox `Apply this crop when capturing this game` (default checked), readout `{x:.1f}%, {y:.1f}%  ·  {w:.1f}% × {h:.1f}%`, buttons `Reset to full frame`, `Cancel`, `Save profile`. Drag selects a rect in normalised preview coordinates; a release smaller than 0.01 in either axis resets to full frame. `profile()` returns `{'enabled', 'x','y','w','h'}` rounded to 6. Saved via `rule['crop_profile'] = dialog.profile()`, `_save_rules()` (re-normalises, writes `game_detection_custom_games`, saves settings); statuses `Crop profile saved and enabled.` / `Crop profile saved but disabled.`.
- Linux engine path (per the test and the launch ABI): crop was meant to be applied natively by the capture engine from `CaptureConfig.crop_*` (`core/capture_settings.py:71`), so Python's FFmpeg crop is a fallback/second path; `select_post_route(manual_crop=bool(crop_profile))` takes the argument but ignores it for routing (it returns `mic` or `finalize` only).

`_apply_crop(clip_path, ffmpeg, crop_profile)` algorithm:
1. Return unless `crop_profile` is a dict with `enabled` truthy (missing key counts as enabled). `crop_source = 'GameCrop'` for log prefixes.
2. `src_w, src_h = _clip_dimensions(clip_path, ffmpeg)` (main:8044; `ffmpeg -i`, stderr regex `(\d{3,5})x(\d{3,5})`, first match, fallback 1920x1080 with log `[Post] Could not probe clip dimensions: {exc}`; timeout 30 s). `float()` of x/y/w/h with defaults 0,0,1,1; `TypeError/ValueError` -> log `[GameCrop] invalid profile: {exc} — skipping`, warning `The game crop profile was invalid; the base clip was retained.`, return (no toast).
3. Pixels (round-half-even Python `round`, then forced even with `& ~1`):
   - `x = max(0, min(src_w-2, int(round(nx*src_w)))) & ~1`; `y` likewise with `src_h`.
   - `width = max(2, int(round(nw*src_w))) & ~1`; `height` likewise.
   - `width = max(2, min(width, (src_w - x) & ~1))`; `height = max(2, min(height, (src_h - y) & ~1))`.
4. No-op: if `x == 0 and y == 0 and width >= src_w-1 and height >= src_h-1` return (no re-encode, no log).
5. `crop = f'{width}:{height}:{x}:{y}'` (FFmpeg `crop=w:h:x:y`).
6. `tmp_dir = mkdtemp(prefix='.fthr-finalize-', dir=dirname(clip_path))`, `tmp_path = tmp_dir/output.mp4`.
7. Bitrate: `target = max(2500, min(50000, int(round(16000 * width*height / (1920*1080)))))` kbit/s; `video_args = software_video_args(bitrate_kbps=target)`. Reason in the source comment: OpenH264 rejects its fixed 16 Mbit/s default at small cropped sizes (bitstream exceeds the H.264 level limit).
8. Command (timeout 120 s, `capture_output`, no-window flag on Windows):
   ```
   ffmpeg -y -i {clip} -vf crop={w}:{h}:{x}:{y}
     -map 0:v:0 -map 0:a? -map_metadata 0 -map_chapters 0 -movflags use_metadata_tags
     {_cfr_video_args(clip)} {software_video_args(bitrate_kbps=target)}
     -c:a copy {tmp_dir}/output.mp4
   ```
   Differences from the overlays: `-map 0:v:0` (first video stream only; other video streams dropped), `-vf` not `-filter_complex`. `software_video_args` (ft:193): libopenh264 `-c:v libopenh264 -b:v {kbps}k -allow_skip_frames 0 -profile:v high` + seekable + SDR colour args, or x264 `-preset superfast -crf 18` (no bitrate) on a system FFmpeg, else `-c:v {enc} -b:v {kbps}k`. `_cfr_video_args` (main:7769): probes average FPS; `['-fps_mode','cfr','-r', fps]` (integer if within 0.005 of a whole number, else `%.6f` trimmed), `[]` if unprobeable/non-finite. All audio streams copied unchanged.
9. rc 0: `os.replace(output.mp4, clip_path)`; `rebind_manifest_after_media_replace(clip_path)`; log `[GameCrop] crop={crop} applied to {basename}`.
   rc != 0: log `[GameCrop] ffmpeg failed: {last stderr line or "(no stderr)"}`, warning `The game crop failed; the base clip was retained.`, toast (via `_ui_call`) `GAME CROP FAILED` / `The saved crop could not be applied. Clip saved uncropped.` level `warning`.
   Any exception (including `TimeoutExpired`): log `[GameCrop] Error: {e}`, same warning, same toast.
10. `finally`: remove `output.mp4` if present (FileNotFoundError swallowed), `rmdir(tmp_dir)` best effort.

Quirks: the crop re-encodes the whole clip in software even for a trivial trim; the 16 Mbit/s scaling floor of 2500 kbit/s can exceed the source's bitrate; `& ~1` on x/y/w/h can lose 1 px; dimension probing parses human stderr (can pick a wrong `WxH`); `GAME CROP FAILED` is the only post-step failure with a toast besides camera; a crop profile only ever applied to ONE game rule at a time (first match), and profile data is stored per rule not per resolution (normalised, so resolution independent).

## 17. Image layers at save (`_apply_image_overlay`, main:8059)

Settings (sm:276-285, all via `settings_manager`):
- `image_overlay_enabled` bool, default `False` (GLOBAL master switch for all layers).
- `image_overlays` list, default `[]` (ordered layers; the authoritative store).
- Legacy singular mirror, kept in sync with layer 0 by `_save_image_layers` (main:11915): `image_overlay_path` `''`, `image_overlay_opacity` `100`, `image_overlay_fit` `'fit'`, `image_overlay_rect` `{'x': 0.76, 'y': 0.04, 'w': 0.20, 'h': 0.24}` (= `DEFAULT_IMAGE_OVERLAY_RECT`, co:11). When layers become empty: `image_overlay_path = ''` and `image_overlay_enabled = False`.
- Migration on settings load (sm:381-392): if `image_overlays` is absent from the file but the file has a non-empty `image_overlay_path`, build one layer from `new_image_overlay_layer(path, 0)` updated with the file's enabled/opacity/fit/rect, store it as `image_overlays`, and mark `_retired_settings_removed` true (forces a rewrite). `test_visual_overlays.py:115` covers `{"image_overlay_enabled": true, "image_overlay_path": "hud.png", "image_overlay_opacity": 72, "image_overlay_fit": "fill"}` -> exactly one layer.
- Layer fields (`co:new_image_overlay_layer`, co:73): `path` (str), `enabled` (bool, default True), `opacity` (int percent, default 100), `fit` (`'fit'` or `'fill'`, default `'fit'`), `rect` (normalised `{x,y,w,h}`, default 0.20 x 0.24 at the slot below).
- New-layer start slots (`_IMAGE_LAYER_POSITIONS`, co:19), cycled by `index % 5`, top-left corner x,y: `(0.76,0.04) (0.04,0.04) (0.04,0.72) (0.76,0.72) (0.40,0.38)`. The index used is the layer count at add time. Rect passes through `clamp_overlay_rect`.
- `clamp_overlay_rect(rect, default)` (co:29): per field `float()` with the default on error; `w,h = clamp(v, 0.08, 0.92)`; `x = clamp(x, 0, 1-w)`, `y = clamp(y, 0, 1-h)`; each rounded to 4 decimals. Non-dict rect -> default. Default for images is `DEFAULT_IMAGE_OVERLAY_RECT` (not the camera's).
- `image_overlay_layers(settings)` (co:88) normaliser used by the burn and the UI: if `image_overlays` is `None` (absent, not `[]`) it falls back to the singular keys (only when `image_overlay_path` non-empty); non-list -> `[]`; skips non-dict items and items with empty `path`; opacity `int()` (error -> 100) clamped 10..100; `fit` is `'fill'` only if exactly `'fill'` else `'fit'`; rect clamped with the default for that layer index (`new_image_overlay_layer('', index)['rect']`, so a layer with a broken rect falls back to its slot rect, not the global default); `enabled = global_enabled and bool(item.get('enabled', True))` where `global_enabled = bool(get('image_overlay_enabled', bool(raw)))`.
- Paths are stored as absolute strings exactly as returned by the file dialog; no copy into the app's data dir, no relative-path or `~` handling. The image is read from that path at every save, so moving or deleting the file breaks the layer silently except for the finalization warning below.

Settings UI (Visuals page, "Visual Overlays", main:13020-13110, handlers main:11894-12090):
- Checkbox `Burn image layers into clips` (initial state `image_layers and image_overlay_enabled`); legacy page text was `Burn an image into clips`. Enabling with zero layers opens the picker first; if nothing valid is chosen the checkbox is unchecked and stored false.
- Picker `_choose_image_overlay`: `QFileDialog.getOpenFileNames`, title `Add image overlays`, filter `Images (*.png *.jpg *.jpeg *.webp *.bmp);;All files (*)` (NOTE: no `*.gif` here, unlike the Gary and preview-background pickers), start dir = parent of layer 0's path else home. Files are accepted only if `QPixmap(path)` is non-null; invalid ones are silently dropped; none valid -> returns False with no message. Each valid file appends `new_image_overlay_layer(path, len(layers))`, sets `image_overlay_enabled = True`, checks the box, selects the last new layer.
- Buttons `Add images`, `Remove` (selected layer; box becomes `bool(layers)`), `Clear all`; selector combo shows layers, empty text `No image layers`; counter `{n} IMAGE LAYER` / `{n} IMAGE LAYERS` (fixed width 92, right aligned).
- Fit combo `Inside frame` (`fit`) / `Fill frame` (`fill`) (legacy page `Fit inside frame`), label `Fit`; opacity slider 10..100 (label `Opacity`, value text `{n}%`); both act on the SELECTED layer and are disabled with no selection.
- Placement: layers are dragged in the shared `UnifiedOverlayPreview`; keys `image:{index}` in `rects_changed` are written back through `clamp_overlay_rect(.., DEFAULT_IMAGE_OVERLAY_RECT)` (`_on_input_overlay_rects_changed`, main:12082). The legacy `_on_image_overlay_rect_changed` writes only layer 0.
- Every edit calls `save_settings()` immediately.

Burn algorithm:
1. Return if `image_overlay_enabled` is falsey. `layers = [l for l in image_overlay_layers(sm) if l.enabled]`.
2. For each, `os.path.isfile(path)`; if false: warning `Image overlay source is missing: {filename}` (`Path(path).name`), and the layer is dropped. If none remain, return. Only existence is checked: an unreadable, zero-byte, corrupt, non-image or unsupported file passes this test and fails inside FFmpeg (see failure handling). No size/format validation, no per-layer error beyond the missing-file warning.
3. `clip_w, clip_h = _clip_dimensions(...)` (see section 16 step 2; same fallbacks).
4. Per valid layer `i` (input index `i+1`): args `-loop 1 -i {path}` (each image is looped as an infinite video input).
   - Pixels: `image_w = max(64, int(clip_w*rect.w)) & ~1`; `image_h = max(48, int(clip_h*rect.h)) & ~1`; `image_x = max(0, int(clip_w*rect.x))`; `image_y = max(0, int(clip_h*rect.y))`. `int()` truncates (no rounding); x/y not forced even; the 64x48 minimum can push the box past the frame edge (FFmpeg crops the overlap).
   - Opacity: `opacity = max(0.10, min(1.0, float(opacity)/100))`, formatted `{:.3f}`.
   - Shape string, fit: `scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=0x00000000` (letterbox centred, transparent padding). Fill: `scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}` (centre crop, `crop` defaults to centred).
   - Filter chain: `[{i+1}:v]format=rgba,{shape},colorchannelmixer=aa={opacity:.3f}[image{i}]`, then `[{previous}][image{i}]overlay={image_x}:{image_y}:eof_action=repeat:shortest=1[{out}]` where `previous` starts as `0:v` and becomes `layer{i}`, and `out` is `vout` for the last layer else `layer{i}`. Graph is the `;`-join of all of these. Later layers draw on top of earlier ones (list order = z-order, last on top).
5. `tmp_dir = mkdtemp(prefix='.fthr-finalize-', dir=dirname(clip))`, `tmp_path = tmp_dir/output.mp4`.
6. Command:
   ```
   ffmpeg -y -i {clip} {-loop 1 -i img1 -loop 1 -i img2 ...}
     -filter_complex "{graph}" -map [vout] -map 0:a? -map_metadata 0 -map_chapters 0
     -movflags use_metadata_tags {_cfr_video_args(clip)} {software_video_args()}
     -c:a copy {tmp_path}
   ```
   Timeout `max(120, 45 * n_layers)` seconds. Bitrate is the 16000k default (NOT scaled like the crop). Output is the same size as the input clip.
7. rc 0: `os.replace(tmp_path, clip_path)`, `rebind_manifest_after_media_replace(clip_path)`, log `[ImageOverlay] Applied {n} layer(s) to {basename}`.
   rc != 0: log `[ImageOverlay] ffmpeg failed: {last stderr line or "(no stderr)"}`, warning `Image overlay failed; the base clip was retained.`; NO toast.
   Exception (incl. timeout): log `[ImageOverlay] Error: {exc}`, same warning.
8. `finally`: remove `tmp_path` (FileNotFoundError ignored), `rmdir(tmp_dir)` best effort.

Image behaviours:
- Transparency: PNG/WEBP alpha survives `format=rgba` + transparent pad + `colorchannelmixer=aa`, then `overlay` composites with alpha (default `format=yuv420`, so the output is opaque H.264; the overlay math happens before encoding). JPG/BMP have no alpha and are opaque (the fit-mode padding is transparent so it does not draw black bars, unlike the camera PiP which pads black).
- Opacity multiplies existing alpha (`aa` = alpha-channel scale); 10% floor means a layer is never fully invisible.
- Animated GIFs: the picker filter excludes `*.gif`, but a GIF can be added via "All files" (and a legacy path could be one). `-loop 1 -i x.gif` loops the image demuxer; for a multi-frame GIF FFmpeg's gif demuxer with `-loop 1` repeats the GIF stream, so the layer may animate (unverified; no test, not designed for). `shortest=1` plus `eof_action=repeat` mean the base clip ends the output. Do not rely on GIF animation; a Rust port can treat GIF as a still (first frame) or implement animation deliberately.
- Missing file: warning only, other layers still burn. Unreadable/corrupt file: whole FFmpeg run fails (one bad layer fails ALL layers: the base clip is retained and only the generic `Image overlay failed; the base clip was retained.` is shown).
- Output length: `-loop 1` makes each image an endless stream; `overlay ...:shortest=1` ends the output when the shortest overlay input (the base clip) ends, so the result has the base clip's length. There is no `-shortest` output flag.
- Tests (`test_visual_overlays.py:128, 173`): 320x180 base, `fit` layer at default rect shows red at pixel (260,25); two `fill` layers (`rect` 0.72/0.04/0.24/0.30 and 0.04/0.04/0.24/0.30) burned in ONE pass, no warnings recorded. `test_multitrack_postprocessing_contract.py` asserts `'-map', '0:a?'`, `'-map_metadata', '0'`, `'-c:a', 'copy'` and `rebind_manifest_after_media_replace(clip_path)` appear in `_apply_image_overlay`, `_apply_camera_overlay` and `_apply_crop` bodies; a port must keep audio stems and manifest binding through every rewrite.

Manifest rebinding (`core/audio_manifest.py:132 rebind_manifest_after_media_replace`): only valid because every audio stream is copied in original order. If the sidecar is missing/invalid, or `media_file` differs from the clip name, returns False silently (legacy/imported clips). Otherwise sets `media_sha256` to the new file's hash and rewrites the sidecar atomically. Call is made after `os.replace`, per step (crop, image, keyboard, camera each rehash the whole file).

Quirks: all-or-nothing failure across layers; global flag and per-layer flag must both be true; `image_overlay_check` is the only way to disable all layers without deleting them; `int()` truncation on pixels vs `round()` in the crop; absolute paths never validated beyond `isfile`; a later `Clear all` also writes `image_overlay_enabled = False`, and removing the last layer via `Remove` also does (through `_save_image_layers`).

## 18. Post-save order and failure model

Order and routes. After the engine commits the file (`_on_save_outcome`, main:7095-7180): `clip_ready = upload_manager.notify_clip_saved(path, has_mic_mux=has_async_mux)`; `select_post_route` (main:449) ALWAYS returns `has_async_mux = True` (`('mic', True)` when mic audio is running or Windows native combined audio, else `('finalize', True)`), so every clip goes through a finalizer worker. Inside the worker the stage order is always: (mic or native audio mux, if the route calls for it) -> `_apply_crop` -> `_apply_image_overlay` -> `_apply_keyboard_overlay` -> `_apply_camera_overlay`. Sites: `_finalize_clip_worker` (main:8037, 8040, with per-stage `emit_event('clip_save', '<stage>_stage_completed', elapsed_ms=..)` for `crop`, `image_overlay`, `keyboard_overlay`, `camera_overlay` and a final `optional_finalization_worker_completed`), `_combine_native_audio_worker` (main:7762), `_mic_mux_worker` (main:8033). Each stage is a full software re-encode that replaces the clip; the next stage reads the previous stage's output (generation loss). The camera rect is therefore relative to the cropped frame and sits on top of image layers.

Readiness / "finalizing" display:
- `ClipReadinessRegistry` (`core/clip_readiness.py`): states `ENGINE_COMMITTED, FINALIZING, READY, READY_WITH_WARNING, FINALIZATION_FAILED`. `upload_manager.notify_clip_saved` registers the path as `FINALIZING` (since `needs_finalization` is true) and returns the handle's `threading.Event` as `clip_ready`. `record_warning(path, msg)` appends a string; `complete()` -> `READY_WITH_WARNING` if any warning else `READY` and sets the event; `finalization_failed(path, msg, base_clip_usable)` -> `READY_WITH_WARNING` (usable) or `FINALIZATION_FAILED`; `can_access` is true only for READY and READY_WITH_WARNING; an untracked existing file is READY, a missing untracked file is FINALIZATION_FAILED.
- `_record_finalization_warning(clip_path, message)` (main:7187) is just `_clip_readiness.record_warning`. The crop/image/keyboard/camera failure strings are all recorded as warnings (clip stays usable); none of them set FINALIZATION_FAILED.
- Library card: `clip_grid.upsert_saved_clip(path, ready=can_access(path))` right after commit inserts the card NOT ready; `ClipCard.set_ready(False)` shows a `Finalizing` badge (`QLabel#cardFinalizingBadge`, top-left at 8,8, `Colors.BG` text on `Colors.WARNING` background, 12 px body font, radius `RADIUS_SM`, padding 3px 7px), disables the delete button and the title edit, and `_update_share_action` blocks sharing. On publication the card is upserted again with `ready=True`, which deletes the badge. Opening a not-ready card shows toast `CLIP STILL FINALIZING` / `Playback, editing and upload become available after optional processing has finished.` (level `warning`, main:8992).
- Status bar: `_set_status('SAVING', ...)` while `has_async_mux`; `_publish_final_clip` (main:7208, runs once per path, on the UI thread via `_ui_call`) sets `SAVED` (after 2 s `_update_status`), or, if any warning was recorded, status `SAVED WITH WARNING` and toast `CLIP SAVED WITH WARNING` with `warnings[-1]` (only the LAST warning is shown; level `warning`), or, if the file is unusable, status `FINALIZATION FAILED` and toast `CLIP FINALIZATION FAILED` / `The final clip file is unavailable. The base save was not reported as a completed clip.` (level `error`). `clip_saved` is emitted only here.
- `_complete_clip_finalization` (main:7191): unless already normalised, runs the CFR normalise; if the readiness state is FINALIZATION_FAILED it publishes the failure; else if the file exists and is non-empty -> `complete()` (event set), otherwise `finalization_failed(..., 'The final clip file is missing or empty.', base_clip_usable=False)`.
- `clip_ready.set()` is in a `finally` of every worker, so the upload worker is always released even if a stage raises. If the committed clip is missing or empty at worker start, `_finalize_clip_worker` raises `OSError('The committed clip is missing or empty.')` (handled by the outer path, which sets the event). If FFmpeg is unavailable (`FFmpegUnavailable`): log `[Finalize] {e} — skipping post-processing` and warning `Optional processing was skipped because FFmpeg is unavailable.`. `_allow_completed_clip_pipeline` refuses non-`is_completed_video_path` paths (log `[Post] Refused incomplete clip path: {name}`) and sets the event.

Failure model summary:
- Every step is best-effort and non-destructive: output goes to `.fthr-finalize-*/output.mp4` in the clip's own directory (same filesystem so `os.replace` is atomic) and only replaces the clip on rc 0; the clip is never edited in place. On any failure the previous file remains.
- User-visible failure strings: crop invalid profile `The game crop profile was invalid; the base clip was retained.`; crop fail `The game crop failed; the base clip was retained.` + toast `GAME CROP FAILED`; image missing `Image overlay source is missing: {name}`; image fail `Image overlay failed; the base clip was retained.` (no toast); camera strings in section 8. All surface through the single `CLIP SAVED WITH WARNING` toast (last one only) plus log lines with prefixes `[GameCrop]`, `[ImageOverlay]`, `[Camera]`, `[Post]`.
- Temp dirs are removed in `finally`; a crash leaves `.fthr-finalize-*` dirs next to clips (dot-prefixed; make sure the Rust library scanner ignores them and consider sweeping stale ones at startup; the reference's scan behaviour was not checked here).
- Stages are independent: a failed crop does not stop images/keyboard/camera.

Quirks not to copy: the whole chain re-encodes up to four times with 120 s per-step timeouts and no cancel; only the last warning is shown; `has_async_mux` is always True so the `SAVING` status and `Finalizing` badge appear even when no visual option is on (cost is just the CFR check then); the crop step is stubbed so a "crop" setting would silently do nothing; dimension probing by stderr regex; manifest rebinding is repeated per stage; a Rust port should build one filter graph (crop -> images -> keyboard -> camera) and one re-encode, keep `-map 0:a? -c:a copy`, keep temp-in-same-dir plus atomic rename, and rebind the manifest once.
