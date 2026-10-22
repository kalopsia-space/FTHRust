# FTHRclips Linux capture engine — inventory for a Rust rewrite

Reference tree: `/tmp/claude-1000/-home-kalopsia-fthrrst-clips/1cc031d0-ffdf-42db-86b0-82c001c067a9/scratchpad/repo`
(paths below are relative to that root; `L/` = `FTHRcapture_linux/`, `C/` = `FTHRcapture_common/`).
Scope: Linux + Wayland. X11 (`L/src/backend_x11.*`, `x11_capture_target.*`, x11grab via libavdevice,
`FTHR_X11_CAPTURE` CMake option default ON) exists but is only noted here. Windows ignored.

Binary: `FTHRclips` (one process, ~5.7k LOC C++20) + `libFTHRPlaybackMixer.so` (C ABI, loaded by the Python UI).

---

## 1. Process model

### 1.1 Launch
- UI (`FTHR_UI/main.py:5729`) spawns `subprocess.Popen([engine_path, <25 positional args>], stdout=PIPE, stderr=STDOUT, text, bufsize=1, env=linux_runtime.child_process_environment())`.
  Engine stdout/stderr is captured line-wise into `~/.fthr/logs/engine.log` (`main.py:5706-5712`). No cwd change, no pipes for commands: all commands go through shared memory.
- Env: `child_process_environment()` (`FTHR_UI/core/linux_runtime.py:242`) restores the host `LD_LIBRARY_PATH` from `LD_LIBRARY_PATH_ORIG` so the engine finds host Mesa/VA-API drivers; optional `FTHR_DIAGNOSTIC_SESSION_ID` is added. Engine itself reads: `WAYLAND_DISPLAY` (presence only, `L/src/capture_backend.cpp:82`), `FTHR_CAPTURE_BACKEND` (`wlr|ext|portal|x11`, case-insensitive, `capture_backend.cpp:86`, `capture_backend_policy.cpp:30`), `FTHR_VAAPI_DEVICE` (default `/dev/dri/renderD128`, `encoder.cpp:132`), `HOME` (portal restore token, `backend_portal.cpp:339`).
- Engine path discovery: `main.py:3361-3396` (PyInstaller `_MEIPASS/FTHRclips`, `FTHRcapture_linux/build/FTHRclips`, env `FTHR_ENGINE`).

### 1.2 argv contract (`L/src/engine_args.h:40-65`, `engine_args.cpp:71-142`, `docs/engine-startup.md`)
Positional, index = argv index; missing or empty = default (no warning); bad value = default + `[Config]` warning on stderr.

| argv | field | default | rule |
|---|---|---|---|
| 1 | fps | 60 | 0 invalid -> 60; clamp 1..360 (`kMaxFps=360`) |
| 2 | buffer_seconds | 30 | 0 invalid -> 30; clamp 1..900 |
| 3,4 | target_width/height | 0,0 = native | half-specified or >7680x4320 -> native |
| 5 | bitrate_kbps | 16000 | clamp 500..200000 (`C/capture_config_limits.h:13-14`) |
| 6 | max_buffer_mb | 0 = no byte budget | nonzero clamped 64..16384; byte budget of encoded ring |
| 7,8 | Windows capture mode / HWND | ignored | |
| 9 | scaling_mode | 0 stretch | `==1` -> fit (letterbox), anything else 0 |
| 10 | target_output | "" = first output | wl_output name e.g. `DP-1` |
| 11 | codec_pref | 0 Auto | 1 H264, 2 HEVC, 3 AV1; unknown -> Auto |
| 12 | preset | 4 | clamp 1..7 |
| 13 | retired multiband slot | ignored | |
| 14 | audio_enabled | 1 | nonzero = true |
| 15,16 | mic endpoint / gain | ignored (UI records mic) | |
| 17 | encoder_pref | 0 Auto | 1 Nvidia, 2 Amd, 3 Intel, 4 Software; unknown -> Auto (never Software by accident) |
| 18-22 | crop | ignored | |
| 23 | separate_audio_enabled | 0 | only affects MP4 `comment` metadata |
| 24 | playback_endpoint_id | "" = default sink | PulseAudio sink name; its `.monitor` is captured |
| 25 | microphone_mono | 0 | parsed, ignored (logged) |

Numbers parsed with `capture_config_limits::ParseU32` (unsigned decimal only, overflow-safe).
`--list-playback-devices` / `--list-microphones` must be the only argument; any other `--flag` anywhere -> usage on stderr, exit 2 (`engine_args.cpp:54-69`, `main.cpp:70-85`). `-1` alone is a value, not a flag.

### 1.3 Lifecycle (`L/src/main.cpp`)
1. Classify CLI; listing modes print one JSON line and exit without touching shm (`main.cpp:51-67`).
2. `ParseEngineArgs` -> `CaptureConfig` (`main.cpp:93-109`; `multiband_enabled` hard-coded false at :103).
3. `SharedMemory::Initialize("FTHR_SharedMemory_v4")` -> `shm_open("/FTHR_SharedMemory_v4", O_CREAT|O_RDWR, 0600)`, `ftruncate(sizeof)`, `mmap`, `memset 0` (`shared_memory.cpp:12-43`). Failure -> exit 1.
4. `CaptureEngine::Initialize` (starts audio + capture thread), then publish `is_initialized=true`, cfg_* fields, zeroed v3/v4 fields (`main.cpp:141-157`).
5. `signal(SIGINT/SIGTERM)` -> `g_quit`; `prctl(PR_SET_PDEATHSIG, SIGTERM)` so an orphaned engine dies with the UI (`main.cpp:162-172`).
6. Command poll loop every 20 ms (`usleep(20000)`, `main.cpp:183-367`): read `ui_command`, write NONE back, dispatch; then refresh status fields every tick.
7. Exit paths: `SHUTDOWN` command (`main.cpp:287-294`), signal, or `!engine.IsCapturing()` for 2 s (`kFailurePublishGrace`, `main.cpp:177,344-352`: lingers so the UI's 500 ms poll can read BACKEND_FAILED + reason). Then `is_initialized=false`, `engine.Shutdown()`, shm unlinked in `~SharedMemory` (`shared_memory.cpp:45-59`), exit 0.
- UI shutdown escalation (`main.py:6066-6090`): write SHUTDOWN, `wait(2.0)`, `terminate()`, `wait(1.0)`, `kill()`, `wait(0.5)`.

### 1.4 Threads
- Main/command thread: shm poll loop; **SaveClip runs synchronously on it** (`main.cpp:213`), blocking status updates and further commands for the whole mux.
- Capture thread (`CaptureEngine::CaptureLoop`, `capture_engine.cpp:98`): backend + encoder + ring push.
- Audio capture thread + default-output watcher thread (`audio_capture.cpp:252-254`).
- Portal backend: PipeWire thread loop (`backend_portal.cpp:397`).

### 1.5 Generations
- `capture_generation` (shm, `capture_engine.cpp:276`): incremented every time a backend generation starts successfully (first start and every recovery).
- `encoder_generation_` (`capture_engine.cpp:264`): incremented only when the encoder is (re)opened; stamped on every `EncodedPacket.generation` (`encoder.cpp:392`) and in `EncoderSnapshot` so a save never mixes packets with foreign extradata.
- Backend-only restart with same encode geometry keeps the encoder and the ring ("gap" rendered via monotonic PTS, `capture_engine.cpp:237-245`); geometry change or encoder failure closes encoder and `ring_->Clear()` (:247-273).

---

## 2. Shared-memory contract

Name `FTHR_SharedMemory_v4` (`main.cpp:127`, `FTHR_UI/core/capture_bridge.py:127`), POSIX shm at `/dev/shm/FTHR_SharedMemory_v4`, mode 0600, engine creates and unlinks; UI opens read-write with mmap and **refuses to map if `fstat.st_size != sizeof`** (`capture_bridge.py:164-190`). Bump name on any layout change (CONTRIBUTING.md:103-150). Commands 4-9 must never be reused.

### 2.1 Layout (`L/src/shared_memory.h:9-46`; must match `capture_bridge.py:87-120` posix branch). Natural x86-64 alignment, **sizeof = 4272, alignof = 8** (computed with g++; Windows variant is 2736 because `ui_string` is wchar there).

| off | size | type | field | writer | notes |
|---|---|---|---|---|---|
| 0 | 4 | u32 | ui_command | UI writes, engine resets to 0 | CommandType |
| 4 | 4 | u32 | ui_param1 | UI | SAVE_CLIP duration s (0 -> buffer_seconds) |
| 8 | 4 | u32 | ui_param2 | UI | SET_RESOLUTION height (dead) |
| 12 | 4 | u32 | ui_param3 | UI | unused |
| 16 | 1024 | char[1024] | ui_string | UI writes UTF-8 path; engine clears after SAVE_CLIP / refused START | may lack NUL: engine uses memchr (`main.cpp:20-26`) |
| 1040 | 4 | u32 | engine_response | engine publishes; **only UI clears to NONE** | ResponseType |
| 1044 | 4 | u32 | engine_param1 | engine | GET_STATUS: nvenc_active |
| 1048 | 4 | u32 | engine_param2 | engine | unused |
| 1052 | 4 | f32 | engine_param3 | engine | unused |
| 1056 | 2048 | char[2048] | engine_string | engine via `set_engine_string` only | save error / manual-recording msg / capture failure or recovery reason |
| 3104 | 1 | bool | is_recording | engine | always false on Linux (`main.cpp:364`) |
| 3105 | 1 | bool | is_initialized | engine | true after init, false at exit |
| 3112 | 8 | u64 | frames_captured | engine | encoded frame count (every 20 ms) |
| 3120 | 8 | u64 | bytes_written | engine | save progress (`save_clip.cpp:20,317`) |
| 3128 | 4 | u32 | cfg_bitrate_kbps | engine at init; SET_QUALITY echoes ui_param1 | |
| 3132 | 4 | u32 | cfg_target_width | engine at init; SET_RESOLUTION echo | |
| 3136 | 4 | u32 | cfg_target_height | engine | |
| 3140 | 1 | bool | nvenc_active | engine | "a compressed replay encoder is open" (software included) |
| 3144 | 4 | u32 | cfg_codec_pref | UI before RECONFIGURE_ENCODER; engine at init | 0 auto,1 h264,2 hevc,3 av1 |
| 3148 | 4 | u32 | cfg_preset | UI before RECONFIGURE_ENCODER; engine at init | 1..7 |
| 3152 | 64 | char[64] | active_codec | engine (strncpy each tick, `main.cpp:357-362`) | e.g. `h264_vaapi` |
| 3216 | 1 | bool | multiband_enabled | engine | always false |
| 3217 | 1024 | char[1024] | active_audio_mappings | engine | always zero on Linux |
| 4244 | 4 | u32 | capture_health_flags | engine | bits below |
| 4248 | 4 | u32 | capture_generation | engine | |
| 4252 | 4 | u32 | content_sample_sequence | engine | +1 per content sample (1/s) |
| 4256 | 4 | u32 | content_suspicious_streak | engine | consecutive black/uniform samples |
| 4260 | 4 | f32 | content_luma_mean | engine | |
| 4264 | 4 | f32 | content_luma_variance | engine | |

Padding: 2 bytes at 3106, 3 at 3141, 3 at 4241. Total 4272 (last field ends at 4268, padded to 8).

### 2.2 Enums (`shared_memory.h:48-86`)
- `CaptureHealthFlags`: ACTIVE=1, RECOVERING=2, BACKEND_FAILED=4, CONTENT_SUSPECT=8, PAUSED=16.
- `CommandType`: NONE 0, START_RECORDING 1, STOP_RECORDING 2, SAVE_CLIP 3, SET_RESOLUTION 4, SET_QUALITY 5, SET_FRAMERATE 6, SET_HOTKEY 7, SET_TARGET_WINDOW 8, GET_STATUS 9, RECONFIGURE_ENCODER 10, SHUTDOWN 11.
- `ResponseType`: NONE 0, RECORDING_STARTED 1, RECORDING_STOPPED 2, CLIP_SAVED 3, STATUS_UPDATE 4, ERROR_OCCURRED 5, SAVE_STARTED 6, MANUAL_RECORDING_ERROR 7.

### 2.3 Command semantics (`main.cpp:186-295`)
- Engine reads `ui_command`, immediately writes NONE, then acts. UI writes payload fields first and `ui_command` last (`capture_bridge.py:321-323`).
- **SAVE_CLIP**: path = ui_string (then cleared), duration = ui_param1 or buffer_seconds; clear engine_string; publish `SAVE_STARTED`; run `engine.SaveClip` synchronously; on failure write error to engine_string **then** `ERROR_OCCURRED`, else `CLIP_SAVED` (`main.cpp:190-226`).
- **GET_STATUS**: engine_param1 = nvenc_active, then `STATUS_UPDATE`.
- **RECONFIGURE_ENCODER**: reads cfg_codec_pref/cfg_preset, `engine.Reconfigure` (full stop/restart of capture thread, ring, audio), clears active_codec, `STATUS_UPDATE`.
- **SET_RESOLUTION / SET_QUALITY**: copy ui_params into cfg_* (no effect), `STATUS_UPDATE`.
- **SET_FRAMERATE/SET_HOTKEY/SET_TARGET_WINDOW** (default branch): `STATUS_UPDATE` so callers never wait.
- **STOP_RECORDING**: `engine.SetPaused(true)` -> `RECORDING_STOPPED` (focus pause; capture thread sleeps 50 ms loops, flags ACTIVE|PAUSED).
- **START_RECORDING**: empty ui_string -> `SetPaused(false)` + `RECORDING_STARTED`; non-empty -> clear it, engine_string = "Manual recording is not available on Linux yet..." then `MANUAL_RECORDING_ERROR` (`main.cpp:33-49`).
- **SHUTDOWN**: `g_quit`.

### 2.4 Status publication each tick (`main.cpp:298-364`)
frames_captured, nvenc_active, health flags, generation, content metrics, active_codec, is_recording=false. Failure-reason rule: engine_string is shared with save results, so while a save response (SAVE_STARTED/CLIP_SAVED/ERROR_OCCURRED/MANUAL_RECORDING_ERROR) is still unconsumed by the UI, the BACKEND_FAILED bit is **held back** and the reason not written (`main.cpp:305-324`); while RECOVERING the reason string is written/cleared as it changes (:325-337). No engine_response is ever published for a capture failure.

### 2.5 "Engine response contract" (`shared_memory.h:57-72`, `tools/verify_engine_response_contract.py`)
Rules enforced statically by the tool over `L/src`: (a) payload (`engine_string` via `set_engine_string`, `engine_param*`) must be written **before** `engine_response` in the same block; (b) no payload write after a non-STATUS_UPDATE publish; (c) `engine_string` only touched through `set_engine_string` (bounded 2047 + NUL, memcpy not strncpy); (d) no `strcpy/sprintf/...` into engine_string; (e) engine never writes `engine_response = NONE` except at init (memset). `tools/verify_shared_memory_contract.py` parses both C++ headers and the ctypes structs, models alignment, compares field order/types/extents/offsets/size and enum values for win32 and linux.

### 2.6 Polling, no signalling
Pure polling, no futex/semaphore/eventfd: engine 20 ms, UI `status_timer` 500 ms (`main.py:3505`) plus a save poller. No atomics, fences or volatile on either side: ordering relies on program order of plain stores (risk, see 9).
UI reads `capture_failure_detail` from engine_string only when BACKEND_FAILED is set (`capture_bridge.py:500-512`); on FAILED state it shows "CAPTURE FAILED" with a RESTART ENGINE action (`main.py:8820-8847`); RECOVERING/stall surfaces as "CAPTURE STALLED".

---

## 3. Capture backends

Interface `ICaptureBackend` (`L/src/capture_backend.h:32-64`): `Initialize(cfg)`, `CaptureFrame(RawFrame&)` (synchronous, bounded), `Shutdown`, `Type`, `NativeWidth/Height`, `InitFailureKind`, `FailureReason`, `SelectedOutput`, `SetStrictOutput`. `RawFrame` = `{data,stride,width,height,av_pix_fmt,timestamp_ns(CLOCK_MONOTONIC)}` valid until next call. `BackendFailureKind`: None, Error (counts toward retry budget), CompositorUnreachable, OutputAbsent (transient, waited out), UserDeclined (never retry).

### 3.1 Selection policy (`capture_backend_policy.cpp`, `capture_backend.cpp`)
- `BackendCandidates(has_wayland)`: Wayland -> `[WlrScreencopy, ExtImageCopy, PortalScreenCast]`; else `[X11Grab]` (`policy.cpp:18-24`).
- `CreateBestBackend` (`capture_backend.cpp:74-140`): honours `FTHR_CAPTURE_BACKEND` (forced, no fallback); otherwise tries candidates in order; portal `UserDeclined` aborts selection; if all fail on Wayland -> "No Wayland capture path is available..." (explicitly refuses XWayland/x11grab fallback).
- `RecoveryCandidate(last_good) == last_good` (`policy.cpp:26`): recovery only ever retries the backend that previously initialised; never re-runs first-time selection (avoids re-opening the portal dialog).
- Strict output: during recovery, if the user named an output and the backend had actually captured it, `OutputAbsent` is returned instead of falling back to the first output (`capture_engine.cpp:203-204`, `backend_wlr.cpp:258-264`).

### 3.2 Recovery policy (`L/src/recovery_policy.h`, `capture_engine.cpp:98-148`)
- `CaptureRecoveryPolicy(healthy_reset_frames = fps*10, max_attempts = 3)`: backoffs 250/750/1500 ms; a generation that produced >= fps*10 frames resets the attempt counter; transient failures (`CompositorUnreachable`, `OutputAbsent`) retry every 2000 ms without consuming budget. Exhausted or `FailedForGood` -> `BACKEND_FAILED`, `running_=false`, command loop stays alive to publish.
- `WaitForRecoveryBackoff` sleeps in 50 ms slices checking `running_`.
- `EncodeFailurePolicy(fps)`: threshold `max(5, fps/2)` consecutive `EncodeFrame` failures ends the generation as `FailedEncoder` (encoder reopened next generation, ring cleared).
- `NextEncodeDeadline`: throttle to 1/fps of capture time; re-anchors if a frame arrives > 2 frame periods late (no burst catch-up) (`recovery_policy.h:83-87`, `capture_engine.cpp:309-313`).
- Generation flow `RunCaptureGeneration` (`capture_engine.cpp:155-343`): create backend -> compute enc size (fit/stretch, even-aligned) -> reuse or open encoder -> loop: paused? sleep 50 ms; `CaptureFrame`; throttle; `SampleContent`; `EncodeFrame` -> ring push; on exit shut backend, close encoder if it failed or stopping.

### 3.3 Content watchdog (`capture_engine.cpp:345-391`)
Once per `fps` produced frames: 16x9 sparse pixels, luma = (19B+183G+54R)>>8, mean/variance; suspicious if (mean<=8 && var<=6) || var<=2; streak >= 12 (12 s) sets CONTENT_SUSPECT. Privacy-safe (no image retained).

### 3.4 wlr-screencopy backend (`L/src/backend_wlr.{h,cpp}`)
- Binds `wl_compositor` (<=v4), `wl_shm` v1, `wl_output` (<=v4), `zwlr_screencopy_manager_v1` (<=v3), `zxdg_output_manager_v1` (<=v3, bound but unused).
- Init (5 s deadline): connect, two bounded roundtrips (registry, outputs), select output by `wl_output.name` (fallback first; strict -> OutputAbsent), wait `output.done`, probe one throw-away frame to learn size/format (`capture_output(overlay_cursor=0)` -> **cursor not captured**), allocate memfd-backed `wl_shm_pool` buffer, copy, wait ready.
- `CaptureFrame` (2 s deadline, `backend_wlr.cpp:371-435`): new frame request each call, wait `buffer_done` (v3 event), reallocate buffer if size/stride/format changed (mode switch, HDR toggle), `copy`, wait `ready|failed`, timestamp = CLOCK_MONOTONIC now (compositor `ready` tv ignored), normalise via `ShmFrameNormalizer`. `y_invert` flag honoured; `damage`, `linux_dmabuf` events ignored (shm only). No frame pacing in the backend: it requests as fast as the compositor answers; engine throttle drops extras.
- Failure of any wait -> `CaptureFrame` false -> generation ends -> recovery.

### 3.5 ext-image-copy-capture backend (`L/src/backend_ext.{h,cpp}`)
- Binds `wl_shm`, `wl_output`, `ext_image_copy_capture_manager_v1` v1, `ext_output_image_capture_source_manager_v1` v1. Creates output source, `create_session(source, options=0)` (**no PAINT_CURSORS**), listens `buffer_size`, `shm_format` (batch closed by `done`; `constraints_serial_` bumped), `stopped`. dmabuf device/format events ignored.
- Format choice `PickWlShmFormat` preference order (`shm_frame.cpp:32-50`): XRGB8888, ARGB8888, XBGR, ABGR, RGBX, RGBA, BGRX, BGRA, then 10-bit variants.
- One pending frame at a time (`pending_frame_` outlives a timed-out call; protocol forbids a second request). `CaptureOnceResult` attaches buffer, full damage, `capture`, waits `ready|failed|stopped`.
- `buffer_constraints` failure: wait (2 s) for a new constraints batch, realloc, retry once (`backend_ext.cpp:347-368`).
- Static-screen handling: owned copy `last_frame_` of the last completed frame; on dispatch `Timeout` with a last frame, repeat it (`ShouldRepeatLastFrame`, `ext_repeat_policy.h`), timestamped now; deadline anchored on `last_delivery_ + 1/fps` (min 1 ms) so a static desktop still produces fps frames.
- `transform`, `damage`, `presentation_time` events ignored.

### 3.6 ScreenCast portal + PipeWire backend (`L/src/backend_portal.{h,cpp}`, `portal_screencast.{h,cpp}`, `pipewire_frame.{h,cpp}`; compiled only with `FTHR_PORTAL_BACKEND`)
- `libdbus-1.so.3` and `libpipewire-0.3.so.0` are **dlopen'd** (function tables `DBusApi`, `PipeWireApi`); headers only at build time. Missing lib -> clear failure reason, other backends continue.
- D-Bus flow (`portal_screencast.cpp:577-685`): private session bus, match rules for `Request.Response` and `Session.Closed`; probe `Properties.Get(ScreenCast, version)` (missing -> Unavailable); `CreateSession` (handle_token, session_handle_token), `SelectSources` (types=1 MONITOR, multiple=false, cursor_mode=2 EMBEDDED, persist_mode=2, restore_token if any), `Start(parent_window="")` with 120 s dialog timeout (method reply 5 s, request 10 s), then `OpenPipeWireRemote` -> fd. Response code 1 -> Cancelled, other nonzero -> Failed. Request path derived from unique name (`:1.42` -> `/org/freedesktop/portal/desktop/request/1_42/<token>`); honours a differing handle returned by old portals. Bus polled in 50 ms slices with `keep_running` cancellation; `Request.Close` sent on abort.
- Restore token persisted at `$HOME/.fthr/portal_screencast_token` (0600, tmp+fsync+rename; empty token deletes file; whitespace/control chars = damaged). `RunPortalStartupWithRetry`: a restored session that fails to negotiate retries once without the token (`portal_screencast.cpp:295-320`).
- PipeWire (`backend_portal.cpp:396-461`): `pw_thread_loop` + `pw_context_connect_fd`, stream props Video/Capture/Screen, `EnumFormat` offer = raw video in BGRx/BGRA/RGBx/RGBA, size 1x1..16384, framerate default fps range 0..1000, **no modifier property => DMA-BUF-only negotiation fails on purpose** ("compositor offers this screen only as DMA-BUF, which this build does not import"). Buffers: 2..16 (default 4), MemFd|MemPtr, header meta for pts. `PW_STREAM_FLAG_AUTOCONNECT|MAP_BUFFERS`. Format must arrive within 10 s, first frame within 5 s.
- `OnStreamProcess` (loop thread): keep only newest queued buffer, reject corrupted chunks/DmaBuf, copy rows into `pending_` with optional R/B swap (`CopyFrameRows`), pts = header pts if within 1 s of now else now. `CaptureFrame` (capture thread) swaps front/pending; on a static screen repeats the last frame at 1/fps anchored on last delivery; timestamps forced strictly increasing (`std::max(ts, last+1)`). Resolution change mid-stream fails the generation (`backend_portal.cpp:237-242`); format-only change just swaps `layout_`.
- `Session.Closed` polled every 30 `CaptureFrame` calls. The dialog chooses the screen; `target_output` cannot be preselected (`backend_portal.cpp:343-347`).
- Cancelled/TimedOut dialog => `UserDeclined`, retry_pointless (engine exits after publishing reason).

### 3.7 Wayland dispatch (`L/src/wayland_dispatch.{h,cpp}`)
Injectable `WaylandOperations` seam (dispatch_pending, prepare_read, cancel_read, read_events, flush, get_fd, get_error, poll, sync, callback listener). `WaitForWaylandEvent`: dispatch pending -> deadline check -> prepare_read -> flush (EAGAIN => POLLOUT) -> `poll` in <=50 ms slices with cancellation -> read_events -> dispatch. Results: EventReceived, Timeout, Cancelled, Disconnected (EPIPE/ECONNRESET/ENOTCONN, POLLHUP/ERR, or `wl_display_get_error`), Error. `DispatchWaylandUntil(predicate)` and `BoundedWaylandRoundtrip` (wl_display_sync callback). Every prepare_read is paired with read/cancel (RAII `PreparedRead`).

### 3.8 Frame normalisation (`L/src/shm_frame.{h,cpp}`, `x11_frame_converter.cpp`)
`WlShmFormatToAvPixFmt`: wl_shm ARGB8888->BGRA, XRGB8888->BGR0, ABGR->RGBA, XBGR->RGB0, RGBA->ABGR, RGBX->0BGR, BGRA->ARGB, BGRX->0RGB, [AX]RGB2101010->X2RGB10LE, [AX]BGR2101010->X2BGR10LE. BGR0/BGRA top-down pass through zero-copy; anything else (other orders, 10-bit, y-inverted via negative linesize) converted by swscale `SWS_POINT` into an owned BGR0 buffer (`X11FrameConverter`). Encoder ultimately treats input as BGRA regardless of `av_pix_fmt`.

### 3.9 Output/monitor selection, cursor, scaling
- Output: by `wl_output.name` (needs wl_output v4); empty -> first; portal: user picks.
- Cursor: wlr `overlay_cursor=0`, ext options=0 -> **no cursor**; portal `cursor_mode=2` -> cursor embedded. Inconsistent across backends.
- Scaling (`capture_engine.cpp:206-223`): target 0 -> native; `scaling_mode==1` fits inside target keeping aspect (letterbox is really "shrink to fit", no bars are painted; output size just becomes smaller); both dims masked to even. Source-size changes mid-generation are rescaled by the encoder (`encoder.cpp:307-312`), output size fixed per encoder generation.
- dmabuf: not supported on any path (shm/memfd only).

---

## 4. Encoder (`L/src/encoder.{h,cpp}`, `encoder_policy.{h,cpp}`)

- Candidates `EncoderCandidates(codec_pref, encoder_pref)` (`encoder_policy.cpp:5-57`): Auto = Nvidia [h264_nvenc, hevc_nvenc, av1_nvenc] + Amd [h264_vaapi, hevc_vaapi, av1_vaapi, h264_amf, hevc_amf, av1_amf] + Intel [h264_qsv, hevc_qsv, av1_qsv], filtered by codec_pref; **never software unless EncoderPref::Software** = [libopenh264, libkvazaar, libsvtav1, libaom-av1, librav1e]. First that opens wins; name published as `active_codec`.
- `TryOpen` (`encoder.cpp:98-179`): `avcodec_find_encoder_by_name`, width/height, `time_base 1/fps`, `framerate fps/1`, `bit_rate = kbps*1000`, `gop_size = 2*fps`, `max_b_frames 0`, colour: range MPEG, BT.709 primaries/trc/space, chroma LEFT; `AV_CODEC_FLAG_GLOBAL_HEADER`. pix_fmt: VAAPI (hw frames ctx NV12, pool 8) / NV12 for other hw / YUV420P software. VA-API guarded by a `dlopen(libva.so.2)`+`dlsym(vaMapBuffer2)` probe (libva >= 2.19; `encoder.cpp:23-29`).
- `ApplyPreset` (`encoder.cpp:33-94`): nvenc p1..p7 + tune ll, rc vbr, forced-idr; amf quality speed/balanced/quality + rc vbr_latency; **vaapi: nothing** (driver-specific semantics, documented "Known limitation"); qsv preset veryfast..slowest + look_ahead 0; openh264 rc_mode bitrate, allow_skip_frames 0, profile high; kvazaar/libx26x preset names (+ x26x tune zerolatency); svtav1 preset 12/9/7/6/4/1/0; libaom none.
- Scaler: `sws_getContext(BGRA src -> enc size, dst fmt, SWS_BILINEAR)` + `sws_setColorspaceDetails(BT709, src full-range, dst studio)`; rebuilt on source-size change (`encoder.cpp:243-273`).
- `EncodeFrame(bgra, stride, w, h, wall_ns, push)` (`encoder.cpp:300-408`): validates stride >= w*4; `sws_scale`; **PTS from wall clock**: `pts = round((wall - epoch) * fps / 1e9)`, forced `>= last+1`; forced IDR every `2*fps` ticks via `pict_type I` + `AV_FRAME_FLAG_KEY`; VA-API path `av_hwframe_get_buffer` + `av_hwframe_transfer_data` + copy props; `avcodec_send_frame` / drain `avcodec_receive_packet`; each packet -> `EncodedPacket{data copy, pts, dts, is_keyframe, wall_time_ns (matched to the input frame with equal pts via pending_timings_), generation}`. Returns false on any send/receive/upload error (feeds `EncodeFailurePolicy`).
- `GetExtradata()` copies codec extradata (SPS/PPS / hvcC / av1C) for the MP4 writer.
- FFmpeg APIs used: libavcodec (find_encoder_by_name, alloc_context3, open2, send_frame, receive_packet, packet/frame alloc), libavutil (opt, hwcontext VAAPI, hwframe, imgutils, error), libswscale.
- fps up to 360 accepted by args (`kMaxFps`); nothing in the encoder bounds it further.

---

## 5. Ring buffer and replay/save

### 5.1 EncodedRingBuffer (`L/src/ring_buffer.{h,cpp}`)
- Constructed by `MakeRing` (`capture_engine.cpp:26-51`): `max_duration_ms = (buffer_seconds + 5) * 1000`, `fps`, `max_bytes = argv[6] MiB` (0 = time-bounded only; logs expected size and warns if the budget covers fewer seconds). UI computes argv[6] with `linux_ring_budget_mb` (`FTHR_UI/core/capture_settings.py:103`).
- `Push`: mutex; time trim from the front while span > max_duration; byte budget trims whole GOPs from the front while a newer keyframe remains (current GOP never dropped); logs once; counts `budget_trims_`; updates `latest_wall_time_ns_` and notifies `publication_cv_`.
- `TakeSnapshot(duration_ms, target_end_ns, generation)`: filters by generation (0 = all), builds `replay_interval::Sample{wall_time, keyframe}` and calls `replay_interval::Select` (`C/replay_interval.h:62-115`): end = last packet with ts <= target; `full_history` iff first packet ts <= requested_start and a keyframe exists at/before requested_start; decode_start = that keyframe (else first keyframe); `presentation_start_pts = newest_pts + 1 - duration_ms*fps/1000` when full history, else decode-start pts (`PresentationStartPts`). Snapshot carries `presentation_start/end_ns`, `media_end_ns`.
- `WaitUntilPublished(target_end_ns, timeout)`: waits until a packet with wall time >= target has been pushed (save uses timeout `max(100, 3000/fps)` ms, `capture_engine.cpp:466-468`).
- `Clear` on encoder reopen / Reconfigure.

### 5.2 SaveClip (`capture_engine.cpp:435-533`)
Refused (false + message) when: not running, paused, health has BACKEND_FAILED|RECOVERING|PAUSED ("Capture is not receiving new frames; restart capture before saving"); no valid `EncoderSnapshot` ("Capture restarted; the replay buffer is still rebuilding"); snapshot empty (distinguishes empty ring vs only-older-generation packets). Otherwise: `save_end_ns = now`; wait publication; take video snapshot; audio window = `ExtractSegment(presentation_end_ns, ceil((end-start)/1ms))`; (dead multiband WAV path `write_pcm_wav`, `capture_engine.cpp:395-431,493-514`); `save_clip_to_file(...)`.

### 5.3 save_clip (`L/src/save_clip.cpp`) + transactional_save (`C/transactional_save.h`)
- `transactional_save::Run(final, writer)`: refuse if final exists (`DestinationExists`) or `<final>.partial` exists; writer writes to `.partial`; `fsync` file; `renameat2(RENAME_NOREPLACE)`; `fsync` parent dir (failure tolerated); on any failure remove temp and return typed `Failure` with `DescribeFailure` text. Exceptions from the writer are caught. `RunPair` (media + manifest sidecar, manifest published first) exists but is unused on Linux.
- `write_clip_to_temporary_file`: **plain MP4 (not fragmented)** via `avformat_alloc_output_context2(... "mp4")`; `avoid_negative_ts = DISABLED` (pre-roll packets before `presentation_start_pts` get negative pts so the clip presents from t=0 while the prior keyframe is retained for decode); metadata `comment = fthr-audio-mode=combined|separated`, `fthr_frame_rate = fps/1`, `fthr_video_bitrate_bps`; `movflags=use_metadata_tags`. Video stream: codec id from snapshot, yuv420p, BT.709 studio range, extradata copied with padding, `avg/r_frame_rate = fps`, `codecpar.bit_rate`; packets `pts/dts -= presentation_start_pts`, `duration = 1`, rescaled from 1/fps; `bytes_written` updated per packet.
- Audio: PCM float32 interleaved stereo 48 kHz -> `swr` FLT->FLTP -> native FFmpeg `aac` 128 kbps, time_base 1/48000, `frame_size` chunks, flushed; each setup failure degrades to video-only (`abandon_audio`) except `avcodec_parameters_copy` after the stream exists (fails the save). One combined audio track only; "separate audio" on Linux is just metadata — the UI finalises the layout (`docs/engine-startup.md` last para).
- Cleanup is idempotent; `avio_closep` failure reported.

### 5.4 Common replay helpers not used on Linux
`C/replay_pause_clock.h` (QPC-based pause compaction), `C/replay_health.h`, `C/capture_pipeline_health.h`, `C/replay_save_diagnostics.h` are Windows-only consumers (`FTHRcapture/FTHRclips/...`). Linux pause = simply not capturing (gap in media time). `C/capture_config_limits.h` and `C/replay_interval.h` and `C/transactional_save.h` are shared.

---

## 6. Audio

### 6.1 AudioCapture (`L/src/audio_capture.{h,cpp}`) — desktop loopback, PulseAudio **simple API**
- Constants: 48000 Hz, 2 ch, `PA_SAMPLE_FLOAT32LE`, 10 ms chunks (`kFramesPerChunk = 480`), `pa_simple_new(RECORD, source=<sink>.monitor, stream name "desktop-output-loopback", fragsize = 1 chunk, maxlength = 20 chunks)`.
- `StartForSink(sink, keep_seconds)` (`audio_capture.cpp:258-280`): resolves the sink's `monitor_source_name` through a temporary async `pa_context` (`resolve_sink_monitor`, 2 s timeouts, `PA_CONTEXT_NOAUTOSPAWN`); server unreachable -> `Start(sink + ".monitor")` (keeps retrying that name); sink unknown -> default; empty -> `Start("")` which follows the default sink.
- `Start` (`:207-256`): ring capacity `AudioRingCapacitySamples(keep_seconds)` = `(keep+10) s` of samples; opening the stream may fail non-fatally; capture thread + (when following default) a watcher thread re-resolving the default monitor every 2 s and handing a `pending_source_` to the capture thread.
- `CaptureLoop` (`:321-431`): switch source if pending (open new before closing old); if no stream, reopen every 1 s; blocking `pa_simple_read` of one chunk; `delivery_end = CLOCK_MONOTONIC`, `latency = pa_simple_get_latency` (-1 -> 0); `chunk.start_ns = AudioChunkStartNs(delivery_end, latency_us, frames, rate) = end - latency - frames/rate` (`audio_timing.cpp:7-17`); push, prune by sample count.
- `ExtractSegment(end_ns, duration_ms)` -> `AssembleAudioWindow` (`audio_timing.cpp:19-65`): lays blocks back to back; a block starting > 50 ms (`kAudioGapFillThresholdNs`) after the cursor is preceded by silence (reconnect gaps, audio starting after video); blocks straddling the window start are trimmed; result exactly `duration_ms` worth (zero-padded) or empty if nothing overlaps.
- Log markers the UI parses: `FTHR_STARTUP_WARNING: DESKTOP_AUDIO_UNAVAILABLE: ...` (`capture_engine.cpp:67`).

### 6.2 AudioMultiCapture (`L/src/audio_multi_capture.{h,cpp}`) — **dead on Linux today**
Threaded-mainloop async API: loads `module-null-sink` per category (`fthr_<name>`), records each `<sink>.monitor`, subscribes to sink-input events and moves apps to category sinks by `application.process.binary` substring match (first match wins, fallback = category with empty patterns), unloads modules on Stop (waits for completion). 120 s ring per category. Only reachable with `multiband_enabled`, which `main.cpp:103` forces false. `GetAudioMappingsJson` likewise unused.

### 6.3 Device listing (`L/src/audio_devices.{h,cpp}`, `audio_capture.cpp:150-178`)
`EnumeratePlaybackSinks`: `pa_context_get_server_info` (default sink) + `pa_context_get_sink_info_list`; endpoint_id = sink name, display_name = description or name, `device_state = 1`, `is_default`. JSON: `{"schema_version":1,"microphones":[{"endpoint_id":..,"display_name":..,"device_state":1,"is_default":bool}]}` (same key for both listings); error form `{"schema_version":1,"error":"..."}` on stderr exit 1. `JsonEscape` escapes `"\ \n \r \t` and <0x20/0x7f as `\u00xx`, UTF-8 passes.

---

## 7. Playback bridge (`C/ffmpeg_playback_bridge.{h,cpp}`, built as `libFTHRPlaybackMixer.so`, CMake `L/CMakeLists.txt:410-411,441-444`)
- Purpose: in-process multi-stem audio decoder/mixer for the clip editor; Python (`FTHR_UI/core/ffmpeg_playback.py:166-214`) `ctypes.CDLL` loads it from `_MEIPASS/engine/`, next to the executable, or `FTHRcapture_linux/build/`, and pulls PCM from a dedicated worker thread into a `QAudioSink`. No Qt, no capture/IPC symbols; shares the pinned FFmpeg ABI.
- C ABI (`ffmpeg_playback_bridge.h`): `fthr_playback_open(path_utf8, const int* stream_indexes, int n, char* err, size_t cap) -> FTHRPlaybackMixer*`; `fthr_playback_pull(mixer, float* interleaved_stereo, int frames, const float* gains, int gain_count, float master_gain) -> frames | 0 at EOF | -1 recoverable error`; `fthr_playback_set_playback_rate(mixer, float rate, int preserve_pitch, err, cap)` (atempo filter when preserve_pitch); `fthr_playback_seek(mixer, int64 ms, err, cap)`; `fthr_playback_is_eof`; `fthr_playback_source_failed(mixer, ordinal)`; `fthr_playback_close`. Output canonical 48 kHz float32 stereo; equal-power headroom + zero-lookahead limiter; crossfade of a few ms after seek to avoid clicks.
- FFmpeg APIs: avformat open/find_stream_info/read_frame/seek, avcodec decoders, swresample, libavfilter (buffersrc/buffersink/atempo graph).
- Test `playback_bridge_load_test` dlopens the .so with no FFmpeg preloaded to catch missing DT_NEEDED.

---

## 8. Tests (Rust test checklist)

CMake registration in `L/CMakeLists.txt` (all under `BUILD_TESTING`). Plain `assert`-based executables, no framework.

### L/tests
1. `audio_timing_test.cpp` (ctest `audio_timing_contract`): `AudioChunkStartNs` subtracts latency and block length, negative latency -> 0; `AssembleAudioWindow` joins jittered contiguous blocks, inserts silence for >50 ms gaps keeping later audio at capture time, pads leading silence when audio started late, trims straddling blocks keeping stereo interleave, returns empty when nothing overlaps; ring capacity = (keep+10)*rate*ch.
2. `capture_backend_candidates_test.cpp` (`capture_backend_candidates`): Wayland order wlr, ext, portal; non-Wayland x11grab only; `RecoveryCandidate` is identity; `BackendTypeName` strings; `ParseBackendOverride` accepts wlr/ext/portal/x11 case-insensitively and rejects "", "x11grab", null.
3. `capture_recovery_test.cpp` (`capture_recovery_policy`): backoff 250/750/1500 ms, exhaustion after 3, healthy generation (>= fps*10 frames) resets budget, backoff sleeps in 50 ms slices and cancellation stops it, stale backend shut down before replacement, no reconnect after shutdown; `EncodeFailurePolicy` threshold fps/2 (min 5) and reset on success; `NextEncodeDeadline` re-anchor after > 2 periods; transient failures retry at 2000 ms without consuming attempts.
4. `ext_repeat_policy_test.cpp` (`ext_repeat_policy` — registered under portal block? actually at :264): `ShouldRepeatLastFrame` true only for Timeout with a last frame.
5. `linux_audio_endpoint_json_test.cpp` (`linux_audio_endpoint_json`): exact JSON shape for empty and populated lists, display_name falls back to sink name, escaping of quotes/backslash/control chars, UTF-8 passthrough, single line, error JSON.
6. `linux_encoded_ring_duration_test.cpp` (`linux_encoded_ring_duration`): full window uses the keyframe before the window; variable timing + delayed publication (`WaitUntilPublished`); recovery `Clear` drops old generation; `TakeSnapshot(generation)` ignores other generations; byte budget drops whole GOPs from the front; never drops the current GOP; no budget + Clear resets counters.
7. `linux_encoder_candidates_test.cpp` (`linux_encoder_candidates`): Auto contains only hardware encoders in backend order (nvenc, vaapi/amf, qsv), codec filtering, Software pref yields only LGPL software encoders, unknown prefs never add software.
8. `linux_encoder_resize_test.cpp` (`linux_encoder_resize`, needs FFmpeg + software codec): encoder keeps output size while source changes size (larger with padding, smaller, back); rejects stride < width*4.
9. `linux_encoder_timestamp_test.cpp` (`linux_encoder_timestamps`): first pts 0, strictly increasing pts and wall times, keyframe at least every 2*fps(+1) ticks, capture gaps visible in media time (not frame-counter), media span ~= capture span within one frame.
10. `linux_engine_args_test.cpp` (`linux_engine_args`): CLI classification (listing flags alone, any `--` elsewhere = usage error, single `-` is a value), all defaults, empty strings = defaults silently, full 2.0 UI launch vector incl. buffer clamp to 900 with exactly one warning, invalid/out-of-range handling per slot.
11. `linux_vaapi_encoder_test.cpp` (`linux_vaapi_encoder`): `ApplyPreset("h264_vaapi")` leaves `compression_level == -1`; if a VA-API device is present, opens and encodes real frames producing packets (skips gracefully otherwise).
12. `playback_bridge_load_test.cpp` (`playback_bridge_load`): dlopen of the mixer .so resolves all `fthr_playback_*` symbols without preloaded FFmpeg.
13. `portal_screencast_test.cpp` (`portal_screencast_helpers`, needs libdbus headers/lib): request/session path derivation from unique names; restore-token load/save/delete, 0600 perms, rejection of damaged tokens; `LoadPortalScreenCastOptions`; `ShouldRetryPortalStartup` (attempt 0 with token only); restored session retries once without token; PipeWire `Interrupted` does not retry; dialog cancellation does not retry; `ParsePortalResponse` on in-process built messages (code, session_handle, restore_token, streams with size/position/source_type, unknown keys skipped); `DescribeSpaVideoFormat`/`CopyFrameRows` pixel glue incl. R/B swap and padding drop.
14. `shm_frame_test.cpp` (`shm_frame_normalization`): wl_shm -> AV_PIX_FMT mapping; format preference order; BGR0/BGRA pass through without copy; RGB-order inputs swapped to BGR; y_invert flips rows; 10-bit converted; bad input (null, zero size, short stride, unknown format) rejected.
15. `short_save_mp4_integration_test.cpp` (`short_save_actual_mp4`, real FFmpeg mux): encodes 10 s synthetic video, snapshot of last 5 s is full history, starts on a keyframe, `presentation_start_pts == 7*fps`, physical pre-roll has negative pts but stream `start_time == 0`, saved MP4 duration == 5 s within a frame, transactional commit produced the final file.
16. `transactional_save_test.cpp` (`transactional_save_contract`, Linux copy): success path, writer failure cleans temp, trailer failure never commits, rename failure is an error, cleanup failure preserves original error, existing destination untouched, missing temporary cannot be committed, default filesystem commit and no-replace, **sync ordering (file fsync before rename, dir fsync after)** and sync failure reported as writer failure.
17. `wayland_dispatch_test.cpp` (`wayland_bounded_dispatch`, pipe-backed fake display): event before deadline; no event -> Timeout without busy spin; shutdown cancels a prepared read promptly; hangup -> Disconnected; pending events dispatched before retrying prepare_read; bounded roundtrip completes on sync callback; init roundtrip timeout cancels the read.
18. `windows_audio_interval_test.cpp`, `windows_encoded_ring_duration_test.cpp`: Windows ring/audio policies compiled on Linux for portability (not Linux behaviour).
19. `x11_capture_target_test.cpp`, `x11_frame_converter_test.cpp`: X11 only (note).

### C/tests
20. `replay_interval_test.cpp` (`replay_interval_contract`): normal durations/frame rates give exact windows; keyframe boundaries (decode start at keyframe before window); insufficient/reset history -> partial from first keyframe; irregular timestamps and non-zero epoch; ring and save boundaries; rapid saves independent and non-mutating; invalid inputs and presentation offset math.
21. `raw_fixed_rate_duration_test.cpp` (Windows raw-ring count model, `SelectFixedRateFrames`): partial physical history classification.
22. `transactional_save_test.cpp` (common copy; same cases as 16 minus the two sync tests).

Python-side gates relevant to the engine: `tools/verify_shared_memory_contract.py` (also run from pytest `tests/test_shared_memory_contract.py`), `tools/verify_engine_response_contract.py`.

---

## 9. Known bugs, TODOs, hacks, suspicious logic

1. **No memory ordering on the shm protocol.** All fields are plain non-volatile, non-atomic stores/loads on both sides (`main.cpp:184-367`, `shared_memory.h:57-72` only documents program order). The compiler may legally reorder `set_engine_string` and `engine_response` stores; the Rust port should use `AtomicU32` with Release/Acquire for `ui_command`/`engine_response` and fence the payload.
2. **`PR_SET_PDEATHSIG` is per parent *thread*** (`main.cpp:171`). If the UI ever spawns the engine from a worker thread that later exits, the engine gets SIGTERM spuriously. Prefer a pidfd/pipe-based parent watch.
3. **`shm_open` without `O_EXCL` + `memset 0`** (`shared_memory.cpp:15,40`): a second engine instance silently clobbers a live region and later unlinks it. Name is not per-user/per-session either.
4. **SaveClip blocks the command loop** (`main.cpp:213`): status fields freeze, pause/resume/shutdown queue behind a mux that can take many seconds for 15-min/200 Mbps buffers; SIGTERM only honoured afterwards.
5. **wlr-screencopy v1/v2 compositors can never finish Initialize**: the probe waits for `buffer_done` (a v3 event) or `failed` (`backend_wlr.cpp:323-329, 385-391`) while binding `min(version,3)`; on a v2 compositor it times out after 5 s and the backend is skipped.
6. **Cursor inconsistency**: wlr `overlay_cursor=0` and ext `options=0` never paint the cursor; portal `cursor_mode=2` embeds it (`backend_wlr.cpp:316,378`, `backend_ext.cpp:273`, `portal_screencast.h:112`). No user setting.
7. **Output transforms ignored**: ext `transform` event and wlr flags other than y_invert are dropped (`backend_ext.cpp:99`, `backend_wlr.cpp:92-96`); rotated outputs encode rotated/mirrored.
8. **No DMA-BUF import anywhere**; portal explicitly fails when only modifier formats are offered (`backend_portal.cpp:221-224`). Every path is a CPU copy (memfd -> swscale -> encoder; VA-API then uploads again). At 4K/240 fps this is the throughput ceiling.
9. **wlr backend captures unthrottled** (one request per loop iteration, no pacing); the engine throttle discards surplus frames after a full compositor copy (`backend_wlr.cpp:371`, `capture_engine.cpp:310-312`). ext/portal pace on `last_delivery_`.
10. **Frame timestamps = "now at completion"** for wlr/ext (`backend_wlr.cpp:428-430`, `backend_ext.cpp:399-402`); compositor presentation times (`ready` tv_sec/nsec, ext `presentation_time`) ignored. A/V sync depends on the copy latency being small.
11. **`EncodeFrame` ignores `RawFrame::av_pix_fmt`** and always scales as BGRA (`encoder.cpp:251`, `:317-323`); correctness rests on the normaliser only ever producing BGR0/BGRA.
12. **VA-API preset deliberately unmapped** (`encoder.cpp:49-56`, "Known limitation"); the hardening spec 1.5 wanted compression_level 7..1 but the test `linux_vaapi_encoder_test.cpp:15-30` now pins "unset". Preset slider is a no-op on the qualified AMD path.
13. **Letterbox "fit" doesn't add bars**; it only shrinks one dimension (`capture_engine.cpp:213-221`), so the output size differs from the requested target.
14. **`SET_RESOLUTION`/`SET_QUALITY` silently accept and do nothing** (`main.cpp:253-264`); `cfg_target_*` then lies about the running config.
15. **`Reconfigure` destroys audio history** (`capture_engine.cpp:537-569`: `Shutdown()` stops audio, ring is re-created) and briefly reports `nvenc_active=false`/empty codec; `RECONFIGURE_ENCODER` answers STATUS_UPDATE even if the restart later fails.
16. **Save refused while paused** (`capture_engine.cpp:439-442`): a focus-paused UI cannot save the buffer it just stopped filling; UI must resume first.
17. **`pa_simple_read` is unbounded** (`audio_capture.cpp:393`); a wedged PulseAudio/PipeWire-Pulse server blocks `Stop()`/join and thus engine shutdown (UI then SIGKILLs).
18. **Dead code**: `AudioMultiCapture`, `write_pcm_wav`, multiband WAV sidecars, `GetAudioMappingsJson`, `active_audio_mappings`/`multiband_enabled` fields, `transactional_save::RunPair`, `ext-foreign-toplevel-list` protocol (generated and linked, never bound), `zxdg_output_manager_v1` (bound, unused), `engine_param2/3`, `ui_param2/3`, `is_recording`.
19. **`renameat2(RENAME_NOREPLACE)`** (`transactional_save.h:139-149`) returns EINVAL on some filesystems (older overlayfs/FUSE/NFS); there is no `link()+unlink()` fallback, so saves to such mounts always fail with "Could not atomically finalize clip".
20. **Fixed `pkt->duration = 1`** for every video packet (`save_clip.cpp:293`) though PTS deltas vary with the wall-clock scheme; harmless for players but `avg_frame_rate` is only nominal.
21. **AAC setup failure silently drops audio** (`save_clip.cpp:160-168`); the UI is not told the clip is video-only.
22. `AudioCapture` default-output watcher creates a new `pa_context` every 2 s for the whole session (`audio_capture.cpp:301-317`): cheap but noisy; a subscription would be the right tool.
23. `SampleContent` indexes `frame.data` with `stride` but the stride check is `< width*4` only; fine today, but a Rust port should bound-check rows explicitly.
24. The 2 s `kFailurePublishGrace` exit path (`main.cpp:344-352`) races the UI's 500 ms poll; if the save poller is holding `engine_string` (save pending), the failure reason is never published before exit (`main.cpp:319-324`).
25. Portal `OpenPortalSession` logs that `target_output` cannot be preselected; KNOWN_ISSUES.md:7 notes the UI shows "CAPTURE STALLED" during the picker (the engine is in RECOVERING/first-start with no frames). The 120 s dialog timeout then becomes `UserDeclined` and the engine exits.
26. `ExtBackend::AllocShmBuffer` ignores the compositor's `buffer_size` changing between `buf_width_` capture and `done` (`backend_ext.cpp:61-67, 122-142`); only serial bump + realloc on the next call covers it.
27. `AudioCapture::Start` when an explicit source fails to open still returns true and keeps retrying the given name forever (by design) but never surfaces a typed status to the UI beyond the one startup warning line.
28. Spec item "Commands 4-9 respond STATUS_UPDATE" is implemented via `default:` placed before the recording cases purely to appease the static response-order checker (`main.cpp:266-275`) — fragile coupling between source layout and a regex tool.

---

## 10. External dependencies

### 10.1 Build (`L/CMakeLists.txt`)
- C++20, `-Wall -Wextra`; options `FTHR_X11_CAPTURE` (default ON), `FTHR_PORTAL_BACKEND`; defines `FTHR_X11_CAPTURE=0/1`, `FTHR_PORTAL_BACKEND=0/1`, `_REENTRANT` (portal).
- pkg-config: `libavcodec libavfilter libavformat libavutil libavdevice libswscale libswresample` (`:188-190`), `wayland-client` (`:236`), `libpulse-simple libpulse` (`:238`), headers-only `libpipewire-0.3 dbus-1` (`:243`); `wayland-scanner` program (`:294`); link `pthread dl rt`.
- FFmpeg: pinned LGPL BtbN build (spec: n8.1.2-34, LGPLv3) via `FTHR_FFMPEG_ROOT` (`tools/fetch_third_party.py --ffmpeg-linux`, manifest `tools/ffmpeg_manifest_linux.json`), RPATH `$ORIGIN/../third_party/ffmpeg/lib`; distro FFmpeg allowed for dev builds. Needs encoders available in that build: nvenc/vaapi/amf/qsv, libopenh264, libkvazaar, libsvtav1, libaom, librav1e, native aac.
- Runtime dlopen: `libva.so.2` (probe), `libpipewire-0.3.so.0`, `libdbus-1.so.3`.
- glibc <= 2.35 gate for the AppImage (`tools/verify_linux_glibc.py`).

### 10.2 Wayland protocol XMLs (`L/protocols/`, generated with wayland-scanner at build; pre-generated copies also checked in)
- `wlr-screencopy-unstable-v1.xml`: `zwlr_screencopy_manager_v1` v3, `zwlr_screencopy_frame_v1` v3 (used).
- `ext-image-copy-capture-v1.xml`: manager/session/frame v1 (+ cursor_session, unused).
- `ext-image-capture-source-v1.xml`: `ext_image_capture_source_v1`, `ext_output_image_capture_source_manager_v1` (used), `ext_foreign_toplevel_image_capture_source_manager_v1` (unused).
- `xdg-output-unstable-v1.xml`: `zxdg_output_manager_v1` v3 (bound, unused).
- `ext-foreign-toplevel-list-v1.xml`: compiled, unused.
- Core: `wl_compositor` (<=4), `wl_shm` (1), `wl_output` (<=4, needs `name` event i.e. v4), `wl_registry`, `wl_callback`.

### 10.3 D-Bus / PipeWire / PulseAudio surfaces
- `org.freedesktop.portal.Desktop` `/org/freedesktop/portal/desktop`: `org.freedesktop.DBus.Properties.Get(ScreenCast,version)`, `org.freedesktop.portal.ScreenCast.{CreateSession,SelectSources,Start,OpenPipeWireRemote}`, `org.freedesktop.portal.Request.{Response signal, Close}`, `org.freedesktop.portal.Session.{Closed signal, Close}`.
- PipeWire: `pw_init, pw_thread_loop_*, pw_context_new/connect_fd, pw_core_disconnect, pw_properties_new, pw_stream_new/add_listener/connect/disconnect/update_params/dequeue_buffer/queue_buffer/get_state/state_as_string`, SPA pod builder, `spa_format_parse`, `spa_format_video_raw_parse`, `spa_buffer_find_meta_data`.
- PulseAudio: simple API `pa_simple_new/read/get_latency/free`; async `pa_mainloop_*`, `pa_context_connect/get_server_info/get_sink_info_by_name/get_sink_info_list`; (dead multi path) `pa_threaded_mainloop_*`, `pa_context_load_module/unload_module/subscribe/move_sink_input_by_index`, `pa_stream_*`.
- Kernel: `memfd_create`, `mmap`, `shm_open`, `renameat2`, `prctl(PR_SET_PDEATHSIG)`, `clock_gettime(CLOCK_MONOTONIC)`, `poll`.

### 10.4 Rust crate mapping (suggestion)
wayland-client + wayland-protocols(-wlr) (+ ext-image-copy-capture bindings via wayland-scanner), ashpd or zbus (portal), pipewire-rs, libpulse-binding / libpulse-simple-binding, ffmpeg-sys-next or ffmpeg-next (must stay LGPL-pinned ABI), memmap2 + shared_memory for shm, nix (prctl, memfd, renameat2), atomic types for the IPC header.
