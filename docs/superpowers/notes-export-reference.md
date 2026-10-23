# Clip export: Python reference notes

Paths are relative to `reference/FTHR_UI/`. `cv` = `ui/clip_viewer.py`, `eq` = `core/export_queue.py`, `el` = `core/export_lifecycle.py`, `ee` = `core/export_encoding.py`, `ft` = `core/ffmpeg_tools.py`. Line numbers are approximate to the line quoted.

## 1. Entry points and what exists

- Editor sidebar (in order): "Share clip" (`share_btn`, cv:5115, opens the share picker), quality combo, "Export" (`export_btn`, cv:5134, calls `_export_clip`), "Export jobs" (cv:5140, opens `show_export_jobs`), "Upload". Shortcut Ctrl+E (`_export_now`, cv:8902) just calls `_export_clip`, it is never a cancel.
- Two export code paths exist for a finished edit:
  1. Editor "Export": `_export_clip` (cv:8008) -> `export_queue.submit` (eq:119). Tracked in the Export jobs window. Output in `{clips root}/Exported`.
  2. "Share clip": `ShareModeDialog` (cv:3356) -> `ShareWindow` (cv:3524) -> `export_queue.run_export` directly (eq:63). Not listed in the Export jobs window. Output in `{clips root}/Shared`.
- Both build their FFmpeg command with the same method, `ClipViewer._build_export_cmd` (cv:8039). `ShareWindow` calls it unbound with itself as `self` (duck typing: it carries `clip_path`, `_speed_rate`, `_preserve_pitch`, `_audio_tracks`, `_playback_sources`, `_source_volumes`, `_source_mutes`, `_master_volume`, `_watermark_enabled`).
- Dead legacy path: `ClipViewer._export_worker` / `_export_worker_impl` / `_on_export_done` / `_reset_export_button` (cv:8219-8370) use `maximum_quality_video_args` and a per-viewer `ExportJob`. Nothing calls `_export_worker` any more (the only `threading.Thread(target=self._export_worker...)` is `ShareWindow.showEvent`, cv:3777, which is a different method). Consequently, the editor button strings "Export cancelled", "exported", "Export failed" and the red `crop_info_lbl` error text are unreachable; after a successful submit the button just reads "Export clip" (cv:8036).
- Webcam picture-in-picture and the "Gary" overlay are NOT part of any export. See section 12.

## 2. Settings keys and defaults

Defaults live in `core/settings_manager.py` (`default_settings`, lines ~145-300). Read with `sm.get(key, fallback)`.

| Key | Default | Used by |
|---|---|---|
| `export_quality` | `'quality'` | editor combo, share dialog; valid `fast`/`quality`/`lossless`, anything else normalizes to `quality` (ee `normalize_quality`) |
| `parallel_exports` | `False` | `configure` (eq:24) |
| `parallel_export_limit` | `2` | `configure`; clamped to 2..4 |
| `watermark_enabled` | `False` | read at command-build time; forces a video re-encode |
| `clips_directory` | platform default clips root | `clips_directory_from(sm)` (settings_manager:68); `Exported` and `Shared` are two of its five standard subfolders (`_CLIPS_SUBDIRECTORIES` = Desktop, Recordings, Exported, Shared, Screenshots) |
| `selected_export_preset` | `'discord'` | last custom preset in the share dialog |
| `master_volume` | `80` | editor master gain (0..100); `ShareWindow` receives the live value |
| `source_volumes` | `{game:100, browser:100, music:100, discord:100}` | legacy per-category values; the real per-source state is the viewer's `_source_volumes` / `_source_mutes` |
| `clip_editor_live_preview` | `True` | live preview master switch (section 11) |
| `editor_color_preview_enabled` | `True` | colour effects shown in preview (section 11); not in the defaults dict, only in `core/ui_performance.py:12` and read with fallback `True` |
| `resolution` / `bitrate_level` / `framerate` | `'source'` / `'medium'` / `60` | passed into `settings_info` for the share window but never read there (only `export_quality`, `clips_directory`, `watermark_enabled` are read, cv:3853/3864/3846) |
| `gary_mode_enabled` / `gary_min_level` / `gary_max_level` / `gary_image_path` | `False` / `55` / `85` / `None` | capture-side only |
| `camera_enabled` / `camera_device_index` / `camera_position` / `camera_size` / `camera_overlay_rect` | `False` / `0` / `'bottom-right'` / `'medium'` / `{x:.72,y:.64,w:.25,h:.34}` | capture-side only |

Preset file: `~/.fthr/export_presets.json` (JSON list of `ExportPreset` dicts, atomic write via `.tmp` + `os.replace`).

Settings page "Clip exports" section (main.py:13550): checkbox "Allow simultaneous exports" (enables the limit combo), combo accessible name "Maximum simultaneous exports" with items "2 exports at once", "3 exports at once", "4 exports at once", hint "Off by default: clips export one at a time. You can keep browsing and queue more clips. Simultaneous exports use more CPU, GPU, memory and disk bandwidth and can affect recording or games. Lowering the limit lets running jobs finish before starting more.", button "View export jobs". Every change saves settings and calls `configure(self.sm)`.

## 3. Queue model (`core/export_queue.py`)

**Records.** `ExportTask` dataclass (eq:102): `source: str`, `output: str`, `quality: str`, `status: str = 'Queued'`, `progress: float = 0`, `detail: str = ''`, `cancel: threading.Event`, `thread`, `done: bool = False`. Module globals: `_records` (list), `_waiting` (FIFO tickets), `_active` (running count), `_limit = 1`, one `threading.Condition`.

**`submit(source, directory, quality, build_command, duration, validate)`** (eq:119), called on the GUI thread:
1. suffix `.mkv` if `quality == 'lossless'` else `.mp4`.
2. `output = directory / f'{source.stem}_export_{uuid4().hex[:12]}{suffix}'`.
3. `staged = create_staged_output_path(output)` (section 8).
4. `template = tuple(build_command(str(staged)))`. The FFmpeg command is fully built here, so the worker never touches editor state.
5. Task is created, the pruning rule runs, the task is appended, a daemon thread `fthr-export` starts running `work()`.

**Pruning** (eq:140): `_records[:] = [i for i in _records[:-50] if not i.done] + _records[-50:]`. The newest 50 records are always kept; older ones survive only if still running. Then the new task is appended, so the list can hold 51.

**`work()`**: calls `run_export(...)` with `progress=lambda v: task.progress = v` and `status=lambda v: task.status = v`. On return: `task.status = result.state.value.title()` (so `Completed`, `Failed`, `Cancelled`, `Timed_Out`; note the underscore), `task.detail = '' if succeeded else (result.stderr_tail or result.detail)`, `task.progress = 100` only when `COMPLETED`. Any exception: `status='Failed'`, `detail=str(error)`. `finally`: `discard_staged_output(staged)`, `task.done = True`.

**Status strings a task can show, in order:** `Queued` (set by `run_export` before waiting for a slot) -> `Preparing` (slot acquired) -> `Exporting · NVIDIA hardware` | `Exporting · AMD hardware` | `Exporting · Intel hardware` | `Exporting · lossless` (ffv1) | `Exporting · software` | `Exporting · stream copy` (template has no encoder placeholder) -> optionally `Software fallback · software` (second attempt) -> terminal `Completed` / `Failed` / `Cancelled` / `Timed_Out`. The jobs window may overwrite with `Cancelling` (section 9).

**Shutdown** (eq:154): `shutdown(timeout=12)` sets `cancel` on every non-done task, then joins each thread against a shared monotonic deadline of 12 s. Called from `_perform_full_shutdown` (main.py:9477) right after closing every open `ShareWindow` (`_share_windows` set).

## 4. Concurrency and admission

- `configure(settings)` (eq:24): `requested = int(settings.get('parallel_export_limit', 2))` (bad value -> 2); `_limit = max(2, min(4, requested)) if settings.get('parallel_exports', False) else 1`; `notify_all`. Called with `self.sm` on every `_export_clip` and share-window creation (`configure(self.sm or {})`), and from the settings page. Module default before any call: 1.
- `export_slot(cancel)` (eq:37) is a context manager yielding `admitted`. A unique ticket joins `_waiting`; it is admitted only when `_active < _limit` AND it is at the front (`_waiting[0] is ticket`), so admission is strict FIFO across editor exports and share exports together. Waits poll with `Condition.wait(.1)`. If `cancel` is set while waiting, the ticket is removed and `admitted=False`; `run_export` then returns `ExportResult(CANCELLED, 'Export cancelled')`.
- Lowering the limit never kills running jobs; it only delays new admissions (matches the settings hint).
- The share window's `run_export` call goes through the same slot, so a Share counts against the limit even though it is not in the jobs list.

## 5. Process lifecycle (`ExportJob`, `core/export_lifecycle.py`)

**States** (`ExportState`, str enum): `IDLE, REQUESTED, PREPARING, EXPORTING, FINALIZING, COMPLETED, FAILED, CANCELLED, TIMED_OUT`. Terminal: the last four. Transition table: IDLE->REQUESTED; REQUESTED->PREPARING/CANCELLED/FAILED; PREPARING->EXPORTING/CANCELLED/FAILED/TIMED_OUT; EXPORTING->FINALIZING/CANCELLED/FAILED/TIMED_OUT; FINALIZING->COMPLETED/CANCELLED/FAILED/TIMED_OUT. An illegal transition raises `RuntimeError('illegal export transition A -> B')`. `ExportResult(state, detail='', returncode=None, stderr_tail='', elapsed_seconds=0.0)`; `succeeded` = `COMPLETED`.

**Constructor defaults:** `overall_timeout=None` (none), `inactivity_timeout=120.0` (also passed explicitly as 120 by `run_export`, eq:90), `cancel_wait=2.0`, `stderr_limit=8192` (min 256), `popen_kwargs` (Windows only: `CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS`).

**Progress args.** `_with_progress_args` inserts `-progress pipe:1 -nostats` immediately before the last argument (the output path) unless `-progress` is already present.

**Run loop.**
1. REQUESTED; if cancel already set -> CANCELLED (`'Export cancelled'`). PREPARING; same check.
2. `Popen(command, stdout=PIPE, stderr=PIPE, text=True, encoding='utf-8', errors='replace')`. A launch exception -> FAILED with `str(error)`.
3. EXPORTING. Two daemon reader threads (`fthr-export-stdout`, `fthr-export-stderr`). stdout lines go into a `queue.Queue(maxsize=512)`; if full the oldest is dropped (newest progress wins). stderr is kept as a bounded tail (drops oldest lines while total chars > 8192) and never touches the progress queue.
4. Main loop, polling every 0.05 s on `progress_events.get(timeout=0.05)`: cancel set -> terminate and reap -> CANCELLED; process exited -> leave loop; inactivity >= 120 s since the last accepted progress line (or `overall_timeout`) -> terminate and reap -> TIMED_OUT with detail `'Export timed out'`.
5. Terminate and reap: `terminate()`, `wait(2.0)`, then `kill()`, `wait(2.0)`.
6. After exit: cancel set -> CANCELLED. Return code != 0 -> FAILED with `f'FFmpeg exited with code {returncode}'`.
7. FINALIZING: staged file must exist with size > 0 else `RuntimeError('Export produced no usable output')`; run the validator (section 7); re-check cancel; `commit_output(staged, final)`; discard staged; final must exist with size > 0 else `RuntimeError('Export finalization produced no usable output')`; `_complete_after_commit` arbitrates a late cancel under the state lock (cancel wins and the result is CANCELLED, final file deleted unless it pre-existed).
8. Any exception: cancel set -> CANCELLED; else FAILED with `str(error)`; staged discarded, and a newly created final is deleted (`_final_existed_at_start` protects a pre-existing file).
9. `finally`: kill any live child, close pipes, join readers 0.25 s each.

**`request_cancel()`** only sets the event (the worker terminates the child); `cancel()` also terminates directly. The share window calls `job.cancel()` only if it holds `_export_job`, but it sets `_export_cancel` which the loop sees within 50 ms.

## 6. Progress parsing and expected duration

`_handle_progress(line, duration)` (el):
- `key, '=', value = line.strip().partition('=')`. Only `out_time_ms` and `out_time_us` are used (both divided by 1,000,000; FFmpeg's `out_time_ms` is actually microseconds, so the same divisor is correct). `progress=end` counts as activity but yields no percent.
- A value that is non-integer, lower than, or equal to the previous accepted value is ignored and does NOT reset the inactivity clock.
- `percent = min(99.0, max(0.0, seconds / duration * 100.0))` if `duration > 0`, else the raw seconds value. 99 is the cap while running; the queue sets 100 only after `COMPLETED`.
- The jobs window shows `max(0, min(100, round(task.progress)))`.

**Expected duration passed to `submit`:** `_speed_adjusted_duration(sum(end - start for kept ranges), self._speed_rate)` = `max(0.1, kept_seconds / _normalise_speed(speed))` (cv:897). Speed is clamped to 0.25..2.0 (`_normalise_speed`, cv:~838; non-finite or <= 0 -> fallback 1.0), so progress matches the output timeline, not the source. Share passes `duration_s` computed the same way (`max(sum, 0.1)` then speed-adjusted); share ignores progress (`progress=None`).

**Empty-edit guard** (cv:8011): `segments = _kept_segments_seconds()`; if none, or total kept < 0.1 s -> `export_error.emit('EXPORT FAILED', 'The edit has no kept segments.', 'warning')` and return. Kept ranges come from `TrimSlider.kept_ranges()` (cv:1371): each non-deleted segment clipped to the trim handles, pieces <= 0.0001 (fraction) dropped; ms = `int(pct * duration_ms)`, seconds = ms / 1000. Adjacent kept segments are NOT merged, so a split without a delete still yields two ranges and therefore the concat path (section 10).

## 7. Encoder selection (`core/export_encoding.py`)

**Quality options** (`QUALITY_OPTIONS`): ("Very fast", `fast`), ("High quality", `quality`), ("Lossless · MKV", `lossless`).

**Template placeholder.** Callers write `VIDEO_PLACEHOLDER = ['-c:v', 'fthr_export_encoder']` into the command. `encode_command(template, args, quality)` (ee) replaces that two-element pair with the chosen args, then:
- inserts `['-threads', '2', '-filter_threads', '2', '-filter_complex_threads', '2']` at index 1 (right after the ffmpeg exe, before `-y`);
- inserts `['-threads:v', '2']` before the last element (output path; `-progress pipe:1 -nostats` is inserted after this by `ExportJob`, so the tail reads `-threads:v 2 -progress pipe:1 -nostats <out>`);
- if `quality == 'lossless'` and `-c:a` present and its value is not `copy`: set it to `flac` and delete the `-b:a <rate>` pair.

**`choose_video_args(ffmpeg, quality, bitrate=None, hardware=True, cancel=None)`:** for non-lossless, tries `h264_nvenc`, `h264_amf`, `h264_qsv` in that order. Each candidate is verified by running (timeout 8 s, results cached per `(ffmpeg, quality, encoder, bitrate is not None)`, probes serialized by a lock polled at 0.1 s):

`ffmpeg -v error -f lavfi -i color=s=128x128:r=30 -frames:v 1 <candidate args> -f null -`

First one that exits 0 wins. If none, or cancel is set, `software_args`.

**Hardware argument sets** (`video_args`, `fast` vs `quality`). Common tail on all hardware: `-pix_fmt yuv420p -g 30 -bf 0 -color_range tv -colorspace bt709 -color_primaries bt709 -color_trc bt709 -bsf:v h264_metadata=video_full_range_flag=0:colour_primaries=1:transfer_characteristics=1:matrix_coefficients=1`.
- `h264_nvenc`: `-preset p1` (fast) / `p5`, `-tune hq -rc vbr`, then `-b:v {bitrate}k` if a bitrate is given else `-cq 23` (fast) / `18` plus `-b:v 0`.
- `h264_amf`: `-quality speed` / `quality`; with bitrate `-rc vbr_peak -b:v {bitrate}k`, else `-rc cqp -qp_i 23|18 -qp_p 23|18`.
- `h264_qsv`: `-preset veryfast` / `medium`; with bitrate `-b:v {bitrate}k`, else `-global_quality 23|18`.

**Software** (`software_args`): lossless -> ffv1. With a bitrate (share size presets): `size_constrained_video_args(bitrate)` (software args with any `-crf` pair removed, `-b:v {n}k` appended if absent). Otherwise `software_video_args(16000 if fast else 32000)`. For `libopenh264` a `-coder cavlc` (fast) / `cabac` is appended. `software_video_args(bitrate_kbps)` (ft) picks the encoder by probing `ffmpeg -hide_banner -encoders` (timeout 20 s; preference `libopenh264`, `h264_mf`, `libx264`; probe failure -> `libopenh264`):
- `libopenh264`: `-c:v libopenh264 -b:v {n}k -allow_skip_frames 0 -profile:v high` + `-g 30 -bf 0` + the SDR colour args above.
- `libx264`: `-c:v libx264 -preset superfast -crf 18` + the same tail (bitrate ignored).
- other (`h264_mf`): `-c:v {enc} -b:v {n}k` + tail.

**Lossless:** `-c:v ffv1 -level 3 -coder 1 -context 1`, no `-pix_fmt`, no colour args; container `.mkv`; audio `flac`.

**Hardware failure retry** (`run_export`, eq:63): if the chosen args contain a hardware encoder name, the loop runs up to 2 attempts; a `FAILED` first attempt (not cancelled) retries with `software_args(quality, ffmpeg, bitrate)`, status `Software fallback · software`. Timeouts and cancels are not retried.

**Unused helper:** `maximum_quality_video_args` (ft:250: libx264 `-preset veryslow -crf 0`, libopenh264 `-b:v 200000k -allow_skip_frames 0 -profile:v high`, other `-b:v 200000k`) is used only by the dead legacy path.

## 8. Transactional output (`core/transactional_output.py`)

- `create_staged_output_path(final)`: `final.with_name(f'.{final.stem}.partial.{uuid4().hex}{final.suffix}')` (hidden dotfile, same directory, same extension so FFmpeg picks the right muxer).
- `commit_staged_output`: raises `ValueError('staged and final output must be in the same directory')` if parents differ after `resolve()`, `FileNotFoundError(staged)` if missing, then `os.replace(staged, final)`.
- `discard_staged_output`: unlink, ignoring `FileNotFoundError`.
- Staged paths are discarded in `ExportJob` result paths, in `work()`'s `finally`, and in the share window's `closeEvent` (`os.remove(pending)`).
- Library hand-off: there is none beyond the file landing on disk. `Exported` and `Shared` are excluded from the main library grid (`_GRID_EXCLUDED_DIRS`, `ui/clip_grid.py:96`; library index skips them, `core/library_index.py:237`). The editor does not notify the library or auto-open the result; the jobs window "Open output folder" and the share window's drag zone are the only ways to reach the file.

## 9. Export jobs window (`ui/export_jobs.py`)

- Singleton `ExportJobsDialog(FthrDialog)`, title "Exports", `width=620` in the base, `resize(720, 400)`, `setMinimumHeight(280)`, non-modal. Opened by `show_export_jobs()` (show, raise, activate).
- Hint: "Keep browsing while your clips export. Closing this window keeps exports running."
- Empty state label: "No export jobs yet. Start an export from a clip." (shown instead of the list when `tasks()` is empty).
- Buttons (primary style): "Cancel selected", "Open output folder".
- Refresh: a 500 ms `QTimer`, started in `showEvent` (and an immediate `refresh()`), stopped in `hideEvent`. Rows are rebuilt only when the tuple of task `id`s changes; selection is preserved by task identity, default row 0. Each row item has size hint `QSize(0, 68)`.
- Row layout (margins 12,10,12,10; spacing 16): filename `Path(source).name` (14 px, tooltip = name), status line (11 px dim) = `f'{quality label} · {task.status}'` where the label is the `QUALITY_OPTIONS` display name ("Very fast", "High quality", "Lossless · MKV"), status tooltip = `task.detail or task.output`; percent label fixed width 42 right-aligned (`f'{percent}%'`); progress bar 110x8, range 0..100, no text, accessible name `Export progress for {filename}`.
- `update_actions`: Cancel enabled only if a row is selected and `not task.done`; Open enabled with any selected row.
- `cancel_selected`: `task.cancel.set()` and `task.status = 'Cancelling'` (overwritten when the worker finishes with `Cancelled`).
- `open_selected`: `QDesktopServices.openUrl(QUrl.fromLocalFile(str(Path(task.output).parent)))`.
- Editor button label (`_update_export_jobs_status`, cv:7990; refreshed on a 1000 ms timer): `pending = count(not done)`, `failed = count(done and status in {'Failed','Timed_Out'})`, `completed = count(status == 'Completed')`; suffix ` · {n} pending` if pending, else ` · {n} failed`, else ` · {n} completed`, else none. `Cancelled` jobs are not counted.
- Errors are not listed anywhere else for queued exports: the failure text is the row's status tooltip (`stderr_tail` last 8192 chars, else the lifecycle detail).

## 10. Command construction (`_build_export_cmd`, cv:8039)

Signature: `(ffmpeg, start_s, duration_s, out_path, crop_rect, video_args, segments=None, effects=None, stretch_ratio=1.0, audio_bitrate='192k', extra_video_filters=None, force_video_encode=False, force_audio_encode=False)`. The editor Export passes `audio_bitrate='320k'`, `segments=kept ranges`, `effects=dict(self._effects)`, `stretch_ratio=self._stretch_ratio`, `video_args=VIDEO_PLACEHOLDER`, and uses `segments[0][0]` / `segments[-1][1] - segments[0][0]` for start/duration.

Segments with `end - start < 0.001` are dropped; none left -> `ValueError('at least one non-empty segment is required')` (surfaced as `EXPORT FAILED` with that text).

**Video filter chain** (`_video_filter_chain(crop_rect, effects, stretch_ratio, ensure_even=False, speed_rate)`, cv:916), in this exact order, each present only when needed:
1. `crop={w & ~1}:{h & ~1}:{x}:{y}` (width and height forced even; x, y unchanged).
2. Exposure/contrast (if either non-zero): `curves=all=0.000/{a} 0.500/{b} 1.000/{c}` with `brightness = exposure * 0.003`, `contrast = 1.0 + contrast * 0.006`, point = `clamp((s - 0.5) * contrast + 0.5 + brightness, 0, 1)` for s in 0.0, 0.5, 1.0, three decimals (comment: bundled FFmpeg lacks `eq`).
3. `hue=s={1.0 + saturation * 0.01:.3f}`.
4. `colorbalance=rs={t:.3f}:bs={-t:.3f}` with `t = temperature * 0.003`.
5. `unsharp=5:5:{sharpness * 0.015:.3f}:5:5:0.0`.
6. `vignette=PI/{20.0 - vignette * 0.15:.2f}`.
7. If `|ratio - 1| > 0.005` (ratio clamped 0.25..4.0): `scale=trunc(iw*{ratio:.4f}/2)*2:trunc(ih/2)*2` then `setsar=1`; elif `ensure_even`: `scale=trunc(iw/2)*2:trunc(ih/2)*2` (never set by export callers).
8. If speed != 1: `setpts=PTS/{speed_text}` (`_speed_text` = `f'{v:.6f}'` stripped of trailing zeros and the dot).
Effect values are clamped via `_normalise_color_effects` (exposure/contrast/saturation/temperature -100..100, sharpness/vignette 0..100, bad values -> 0). Preset filters (`extra_video_filters`, share only) are appended after the chain: `fps={n:g}` if the target FPS is lower, `scale={w}:{h}:force_original_aspect_ratio=decrease` if the size changes, then always `scale=trunc(iw/2)*2:trunc(ih/2)*2`.

**Audio speed chain** (`_speed_audio_chain`, cv:~885): preserve pitch (default) -> `_atempo_chain`: repeats `atempo=2` while remaining > 2.0, `atempo=0.5` while < 0.5, then `atempo={speed_text(remaining)}` if not 1.0, joined by commas; none at 1.0. Pitch changing -> `_pitch_changed_chain`: `aresample=48000,asetrate={round(48000 * speed)},aresample=48000`, none at 1.0.

**Audio mix filter** (`ffmpeg_mix_filter(sources, states, master_gain, output_label='aout')`, `core/playback_mix_model.py:280`): selected = sources with `audio_index is not None and available`; none -> `([], None)`. Per source `[0:a:{audio_index}]volume={gain:.6f}[src{i}]` where gain = 0 for unavailable, 1.0 for `mix_role == 'base'`, `gain - 1.0` for `'delta'`, else `gain_percent/100` clamped 0..100 (0 when muted). Then `[src0][src1]...amix=inputs=N:normalize=0,volume={1/sqrt(active):.6f},alimiter=limit=0.98:level=disabled,volume={master/100:.6f}[{label}]` where `active` counts sources with |gain| > 1e-6 and master = clamp(0..100)/100. Output audio is AAC at the passed bitrate (`-c:a aac -b:a 320k` for the editor).

`use_video_filter = bool(video_filters) or watermark_enabled`; `use_audio_filter = bool(self._audio_tracks)` (clip has manifest-identified tracks). `watermark_enabled` is read from `self.sm.get('watermark_enabled')` when the object has `sm` (editor), else from `self._watermark_enabled` (share window).

### 10a. More than one kept range (concat path)

`[ffmpeg, -y, -i, clip]` then one `-filter_complex`:
- Per range i: `[0:v]trim=start={s:.6f}:end={e:.6f},setpts=PTS-STARTPTS,{video_filters...}[v{i}]`.
- Audio, mix case (`use_audio_filter`): `ffmpeg_mix_filter(..., output_label='amixed')` snippets, then `[amixed]asplit=N[amix0]...[amix{N-1}]`, then per range `[amix{i}]atrim=start={s:.6f}:end={e:.6f},asetpts=PTS-STARTPTS[a{i}]`. No manifest tracks but playback sources exist: `[0:a:0]asplit=N[adirect0]...` and `[adirect{i}]atrim=...,asetpts=PTS-STARTPTS[a{i}]`.
- Concat: `[v0][a0][v1][a1]...concat=n=N:v=1:a=1[{vlabel}][aout]`, or video only `[v0][v1]...concat=n=N:v=1:a=0[{vlabel}]`. `vlabel` is `vbase` when the watermark is on, else `vout`.
- Watermark filters appended after the concat (section 13).
- If audio speed applies, `[aout]{speed_audio}[aspeed]` is added and re-joined into the graph (so speed changes audio after concat; video speed is applied per range via `setpts`).
- Args: `-map [vout]`, `-map [aout]` or `[aspeed]`, then `video_args`, then `-c:a aac -b:a {rate} -shortest` (only if there is audio), then the output.
- No `-ss`/`-t`; always re-encodes video.

### 10b. Exactly one kept range

Base: `[ffmpeg, -y, -ss, str(start), -i, clip, -t, str(_speed_adjusted_duration(end - start, speed))]`.
- No video filters, no watermark, no audio mix: stream copy `-map 0:v? -map 0:a? -c copy <out>` (video/audio re-encode only when `force_video_encode` / `force_audio_encode`, from a size preset).
- Otherwise: `-filter_complex` joining: `[0:v]{chain or 'null'}[vbase|vout]` (+ watermark filters when on), plus the audio mix snippets and `{audio_output}{speed_audio}[aspeed]` when both apply. Maps: video `[vout]` if a video filter graph exists else `0:v:0`; audio `[aspeed]` / the mix output label / `0:a?`. Video codec: `video_args` if filtered or forced, else `-c:v copy`. Audio: if mixing, speeding or forced -> `-af {speed_audio}` (only when speed applies without a mix) and `-c:a aac -b:a {rate}`; else `-c:a copy`.

### 10c. Share command selection (cv:3880-3935)

In order: (1) a size preset -> `_build_export_cmd` with `video_args = [*VIDEO_PLACEHOLDER, '-maxrate', f'{int(kbps * 1.15)}k', '-bufsize', f'{kbps * 2}k']`, `audio_bitrate=f'{plan.audio_bitrate_kbps}k'`, preset filters, force flags from the plan; (2) more than one range, or any non-zero effect, or watermark on, or stretch beyond 0.005, or speed != 1 -> `_build_export_cmd` at `320k`; (3) otherwise a simple crop and/or mix: `-ss start -i clip -t duration -filter_complex [0:v]crop=...[vout];...` with `-c:v` placeholder or `copy`, audio `-c:a aac -b:a 320k` or `copy`; (4) otherwise `-map 0:v? -map 0:a? -c copy`. The branch-3 mix is built by `_build_audio_filter_chain`, active only when the clip has multitrack audio and `_preserve_tracks` is False (that is `discord_mode`).

## 11. Output naming and collisions

- Editor: `{Exported}/{source stem}_export_{12 hex}.mp4` (`.mkv` for lossless). Directory created with `mkdir(parents=True, exist_ok=True)`. The 12-hex UUID slice is the only collision protection (no existence check).
- Share: `{Shared}/{stem}_share_{HH-MM-SS}_{8 hex}.mp4`, or `_discord_` instead of `_share_` when `discord_mode`; `.mkv` when the raw `settings['export_quality'] == 'lossless'`. Then `while out_p.exists(): out_p = {stem}{suffix without ext}_{n}{ext}` with `n` from 2. Directory-creation failure emits `f'Cannot create {share_dir}: {e}'` through `_export_sig` (red filename label, "Failed" badge) and `EXPORT_INIT_FAILED` diagnostics.
- Lossless + any size preset: `ValueError('Lossless cannot be combined with a file-size preset.')` (caught by the worker wrapper, shown as the failure text). The share dialog already blocks it (section 12).

## 12. Share flow (`ShareModeDialog` cv:3356, `ShareWindow` cv:3524)

**Picker** (frameless Tool dialog, fixed 330x350, centered on the editor; emits `export requested` diagnostics). Header label "Share as" with a close button. Contents in order:
- Quality combo (accessible name "Share quality"), preselected from `export_quality`.
- Hint: "Lossless uses MKV and may not preview in chat apps. Size presets require Very fast or High quality."
- Button "Share clip" (40 px high, full quality).
- Button "20 MB  ·  Discord" on Windows, "10 MB  ·  Discord" otherwise (two spaces around the middle dot). Discord preset: `ExportPreset('discord', 'Discord · 20 MB' | 'Discord · 10 MB', target_size_mb 20/10, target_fps 60, h264, audio 96 kbps, safety_margin 0.93, built_in)`.
- If custom presets exist: a combo (their names) with a "Use" button (88x34), preselecting `selected_export_preset`.
- Choosing a size preset (Discord or custom) with Lossless shows an information dialog "Choose a quality" / "Lossless cannot meet a fixed file size. Choose Very fast or High quality for this preset." and stops. Otherwise the choice is saved to `export_quality` (and mirrored into the editor combo), `selected_export_preset` is saved for custom presets, the dialog closes, then `mode_selected(bool)` or `preset_selected(preset)` fires.
- Dragging anywhere moves the dialog.

**Creation** (`_create_share_window`, cv:8390): no kept ranges -> `export_error('SHARE FAILED', 'The edit has no kept segments.', 'warning')`. `settings_info` = `{export_quality: combo data, clips_directory, resolution (default 'source'), fps: int(self._fps), bitrate (default 'medium'), watermark_enabled}`. Calls `configure`. The window receives copies of crop, effects, stretch, speed, preserve_pitch, per-source volumes/mutes, master volume, playback sources, audio tracks, kept ranges and the preset. Failure in creation: diagnostics `init_failed`, `export_error('SHARE FAILED', str(error), 'warning')` and a "Share could not start" warning dialog. Windows are tracked in the module set `_share_windows`, are independent top-level dialogs (parent `None`) centered on the editor.

**Window** (frameless Dialog, fixed 340x242): header 38 px titled "SHARE CLIP" or `SHARE  ·  {PRESET NAME UPPER}`; status badge "Exporting..." -> "Ready" (accent) / "Failed" (error colour); thumbnail zone 174 px tall (one frame at about `start + 0.5 s` via OpenCV, cropped and stretched with the edit, scaled to fit 310x174); filename bar 30 px starting "Preparing...", then showing status/plan text, then the filename (truncated to 40 chars + "…" past 42), or on error the message (42 chars + "…" past 44). Export starts exactly once, in `showEvent`, on thread `fthr-share-export`.
- Plan summary for a preset (`ExportPlan.summary`): `' · '.join(changes)` such as `60→30 FPS`, `{w}×{h}→{w2}×{h2}`, `{src:.1f} MB→≤{target:.1f} MB`, `H.264 video`, `AAC {n} kbps audio`; or "Original already fits; no re-encoding needed.".
- Success: drag zone gains the file (drag-and-drop copies the file URL, ghost pixmap "FTHRClips" 100x56), "drag to share" hint, and a "FTHRClips" corner pill animates in 300 ms later (380 ms OutCubic slide, hold 2500 ms, 280 ms InCubic out). Failure also emits `export_error('EXPORT FAILED', 'FFmpeg returned an error. The output file may be incomplete.', 'warning')`.
- Failure text: the last non-blank line of `stderr_tail` if present, else `result.detail`, else "Export failed".
- Preset size check in the validator: after the media validation, `size > target_size_mb * 1048576` raises `RuntimeError('Output is {x:.1f} MB, above the {target:g} MB target')`.
- Preset planning (`plan_export`, `core/export_profiles.py`): `target_bytes = int(size_mb * 1048576 * safety_margin)`; `budget_kbps = max(64, int(target_bytes * 8 / duration / 1000))`; `video_kbps = max(120, budget - audio_kbps - 12)`; FPS never raised; resolution ladder `(2160, 1440, 1080, 900, 720, 540, 480, 360)` walked only when the preset has no fixed width and the source is bigger than the target, stopping once `video_kbps >= max(180, int(w * h * min(fps, 60) * 0.035 / 1000))`; re-encode video if codec change, FPS drop, source larger than target, or scale change (not when the source is H.264/avc1, fits, and neither FPS nor size changes); re-encode audio only when video re-encodes and (non-AAC or bitrate > preset + 8). `probe_media` uses `ffprobe -v error -show_streams -show_format -of json` (timeout 45 s); failures show `f'Media inspection failed: {exc}'`.
- Closing (X, Escape, `reject`) sets `_cancelled`, sets `_export_cancel`, cancels the job, emits `close_requested`, joins the worker up to 2.5 s, removes any pending staged file, then ends the dialog directly. A cancelled share shows nothing further (callbacks return early).

## 13. Output validation (`_validate_export_output`, cv:134)

Run in FINALIZING with the cancel event: (1) `ffprobe -v error -show_streams -of json <staged>`; failure raises its stderr or `'FFprobe rejected the export'`; invalid JSON `'FFprobe returned invalid metadata'`; no video stream `'Export has no video stream'`. (2) `ffmpeg -v error -i <staged> -map 0:v:0 -frames:v 1 -f null -`; failure raises stderr or `'first-frame decode failed'`. Each child is wrapped by `_run_bounded_validation`: 15 s timeout (`'Export validation timed out'`), cancel check every 0.2 s (`'Export validation cancelled'`), terminate then kill with 1 s waits.

## 14. Watermark

`capture_card_watermark_filters(base_label, output_label='vout', card_label='fthr_watermark')` (`core/capture_card_watermark.py`) returns four filter-graph snippets, all appended to the main graph after the (concat-ed) video is labelled `vbase`:
1. Card: `color=c={bg}@0.92:s=320x76:r=60,format=rgba,colorchannelmixer=aa=0.92,drawbox=x=0:y=0:w=3:h=ih:color={accent}@1:t=fill,drawbox=x=76:y=12:w=1:h=52:color={divider}@1:t=fill,drawtext=fontfile='{Oswald-Bold.ttf}':text='CAPTURED WITH':fontcolor={stats_dim}:fontsize=10:x=94:y=10,drawtext=fontfile='...':text='FTHRClips':fontcolor={text}:fontsize=22:x=94:y=29[fthr_watermark]`.
2. Icon: `movie=filename='{assets/favicon.ico}':loop=1,format=rgba,scale=48:48:flags=neighbor,pad=48:48:(ow-iw)/2:(oh-ih)/2:color=black@0[fthr_watermark_icon]`.
3. `[fthr_watermark][fthr_watermark_icon]overlay=x=14:y=14:format=auto:eof_action=repeat[fthr_watermark_composed]`.
4. `[vbase][fthr_watermark_composed]overlay=x='{x_expr}':y='H-h-24':enable='between(t,0,2.9)':eof_action=pass[vout]`, with `x_expr` = `if(lt(t,0.38),W-(w+24)*(1-pow(1-t/0.38,3)),if(lt(t,2.55),W-w-24,W-w-24+(w+24)*pow(min(1,(t-2.55)/0.35),3)))`.
Constants: card 320x76, margin 24, exit time 2.90 s. Colours come from the theme `capture_card` group (`ThemeManager().get_all_capture_card_colors()`: `CAPTURE_CARD_BG`, `_ACCENT`, `_TEXT`, `_DIVIDER`, `_STATS_DIM`), converted `#RRGGBB` -> `0xRRGGBB` (3-digit expanded, anything malformed -> `0x000000`). Filter paths are escaped (`\` -> `/`, `:` -> `\:`, `'` -> `\'`). The card shows for the first 2.9 s of the exported timeline only (slides in over 0.38 s, holds, slides out from 2.55 s). With the watermark on, video is always re-encoded.

## 15. Webcam and Gary: not export features

- `clip_viewer.py`, `export_*.py` and `export_encoding.py` contain no webcam or Gary code; the filter graph in section 10 has no webcam input and no Gary layer.
- Webcam: `core/camera_recorder.py` (a `CameraRecorder` singleton keeping 90 s of JPEG frames at default 30 fps, quality 85) and the overlay geometry in `core/camera_overlay.py` (default rect `x .72, y .64, w .25, h .34`; default image overlay rect `x .76, y .04, w .20, h .24`; min size 0.08) are capture-side. The editor code has no webcam input, so the picture-in-picture can only be whatever the capture side wrote into the file (the capture pipeline was not traced for this digest). The only export-adjacent mention is the FFmpeg-missing text in `ft` ("Watermark, webcam overlay and clip export need it.").
- Gary Mode: `ui/gary_overlay.py` (`GaryOverlay`, click-through always-on-top screen image) and `core/gary_mode.py` (`DEFAULT_MIN_LEVEL 55`, `DEFAULT_MAX_LEVEL 85`, `MIN_THRESHOLD_GAP 5`, `clamp_thresholds`, `intensity_for_level`, `step_intensity(rise 0.18, fall 0.065)`) react to the live microphone level on a 33 ms timer (main.py:3618-3630). They draw to the screen being captured and are never composited by the export.

## 16. Live preview of crop / stretch / colour (not the export pipeline)

Applied through `ClipViewer._refresh_live_preview(immediate=False)` (cv:6381) -> `video_widget.set_edit_state(crop, effects, stretch, preview_enabled)` (cv:2333) and `_update_video_renderer` (cv:5828).
- Gates: `clip_editor_live_preview` (default True; off -> `set_edit_state(None, None, 1.0, preview_enabled=False)` once, original video shown, edits still exported). `editor_color_preview_enabled` (default True; off -> `_preview_color_effects()` returns `{}`, crop and stretch still preview).
- Debounce: slider/effect changes restart a single-shot 70 ms `_preview_update_timer` (`_flush_live_preview`); `immediate=True` bypasses it.
- Two surfaces: the GPU surface `QuickVideoPreview` (`ui/quick_video_preview.py`, QML `ui/qml/VideoPreview.qml` + `ui/qml/color.frag`) and a CPU `LiveVideoPreview` fallback (used when the scene graph/shader fails, `gpu_ready` False, or `QT_QUICK_BACKEND=software` / offscreen platform).
- GPU property mapping (`set_edit_state`, quick_video_preview.py:66): `exposure = v * 0.003`, `contrast = 1 + v * 0.006`, `saturation = 1 + v * 0.01`, `temperature = v * 0.003`, `sharpness = v * 0.015`, `vignette = v * 1.8 / 255`, `stretchRatio`, `sourceAspect = w/h`, `cropArea = (x/W, y/H, w/W, h/H)`, `cropActive`, `cropLabel = 'CROP  ·  {w} × {h}'`. The shader is only enabled when any effect is non-zero (`effectsActive`).
- Shader (`color.frag`): per pixel `rgb = (rgb - 0.5) * contrast + 0.5 + exposure`; saturation `mix(luma, rgb, saturation)` with luma weights `0.2126, 0.7152, 0.0722`; temperature multiplies by `(1+t, 1, 1-t)`; clamp and quantize to 1/255. Sharpness: 5x5 binomial blur (weights 1,4,6,4,1) of the graded image, `rgb*(1+s) - blurred*s`. Vignette (only inside the crop area, relative to the crop size): `radius = max(cropW, cropH) * 0.72`, `mix(rgb, background, clamp((dist - 0.35) / 0.65, 0, 1) * vignette)`.
- Display aspect: the QML item is `sourceAspect * stretchRatio` wide-to-tall, fit inside the surface with `VideoOutput.Stretch` (stretch is a real horizontal scaling). `_video_display_rect` in the CPU path uses `(src_w * stretch) / src_h`.
- Crop is NOT applied to the picture in the preview. The full frame stays visible; outside the crop is veiled and the crop edge is outlined. GPU: four rectangles of `backgroundColor` at opacity 0.32, 3 px accent border, label pill (22 px high, 12 px bold accent text, offset 6 px from the crop corner, minimum y -28). CPU `CropPreviewOverlay` (cv:1885): veil `Colors.BG` alpha 82, 3 px `ACCENT` pen on `(cx-1, cy-1, cw+2, ch+2)`, label `CROP  ·  {w} × {h}` in `Fonts.DISPLAY` 9 pt bold, 22 px high, fill `ACCENT_SOFT` alpha 222, `label_y = max(top + 6, cy - 28)`.
- CPU colour path (`_apply_color_effects`, cv:2600): one OpenCV affine transform (`cv2.transform`) combining brightness `exposure*0.003`, contrast `1+0.006c`, saturation matrix with luma `(0.2126, 0.7152, 0.0722)`, and temperature diag `(1+b, 1, 1-b)`, `b = temperature*0.003`; sharpness via `GaussianBlur(5x5)` + `addWeighted(out, 1+a, blurred, -a, 0)`, `a = sharpness*0.015`. Vignette is a `QRadialGradient` over the crop rect (`_effect_display_rect`): radius `0.72 * max(w, h)`, stops 0.35 -> alpha 0, 1.0 -> `Colors.BG` alpha `int(vignette * 1.8)`.
- The preview is an approximation: export uses FFmpeg `curves` / `hue` / `colorbalance` / `unsharp` / `vignette`, the shader uses different maths for the same slider values. Speed is not previewed visually (playback rate changes the player only).
- `_requires_software_video` (cv:5465): with a `QuickVideoPreview` surface, software mode iff `not gpu_ready`.

## 17. Error titles and messages (user visible)

Emitted on `export_error(title, detail, level)` and routed (via `clip_editor_window` to `main.py:9679`) to `push_error`, which silently drops any title not in `core/error_codes.py`'s table:
- `EXPORT FAILED` (Error 018, group "Export and sharing"): `'The edit has no kept segments.'` (level `warning`); `str(error)` from the submit/command building step (`warning`); `'FFmpeg returned an error. The output file may be incomplete.'` (from the share window failure, `warning`).
- `SHARE FAILED` (Error 019): `'The edit has no kept segments.'`; `str(error)` from window creation. Plus a modal warning "Share could not start" with `str(error)`.
- `FFMPEG NOT FOUND` (level `error`): `'ffmpeg is required for export. It normally ships with FTHR Clips; if you are running from source, install it: sudo pacman -S ffmpeg (Arch) or sudo apt install ffmpeg (Debian).'` Emitted by the share window only. This title is not in `error_codes.py`, so `push_error` returns without showing it.
- `FFmpegUnavailable` message (editor path, shown as the `EXPORT FAILED` detail via `get_ffmpeg_exe()`): "No ffmpeg binary found. FTHR Clips ships one next to the capture engine; if this is a source checkout, run the build script or install ffmpeg and make sure it is on your PATH. Watermark, webcam overlay and clip export need it."
- Queue-side (only visible in the jobs window tooltip): `'FFmpeg exited with code {n}'`, `'Export timed out'`, `'Export cancelled'`, `'Export produced no usable output'`, `'Export finalization produced no usable output'`, validation messages from section 13.
- The Delete guard text for an in-flight export is in the editor notes (section 9 there).

## 18. Reference quirks worth not copying blindly

- The dead legacy export code (section 1) still has the user-visible "exported" / "Export failed" / "Export cancelled" strings; the live flow shows none of them. The editor gives no completion feedback besides the jobs-window count.
- A split without a delete produces two contiguous kept ranges and takes the full re-encode concat path, even though nothing is removed.
- `_build_export_cmd` single-range branch: if `use_audio_filter` is true but `ffmpeg_mix_filter` returns no output (no available sources), `audio_output` is `None` and gets appended to the command list.
- Concat path audio speed is applied after concat, video speed per range; the single-range path uses `-t` of the speed-adjusted length and `-ss` input seeking, so cut points can land on the nearest keyframe when the video is stream-copied.
- `-shortest` is set on the concat path only.
- `task.status` uses `Timed_Out` (title-cased enum) in the UI.
- The editor passes its master volume (default 80) into the export mix as `volume=0.8`, so exporting with the default master gain lowers the output level compared with 100.
- Share status updates are routed through the `_plan_sig` channel and shown in the filename label; the progress percent is not shown for share at all.
- `ShareWindow` reads `export_quality` raw (`== 'lossless'`) for the extension but normalizes it for encoding; an invalid stored value gets `.mp4` and `quality` encoding consistently only by luck.
- `ClipViewer.export_error` for `FFMPEG NOT FOUND` is dropped by `push_error` because the title has no failure code.
- `resolution`, `bitrate`, `fps` are copied into the share window's `settings_info` and never used.
- The preview's colour maths does not match the FFmpeg filters, so look-matching is approximate by construction.
