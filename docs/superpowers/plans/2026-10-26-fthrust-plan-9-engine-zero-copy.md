# Plan 9: Engine zero-copy, disk buffer, AV1 bitrates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Take the capture engine as far down in CPU and memory as it will go without lowering quality. The CPU should touch no pixels on the common path. The replay buffer can live on disk. AV1 gets a bitrate table that matches H.264 quality with fewer bits.

**Architecture:**
- **CPU.** The ext-image-copy path stops copying every frame three times (shm → `last_frame` → staging → GPU). First it drops to one copy: a double-buffered shm pool, plus an upload straight into a mapped VA surface. A static screen re-sends the last encoded surface with no copy at all.
- **Zero copy.** When the compositor offers dmabuf capture on the encoder's own GPU, the engine allocates BGRX VA surfaces, exports them as dmabufs, and the compositor renders into them directly. The CPU never touches a pixel, and shm stays as the fallback.
- **Memory.** The ring keeps only clip + one GOP + 0.5 s. An opt-in disk store (`replay_buffer_location = "disk"`, default `"memory"`) keeps packet payloads in segment files, and the ring holds only metadata in RAM.
- **AV1.** A measured bitrate table gives AV1 the lowest bitrate whose VMAF matches H.264 at each level.

**Tech Stack:** Rust 1.98.1, ffmpeg-next 9 / FFmpeg n9.0.2 (`av_hwframe_map`, `AV_PIX_FMT_DRM_PRIME`, `scale_vaapi`, `libvmaf`), wayland-protocols 0.32 (`ext_image_copy_capture_v1`, `zwp_linux_dmabuf_v1`), libc.

**Spec:** this conversation's measurements and decisions; there is no separate design doc.
- **Baseline after Plan 8**, live on the user's desktop (2026-10-25): RX 7900 XTX (`renderD128`) plus an integrated Raphael GPU, Hyprland, DP-3 1080p180, 50 Mbps, 45 s clips.
  - Engine: 25% of a core, 428 MB RSS / 300 MB anon. Window: 184 MB.
  - Per-thread: `fthrust-capture` 21.5%, `fthrust-en:cs0` 1.0%.
- **Copies per frame on the ext path:**
  - `ExtImageCopy::store_frame` copies the shm buffer into `last_frame` (`crates/fthrust-wayland/src/ext_capture.rs:467`).
  - `VaapiConverter` copies rows into a staging `BGR0` frame (`ffi/vpp.rs`).
  - `av_hwframe_transfer_data` uploads it (`ffi/vpp.rs:232`).
- **Encode-path CPU** (`tests/encoder_throughput.rs`): GPU path 0.50 ms/frame, CPU path 3.29 ms/frame.
- **User decisions (2026-10-26):**
  - Disk buffer ships as a setting, **default Memory**.
  - AV1: measure with VMAF and use the matched bitrates **when AV1 is selected**. **H.264 stays the default.**

## Global Constraints

- Output colour unchanged: BT.709, limited (TV) range, NV12, left chroma siting, as `RawEncoder::open` tags it.
- Every new fast path has a fallback that keeps today's behaviour. The fallback logs once at `info` with a `reason` field, using exactly these lines:
  - `zero-copy capture unavailable; copying frames`
  - `mapped upload unavailable; using transfer`
  - `disk replay buffer unavailable; keeping it in memory`
- `FTHR_NO_ZERO_COPY=1` disables dmabuf capture. `FTHR_CPU_COLOR_CONVERT=1` keeps its Plan 8 meaning, and it also disables zero copy.
- No new crate dependencies. `libc` is already in `[workspace.dependencies]`; add it to a crate's own `Cargo.toml` only where that crate needs it.
- `fthrust-wayland` must not depend on FFmpeg. Dmabufs cross the crate boundary as plain fds plus metadata (`DmabufImage`).
- Replay files are private:
  - Directory mode `0700` and files `0600`.
  - Location: `$XDG_CACHE_HOME/fthrust/replay/<pid>/`, or `~/.cache/fthrust/replay/<pid>/` when `XDG_CACHE_HOME` is unset.
  - The engine removes its own directory on clean exit. On start it removes directories whose pid is no longer running.
- Settings key `replay_buffer_location`: `"memory"` (default; unknown values also fail closed to memory) or `"disk"`.
- Tests that need VA-API skip with `eprintln!("skipping: …"); return;` when `vaapi_available()` is false. Tests that need a live compositor are `#[ignore]`. CI is ubuntu-24.04 with no `/dev/dri`.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace` and `cargo deny check` stay green.

## Review Focus

1. **Two GPUs** (this machine has a 7900 XTX and an integrated GPU). The compositor's `dmabuf_device` can name a different GPU from the encoder's render node. Zero copy must then stay off. Test in Task 6 (`zero_copy_off_for_other_gpu`).
2. **Output size or mode change mid-capture** on the dmabuf path. Surfaces must be reallocated at the new size, or the engine falls back to shm; output size stays fixed. Test in Task 6 (`dmabuf_geometry_change_reallocates_or_falls_back`).
3. **Compositor without dmabuf in ext-image-copy, or a modifier it does not list** (wlr screencopy, KDE, older Hyprland). Behaviour must be exactly today's shm path. Test in Task 6 (`zero_copy_off_without_constraints`, `zero_copy_off_when_modifier_not_offered`).
4. **Disk mode with the cache directory unwritable or full.** The ring falls back to memory and keeps capturing; it never stops capture. Test in Task 9 (`unwritable_cache_falls_back_to_memory`).
5. **Crash leaves footage on disk.** The next start removes stale pid directories, but not the directory of a live engine. Test in Task 9 (`stale_replay_dirs_are_removed`).
6. **Static screen:** a repeated frame must keep pts strictly increasing, and must not break the next real frame or a forced keyframe. Test in Task 3 (`repeats_keep_pts_monotonic_and_keyframes`).

---

## File Structure

| File | Responsibility |
|---|---|
| Modify `crates/fthrust-wayland/src/shm.rs` | `MemfdPool` holds two buffer slots; `data(slot)`. |
| Modify `crates/fthrust-wayland/src/ext_capture.rs` | Alternate shm slots (no `store_frame` copy); dmabuf negotiation and dmabuf buffers. |
| Modify `crates/fthrust-wayland/src/frame.rs` | `FrameView.repeated`, `FrameView.source: FrameSource`. |
| Create `crates/fthrust-wayland/src/dmabuf.rs` | `DmabufImage`, `DmabufConstraints`, `zwp_linux_dmabuf_v1` buffer creation, format/modifier matching. |
| Modify `crates/fthrust-wayland/src/screencopy.rs` | Sets `repeated`/`source` on its views (shm only). |
| Modify `crates/fthrust-media/src/ffi/vpp.rs` | Mapped upload; `convert_surface`; repeat of the last output. |
| Create `crates/fthrust-media/src/ffi/capture_surfaces.rs` | `CaptureSurfaces`: BGRX VA surfaces exported as dmabufs, `download` for the watchdog. |
| Modify `crates/fthrust-media/src/encoder.rs` | `encode_repeat`, `dmabuf_targets`, `encode_surface`. |
| Modify `crates/fthrust-media/src/ring.rs` | Time window helper; payload store selection. |
| Create `crates/fthrust-media/src/payload.rs` | `Payload` (memory or disk segment ref), `SegmentStore`. |
| Modify `crates/fthrust-media/src/recorder.rs`, `src/playback/proxy.rs`, the save writer in `crates/fthrust-engine/src/save.rs` | Read payloads via `Payload::with_bytes`. |
| Modify `crates/fthrust-engine/src/engine.rs` | Zero-copy decision and loop wiring; repeats; disk ring construction. |
| Create `crates/fthrust-engine/src/zero_copy.rs` | `zero_copy_plan` decision function. |
| Create `crates/fthrust-engine/src/replay_dir.rs` | Replay dir path, creation, stale cleanup, removal. |
| Modify `crates/fthrust-core/src/capture_config.rs` | `BufferLocation`, ring window and budget, AV1 bitrate table. |
| Modify `crates/fthrust-ui/src/popups/capture_settings.rs` | "Replay buffer" row. |
| Create `tools/bitrate-match/run.sh`, `tools/bitrate-match/README.md` | VMAF ladder measurement. |

---

### Task 1: Double-buffered shm, no per-frame copy

**Files:**
- Modify: `crates/fthrust-wayland/src/shm.rs`, `crates/fthrust-wayland/src/ext_capture.rs`, `crates/fthrust-wayland/src/frame.rs`, `crates/fthrust-wayland/src/screencopy.rs`
- Test: unit tests in `shm.rs` and `ext_capture.rs` (pure slot logic); `crates/fthrust-wayland/tests/` if a live-compositor test exists there, `#[ignore]`

**Interfaces:**
- Produces:
  - `FrameView { …, pub repeated: bool, pub source: FrameSource }`, with `pub enum FrameSource { Shm, Dmabuf(usize) }`.
  - `is_encodable_as_is` is false for `Dmabuf`, and `normalize` returns a new `FrameError::NotInMemory` for it.
  - `MemfdPool` keeps two `wl_buffer`s of the same geometry in one memfd (`2 × stride × height` bytes).
  - `ensure(qh, geometry, slot: usize) -> Result<&WlBuffer>` and `data(slot: usize) -> &[u8]`.
- Behaviour:
  - `ExtImageCopy` requests each frame into the slot not currently delivered, and `last_view` points at the delivered slot. `store_frame` no longer copies pixels and keeps only geometry, transform and timestamps. `last_frame: Vec<u8>` is removed.
  - Repeated views set `repeated = true`.
  - `screencopy.rs` sets `repeated` where it repeats, and `source = Shm`.

- [ ] **Step 1: Write the failing tests:**
  - `slots_alternate_and_view_points_at_delivered_slot`: a pure `SlotState` helper with `next_request()` and `delivered(slot)`. After delivering slot 0, the next request uses slot 1, and the view slot is 0.
  - `pool_slots_do_not_overlap`: `data(0)` and `data(1)` address disjoint ranges.
  - `normalize_rejects_dmabuf_views`.
- [ ] **Step 2: Run** `cargo test -p fthrust-wayland`. Expected: compile failure.
- [ ] **Step 3: Implement** as above, and update every `FrameView { … }` construction.
- [ ] **Step 4: Run** `cargo test -p fthrust-wayland -p fthrust-engine`. Expected: pass.
- [ ] **Step 5: Commit** `perf(wayland): double-buffer shm captures instead of copying each frame`

---

### Task 2: Upload straight into a mapped VA surface

**Files:**
- Modify: `crates/fthrust-media/src/ffi/vpp.rs`, `crates/fthrust-media/src/ffi/hw.rs` (CPU path upload)
- Test: `crates/fthrust-media/tests/vpp.rs`, `crates/fthrust-media/tests/encoder_throughput.rs`

**Interfaces:**
- Produces: `VaapiConverter::convert` keeps its signature. It gets a fresh BGRX surface from the pool and calls `av_hwframe_map(mapped, surface, AV_HWFRAME_MAP_WRITE | AV_HWFRAME_MAP_OVERWRITE)`. Then it copies rows from `bgra` (respecting `stride`) into the mapped planes, and unmaps. If mapping fails once, it logs `mapped upload unavailable; using transfer` and uses the Plan 8 staging + transfer path for the rest of the converter's life.
- The CPU path's NV12 upload (`VaapiFrames::upload`) gets the same mapped write with the same fallback.
- Test hook: `#[doc(hidden)] pub fn force_transfer_upload()` on `Encoder`, for the comparison.

- [ ] **Step 1: Write the failing test** `mapped_and_transfer_uploads_match`. Convert the same strided frame with each path, then download both through `gpu_convert_rgb_with_stride`'s readback. The results must be byte-identical. Extend `encoder_throughput` to print a third line, `gpu-transfer: … ms CPU/frame`.
- [ ] **Step 2: Run** `cargo test -p fthrust-media --test vpp`. Expected: failure (no hook).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p fthrust-media --test vpp --test encoder`, then `cargo test -p fthrust-media --test encoder_throughput -- --ignored --nocapture`.
  - Expected: `gpu` is lower than `gpu-transfer`.
  - If the mapped write is **slower** on radeonsi (write-combined VRAM), keep transfer as the default and mapped as an opt-in. Report both numbers in the commit message either way.
- [ ] **Step 5: Commit** `perf(media): write captured rows straight into the VA surface`

---

### Task 3: Static screen re-sends the last frame

**Files:**
- Modify: `crates/fthrust-media/src/encoder.rs`, `crates/fthrust-media/src/ffi/vpp.rs`, `crates/fthrust-engine/src/engine.rs` (encode loop)
- Test: `crates/fthrust-media/tests/encoder.rs`, `crates/fthrust-engine/tests/engine_core.rs`

**Interfaces:**
- Consumes: `FrameView.repeated` (Task 1).
- Produces: `Encoder::encode_repeat(&mut self, ts_ns: i64, out: &mut Vec<EncodedPacket>) -> Result<bool>`.
  - It sends a new reference to the last frame handed to the codec, either the GPU output surface or the CPU path's NV12 hw frame. It computes the pts and keyframe exactly as `encode` does: a pending forced keyframe or a keyframe request applies to the repeat.
  - It returns `Ok(false)`, sending nothing, when there is no previous frame or the last frame's size differs from the encoder's input. The caller then uses `encode`.
- Engine: when `view.repeated`, it calls `encode_repeat` and falls back to `encode` on `false`. The watchdog keeps observing as today.

- [ ] **Step 1: Write the failing tests:**
  - `repeats_keep_pts_monotonic_and_keyframes`, on VA-API: encode 5 frames, repeat 5, request a keyframe, repeat 1, then encode 5. Packet pts must be strictly increasing. The packet after the keyframe request must be a keyframe. Decoding frame 7 must give the same RGB as frame 4, within ±2.
  - `repeat_without_previous_frame_returns_false`.
  - Engine: `static_screen_repeats_do_not_convert`. Use the synthetic backend with a static scene if it supports repeats; otherwise add `repeated` to its views. Over 30 repeated frames, the converter count stays flat; add a `#[doc(hidden)] Encoder::conversions() -> u64` counter.
- [ ] **Step 2: Run** them. Expected: compile failure.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p fthrust-media --test encoder` and `cargo test -p fthrust-engine`. Expected: pass.
- [ ] **Step 5: Commit** `perf(engine): repeat the last encoded surface on a static screen`

---

### Task 4: Dmabuf capture buffers in `fthrust-wayland`

**Files:**
- Create: `crates/fthrust-wayland/src/dmabuf.rs` (register in `lib.rs`)
- Modify: `crates/fthrust-wayland/src/ext_capture.rs`, `crates/fthrust-wayland/src/connection.rs` (bind `zwp_linux_dmabuf_v1`, version ≥ 4)
- Test: unit tests in `dmabuf.rs`; `#[ignore]` live test `ext_capture_offers_dmabuf_constraints` in `crates/fthrust-wayland/tests/`

**Interfaces:**
- Produces, in `fthrust_wayland::dmabuf`:
  - `pub struct DmabufPlane { pub fd: std::os::fd::OwnedFd, pub offset: u32, pub stride: u32 }`
  - `pub struct DmabufImage { pub width: u32, pub height: u32, pub fourcc: u32, pub modifier: u64, pub planes: Vec<DmabufPlane> }`
  - `pub struct DmabufConstraints { pub device: u64 /* dev_t */, pub formats: Vec<(u32, Vec<u64>)> }`, built from the session's `dmabuf_device` and `dmabuf_format` events.
  - `pub fn offers(c: &DmabufConstraints, fourcc: u32, modifier: u64) -> bool`
  - `pub const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258` and `DRM_FORMAT_ARGB8888: u32 = 0x3432_5241`. These are byte order B,G,R,X in memory, the same layout as `PixelLayout::Bgrx`.
- In `ExtImageCopy`:
  - `pub fn dmabuf_constraints(&self) -> Option<DmabufConstraints>`
  - `pub fn use_dmabuf(&mut self, images: Vec<DmabufImage>) -> Result<(), CaptureFailure>` builds one `wl_buffer` per image with `zwp_linux_buffer_params_v1.create_immed`. The images are used round-robin, never re-attaching the most recently delivered one.
  - `pub fn use_shm(&mut self)` switches back.
  - In dmabuf mode, `capture()` returns views with `source = FrameSource::Dmabuf(index)`, `data = &[]` and the image's geometry.
  - A `BufferConstraints` failure or a geometry change in dmabuf mode returns `CaptureFailure::DmabufInvalidated`, a new variant. The engine handles it (Task 6).

- [ ] **Step 1: Write the failing unit tests:**
  - `offers_matches_format_and_modifier`
  - `offers_rejects_unlisted_modifier`
  - `constraints_collect_device_and_formats`: feed the event handler's pure accumulator the `dmabuf_device` bytes (a native-endian `dev_t`) and two `dmabuf_format` events.
- [ ] **Step 2: Run** `cargo test -p fthrust-wayland`. Expected: compile failure.
- [ ] **Step 3: Implement.** Store the dmabuf events beside the existing `shm_formats` handling (`ext_capture.rs:112`).
- [ ] **Step 4: Run** `cargo test -p fthrust-wayland`. Then run `cargo test -p fthrust-wayland --test <live> -- --ignored --nocapture` inside the uidrive bench's cage if cage offers ext-image-copy dmabuf. Otherwise note in the report that the live check happens in Task 11.
- [ ] **Step 5: Commit** `feat(wayland): negotiate dmabuf buffers for ext-image-copy capture`

---

### Task 5: Export VA surfaces as dmabufs and encode them

**Files:**
- Create: `crates/fthrust-media/src/ffi/capture_surfaces.rs` (register in `ffi/mod.rs`)
- Modify: `crates/fthrust-media/src/ffi/vpp.rs`, `crates/fthrust-media/src/encoder.rs`, `crates/fthrust-media/src/ffi/hw.rs`
- Test: `crates/fthrust-media/tests/capture_surfaces.rs`

**Interfaces:**
- Produces:
  - `pub(crate) struct CaptureSurfaces`, created by `CaptureSurfaces::new(device: &VaapiFrames, width: u32, height: u32, count: usize) -> Result<Self>`. It allocates `count` BGR0 surfaces from `device.pool_on(..)`, maps each with `av_hwframe_map` to `AV_PIX_FMT_DRM_PRIME`, and dups the object fds.
    - `fn images(&self) -> Result<Vec<ExportedImage>>`, where `pub struct ExportedImage { pub width: u32, pub height: u32, pub fourcc: u32, pub modifier: u64, pub planes: Vec<(OwnedFd, u32 offset, u32 pitch)> }`. This is the `fthrust-media` mirror of `DmabufImage`; the engine converts between them.
    - `fn surface(&self, index: usize) -> &OwnedFrame`
    - `fn download(&self, index: usize, out: &mut Vec<u8>) -> Result<usize /* stride */>`, for the watchdog.
  - `VaapiConverter::convert_surface(&mut self, src: &OwnedFrame, out: &mut OwnedFrame) -> Result<()>`: the existing graph without the upload.
  - On `Encoder`:
    - `pub fn dmabuf_targets(&mut self, width: u32, height: u32, count: usize) -> Result<Vec<ExportedImage>>`. It errors unless the encoder is on `Conversion::Gpu`, and it rebuilds the converter for that source size.
    - `pub fn encode_surface(&mut self, index: usize, ts_ns: i64, out: &mut Vec<EncodedPacket>) -> Result<()>`, with the same pts, keyframe and fallback rules as `encode`. A failure on the first surface frame returns `Err`; the engine falls back.
    - `pub fn download_surface(&self, index: usize, out: &mut Vec<u8>) -> Result<usize>`
    - `pub fn render_node(&self) -> Option<&str>`
  - `fthrust_media::vaapi::render_node_dev(path: &str) -> Option<u64>`: the `st_rdev` of the node. Also `fthrust_media::vaapi::same_gpu(a: u64, b: u64) -> bool`, which compares through `/sys/dev/char/<maj>:<min>/device`, so a primary node (`card1`) and its render node (`renderD128`) match.

- [ ] **Step 1: Write the failing tests:**
  - `surfaces_export_one_plane_bgrx_dmabufs`: 4 surfaces at 1920×1080; each image has `fourcc == DRM_FORMAT_XRGB8888`, one plane, and pitch ≥ 1920×4.
  - `encoding_an_uploaded_surface_matches_the_shm_path`: upload a solid red frame into surface 0 through the mapped writer, `encode_surface(0, …)`, decode, and expect RGB within ±6 of `[255,0,0]`.
  - `same_gpu_matches_card_and_render_node`: compare `/dev/dri/renderD128` with the `card*` that shares its `/sys/.../device`; skip when there is no `/dev/dri`.
  - `dmabuf_targets_require_gpu_conversion`: `open_forcing_cpu_conversion`, then `dmabuf_targets` must be `Err`.
- [ ] **Step 2: Run** `cargo test -p fthrust-media --test capture_surfaces`. Expected: compile failure.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p fthrust-media`. Expected: pass.
- [ ] **Step 5: Commit** `feat(media): export capture surfaces as dmabufs and encode from them`

---

### Task 6: Engine uses zero copy when it can

**Files:**
- Create: `crates/fthrust-engine/src/zero_copy.rs` (register in `lib.rs`)
- Modify: `crates/fthrust-engine/src/engine.rs` (backend setup and the encode loop)
- Test: unit tests in `zero_copy.rs`; `crates/fthrust-engine/tests/engine_core.rs`

**Interfaces:**
- Consumes: Task 4 (`dmabuf_constraints`, `use_dmabuf`, `use_shm`, `FrameSource::Dmabuf`, `CaptureFailure::DmabufInvalidated`) and Task 5 (`dmabuf_targets`, `encode_surface`, `download_surface`, `render_node`, `render_node_dev`, `same_gpu`).
- Produces: `pub enum ZeroCopy { Use, Off(&'static str) }` and `pub fn zero_copy_plan(constraints: Option<&DmabufConstraints>, encoder_gpu: Option<u64>, gpu_conversion: bool, image: Option<(u32 fourcc, u64 modifier)>, env_disabled: bool) -> ZeroCopy`. It returns `Off` with a reason for each of:
  - env disabled;
  - no GPU conversion;
  - no constraints;
  - a different GPU;
  - format or modifier not offered.
- Behaviour:
  - After the encoder opens on an ext-image-copy backend, the engine requests 4 `dmabuf_targets` at the native size and runs `zero_copy_plan`. On `Use` it calls `use_dmabuf`.
  - On `Off`, or on the first `encode_surface` error, it calls `use_shm` and logs `zero-copy capture unavailable; copying frames` with `reason`, once per capture start.
  - On `DmabufInvalidated` the engine reallocates targets at the new size once. If that fails it falls back to shm.
  - The watchdog observes every `max(1, fps / 6)`-th dmabuf frame through `download_surface` into a reused buffer.
  - Repeated dmabuf views go to `encode_repeat` (Task 3).

- [ ] **Step 1: Write the failing unit tests:**
  - `zero_copy_off_without_constraints`
  - `zero_copy_off_for_other_gpu`
  - `zero_copy_off_when_modifier_not_offered`
  - `zero_copy_off_without_gpu_conversion`
  - `zero_copy_off_when_env_disabled`
  - `zero_copy_used_when_everything_matches`
  - Engine test `dmabuf_geometry_change_reallocates_or_falls_back`: add a `#[doc(hidden)]` fake backend mode to the synthetic backend that reports `DmabufInvalidated` once. The loop must keep producing packets.
- [ ] **Step 2: Run** `cargo test -p fthrust-engine`. Expected: compile failure.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: pass.
- [ ] **Step 5: Commit** `perf(engine): zero-copy capture through dmabufs on the encoder's GPU`

---

### Task 7: Ring keeps clip + one GOP

**Files:**
- Modify: `crates/fthrust-core/src/capture_config.rs` (`linux_ring_budget_mb`), `crates/fthrust-engine/src/engine.rs` (`make_ring`), `docs/architecture.md` ("The ring" paragraph)
- Test: `crates/fthrust-core` unit tests, `crates/fthrust-media/tests/ring.rs`

**Interfaces:**
- Produces: `pub fn ring_window_ms(buffer_seconds: u32) -> u64 = (buffer_seconds as u64 + 2) * 1000 + 500`, where the GOP is `2 × fps` frames (`ffi/codec.rs:142`). `make_ring` uses it, and `linux_ring_budget_mb` sizes for `buffer_seconds + 3`.

- [ ] **Step 1: Write the failing tests:**
  - `ring_window_is_clip_plus_gop_and_margin`: `ring_window_ms(45) == 47_500`.
  - In `tests/ring.rs`, `clip_length_snapshot_starts_on_a_keyframe_within_the_window`: a ring of `ring_window_ms(45)` at 180 fps, 60 s of packets with a keyframe every 360 frames. A 45 000 ms snapshot starts on a keyframe, spans ≥ 45 s, and has `full_history == false`.
  - Update `linux_ring_budget_mb`'s existing expected values.
- [ ] **Step 2: Run.** Expected: failure.
- [ ] **Step 3: Implement**, and fix the doc paragraph (it currently says "plus 2 seconds" while the code used 5).
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: pass.
- [ ] **Step 5: Commit** `perf(engine): keep only clip length plus one GOP in the ring`

---

### Task 8: Disk payload store

**Files:**
- Create: `crates/fthrust-media/src/payload.rs` (register in `lib.rs`)
- Modify: `crates/fthrust-media/src/ring.rs`, `crates/fthrust-media/src/recorder.rs`, `crates/fthrust-media/src/playback/proxy.rs`, `crates/fthrust-engine/src/save.rs`, and every `EncodedPacket { data: … }` site
- Test: `crates/fthrust-media/tests/payload.rs`, `crates/fthrust-media/tests/ring.rs`

**Interfaces:**
- Produces:
  - `pub enum Payload { Mem(Arc<[u8]>), Disk(DiskRef) }`
    - `DiskRef { segment: Arc<Segment>, offset: u64, len: u32 }`, where `Segment` owns a file and deletes it on `Drop`.
    - `fn len(&self) -> usize` and `fn is_empty(&self) -> bool`.
    - `fn with_bytes<R>(&self, scratch: &mut Vec<u8>, f: impl FnOnce(&[u8]) -> R) -> std::io::Result<R>`, which reads with `pread` for `Disk`.
  - `EncodedPacket.data: Payload`. The encoder produces `Mem`.
  - `pub struct SegmentStore`, created by `SegmentStore::new(dir: PathBuf, segment_bytes: u64) -> io::Result<Self>`.
    - `fn store(&mut self, data: &[u8]) -> io::Result<DiskRef>` appends to the current segment, opened with mode `0o600`, and rotates at `segment_bytes`.
    - On rotation it calls `sync_file_range(SYNC_FILE_RANGE_WRITE)` on the closed segment, and `posix_fadvise(POSIX_FADV_DONTNEED)` on the segment closed before that.
  - `EncodedRing::new_on_disk(max_duration_ms: u64, fps: u32, max_bytes: u64, store: SegmentStore) -> Self`. `push` converts `Mem` payloads to `Disk` through the store. If a store write fails, the ring logs `disk replay buffer unavailable; keeping it in memory` once and keeps `Mem` payloads from then on.
  - Default segment size: 8 MiB.
  - Writers: `packet.data.with_bytes(&mut scratch, |b| mux.write_packet(index, b, …))`. Each writer keeps one `scratch: Vec<u8>` per save or recording.

- [ ] **Step 1: Write the failing tests:**
  - `disk_ring_snapshot_reads_back_the_same_bytes`
  - `trimmed_segments_are_deleted`: once the ring trims past a segment and no snapshot holds it, the file is gone.
  - `snapshot_keeps_segments_alive_after_trim`
  - `segment_files_are_private`: mode `0o600`.
  - `store_failure_falls_back_to_memory`: use a store whose directory is removed after creation; pushes keep working and the snapshot is complete.
  - Update the existing ring and recorder tests to `Payload::Mem(…)`.
- [ ] **Step 2: Run** `cargo test -p fthrust-media`. Expected: compile failure.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: pass.
- [ ] **Step 5: Commit** `feat(media): optional disk-backed payload store for the replay ring`

---

### Task 9: "Replay buffer" setting, end to end

**Files:**
- Create: `crates/fthrust-engine/src/replay_dir.rs` (register in `lib.rs`)
- Modify:
  - `crates/fthrust-core/src/capture_config.rs` (`BufferLocation`, parsed into `CaptureConfig`)
  - `crates/fthrust-engine/src/engine.rs` (`make_ring`), `crates/fthrust-engine/src/main.rs` (stale cleanup at serve start, removal at exit)
  - `crates/fthrust-ui/src/popups/capture_settings.rs` (a combo row "Replay buffer" with options "Memory" and "Disk", placed directly under "Clip length", same `combo_row` helper)
  - `docs/capturing.md` (the "How much memory it uses" section)
- Test: unit tests in `replay_dir.rs` and `capture_config.rs`; engine test; uidrive scenario check

**Interfaces:**
- Produces:
  - `pub enum BufferLocation { Memory, Disk }` with `parse(&str)`; unknown values fail closed to `Memory`.
  - `CaptureConfig.buffer_location: BufferLocation`, with serde default `Memory` so older UIs and engines interoperate.
  - In `replay_dir`:
    - `pub fn replay_root() -> PathBuf`
    - `pub fn create_own() -> io::Result<PathBuf>` creates `<root>/<pid>` with mode `0o700`.
    - `pub fn remove_stale(root: &Path, is_alive: impl Fn(u32) -> bool)`
    - `pub fn remove_own()`
  - Engine:
    - With `Disk`, `make_ring` builds `EncodedRing::new_on_disk` with a `SegmentStore` in `create_own()`. If that errors, it logs the disk-unavailable line and builds a memory ring.
    - A config change of `buffer_location` rebuilds the ring, which clears it, as a bitrate change does today.
    - If `statfs` reports `TMPFS_MAGIC` for the directory, log `info` "replay buffer directory is on tmpfs; disk mode saves no memory".
  - UI help text under the row: "Disk keeps the replay buffer in your cache folder instead of RAM. It uses much less memory but writes to your drive continuously."

- [ ] **Step 1: Write the failing tests:**
  - `buffer_location_parses_and_defaults_to_memory`
  - `stale_replay_dirs_are_removed`: dirs `1` (dead), `2` (alive) and `notapid`; only `1` is removed and `notapid` is left alone.
  - `own_replay_dir_is_private`
  - `unwritable_cache_falls_back_to_memory`, an engine test with `XDG_CACHE_HOME` pointed at a read-only temp dir. Pass the root explicitly through a `make_ring_in(root)` seam rather than mutating the env in a test.
  - `disk_mode_ring_writes_segments`, an engine test with the synthetic backend in disk mode: segment files appear under the given root.
- [ ] **Step 2: Run** `cargo test -p fthrust-core -p fthrust-engine`. Expected: compile failure.
- [ ] **Step 3: Implement**, including the UI row and the docs paragraph. The docs give the size math (bitrate × length) and the SSD write rate (50 Mbps ≈ 22 GB/hour).
- [ ] **Step 4: Run** `cargo test --workspace`. On the bench, open the capture settings popup (`tools/uidrive`), switch "Replay buffer" to Disk, and check that segment files appear under the bench HOME's `.cache/fthrust/replay/<pid>/`. Screenshot the popup and check the row matches its neighbours.
- [ ] **Step 5: Commit** `feat: replay buffer can live on disk (setting, default memory)`

---

### Task 10: AV1 bitrates matched to H.264 quality

**Files:**
- Create: `tools/bitrate-match/run.sh`, `tools/bitrate-match/README.md`
- Modify: `crates/fthrust-core/src/capture_config.rs` (AV1 table, applied at the preset resolution near line 348), `crates/fthrust-ui/src/services/recording.rs:117` and `crates/fthrust-ui/src/shell/toolbar.rs` (`preset_bitrate` / `clip_bitrate` show the AV1 value when `codec_pref == "av1"`)
- Test: `crates/fthrust-core` unit tests; UI toolbar test if one exists for `clip_bitrate`

**Interfaces:**
- Produces: `pub fn codec_preset_kbps(resolution: &str, level: BitrateLevel, codec: Codec) -> Option<u32>`.
  - It equals `bitrate_preset_kbps` for every codec except `Av1`, which uses `AV1_PRESET_KBPS`, a table with the same shape as `bitrate_preset_kbps` that holds the measured values.
  - Custom bitrates are untouched. Every caller of `bitrate_preset_kbps` that knows the codec switches to it.

- [ ] **Step 1: Write the measurement script.** `run.sh <source.mp4>…`:
  - For each source, take a 20 s excerpt at 1920×1080.
  - Encode it with the FFmpeg CLI using the engine's exact encoder settings from `RawEncoder::open`: rate control, GOP `2×fps`, colour tags and preset. Do this with `h264_vaapi` at the three 1080p levels (10/25/50 Mbps), and with `av1_vaapi` on a ladder of 50–90% of each in 5% steps.
  - Score every encode with `libvmaf` against the excerpt and print a table.
  - For each level, pick the lowest AV1 bitrate whose mean VMAF is ≥ H.264's mean VMAF at that level, and whose 5th-percentile VMAF is no more than 1 point below H.264's.
- [ ] **Step 2: Run it** on three of the user's own clips from `~/Videos/clips`: one high-motion, one mid, one mostly static (pick by file bitrate with `ffprobe`). Do the work in the scratchpad, keep their files untouched, and paste the table into the README. Derive the other resolutions by the same per-level percentage.
- [ ] **Step 3: Write the failing tests:**
  - `av1_presets_use_the_matched_table`: assert the measured values.
  - `h264_presets_are_unchanged`
  - `custom_bitrate_ignores_codec`
- [ ] **Step 4: Implement**, then run `cargo test --workspace`.
- [ ] **Step 5: Commit** `feat: AV1 bitrate presets matched to H.264 quality (VMAF)`, with the table in the message.

---

### Task 11: Measure on the real desktop

**Files:**
- Modify: `docs/capturing.md` and `docs/architecture.md` (zero copy, if the numbers change what they say)

- [ ] **Step 1: Build** `cargo build --release -p fthrust-ui -p fthrust-engine`.
- [ ] **Step 2: Launch a scratch copy.**
  - Use a scratch `HOME` holding a copy of `~/.fthr/settings.json`. Set `notification_monitor=HDMI-A-1`, and point `clips_directory`, `recording_directory` and `imported_clip_folders` at scratch.
  - Use a short private runtime dir (`mktemp -d /tmp/fr.XXXX` with a `wayland-1` symlink to the real socket), with `PIPEWIRE_RUNTIME_DIR=/run/user/1000` and `HYPRLAND_INSTANCE_SIGNATURE=`.
  - Add the window rule `hl.window_rule({match = {class = "^(space\\.kalopsia\\.fthrust)$"}, monitor = "HDMI-A-1", no_initial_focus = true})` first, and check placement and focus within 4 s. Stop the copy by PID if it lands wrong.
- [ ] **Step 3: Record**, after 5 minutes and one save via `fthrust --hotkey save_clip`:
  - engine `%CPU`;
  - per-thread CPU from `/proc/<pid>/task/*/stat` over 10 s;
  - `VmRSS` / `RssAnon`;
  - whether the log shows zero copy in use, or its fallback line with the reason.

  Repeat with `FTHR_NO_ZERO_COPY=1`, and once in disk mode, recording RSS and the segment files.
- [ ] **Step 4: Check against targets:**
  - zero copy: engine ≤ 6% CPU;
  - shm path: ≤ 15% CPU;
  - memory mode at baseline settings: ≤ 400 MB RSS;
  - disk mode: ≤ 160 MB RSS.

  Report the numbers rather than tuning beyond this plan.
- [ ] **Step 5: Check the clips:** `ffprobe` tags are `bt709`/`tv`/`left`, and frame 1 compared between the zero-copy and shm clips is visually identical (extract PNGs and view them).
- [ ] **Step 6: Commit** the doc updates: `docs: zero-copy capture and the disk replay buffer`

---

## Not in this plan

- **The wlr screencopy dmabuf path.** Hyprland uses ext-image-copy; screencopy keeps shm, but gets Task 1's repeat flag.
- **Zero copy for NVENC, AMF and QSV.**
- **Changing the default codec.** The user chose to keep H.264.
- **UI process memory.**
