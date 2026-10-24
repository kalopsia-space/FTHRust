# Plan 8: Engine CPU and memory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut the capture engine's CPU and memory use at high frame rates by moving colour conversion to the GPU, skipping a per-frame copy, sharing packet buffers instead of copying them, and stopping glibc from hoarding freed memory.

**Architecture:** On VA-API, each captured BGRA frame is uploaded to a BGRX surface and converted to NV12 (and scaled) by a `scale_vaapi` filter graph on the GPU; the encoder is opened on that graph's output frame pool. The CPU swscale path stays as the fallback for every other encoder and for any VA-API driver whose video processor refuses. Encoded packets carry `Arc<[u8]>` payloads so ring snapshots and the recording tap share bytes. The engine caps malloc arenas and trims after saves.

**Tech Stack:** Rust 1.98.1, ffmpeg-next 9 (FFmpeg 8.1: libavfilter `buffer` / `scale_vaapi` / `buffersink`, `av_hwframe_*`), libc `mallopt` / `malloc_trim`.

**Spec:** this conversation's measurements, recorded here because there is no separate design doc:

- Baseline, user's desktop, 2026-10-24: AMD RX 7900 XTX (radeonsi, VA-API with `VAEntrypointVideoProc`), Hyprland, DP-3, settings `framerate=180`, `resolution=source` (1920×1080), `bitrate_level=high` (50,000 kbps), `clip_length=45`, encoder `h264_vaapi`.
- Engine: **~72% of one core, all in one thread; ~990 MB RSS** (825 MB anonymous; `[heap]` ~444 MB plus ten ~64 MB malloc arena regions). App: ~0.3% CPU hidden, ~350 MB (out of scope).
- `ffmpeg … testsrc2 1920x1080 bgra → scale → nv12 -threads 1`: **~3.0 ms CPU per frame** (bilinear and fast_bilinear alike), i.e. ~54% of a core at 180 fps.
- The ring's byte budget is reached: `replay ring byte budget reached … budget_mib=451 kept_seconds=51`.
- Per frame today: shm → `normalize` row copy into `BgraFrame` (8 MB) → swscale BGRA→NV12 (`encoder.rs:324`) → `av_hwframe_transfer_data` upload → `h264_vaapi`.
- `EncodedRing::snapshot` clones every packet's `Vec<u8>` (a save duplicates up to the whole budget), and the recording tap clones each packet (`engine.rs` `tap.try_send(p.clone())`).

## Global Constraints

- Output must stay **BT.709, limited (TV) range, NV12, left chroma siting**, exactly as `RawEncoder::open` tags it today (`ffi/codec.rs:113-117`). A GPU-converted frame must decode to the same colours as the swscale path within the tolerance in Task 1.
- **No silent behaviour change on failure:** if GPU conversion can't be set up or its first frame fails, the encoder uses the swscale path and logs once at `info`: `GPU colour conversion unavailable; converting on the CPU` with the reason.
- `FTHR_CPU_COLOR_CONVERT=1` forces the swscale path (for bug reports).
- No new crate dependencies except `libc` (already `[workspace.dependencies]`) in `fthrust-engine` and `fthrust-media`.
- Upload isolation, `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace` and `cargo deny check` stay green.
- Tests that need VA-API skip with `eprintln!("skipping: …")` when `vaapi_available()` is false, like `tests/encoder.rs::vaapi_encodes_when_device_present`.
- Out of scope: NVENC/AMF/QSV RGB input, zero-copy dmabuf capture (Plan 8b), the app process.

## Review Focus

1. **Source size changes mid-capture** (monitor mode switch, output transform rotation): today `encode()` rebuilds the scaler; the GPU path must rebuild its graph and keep the output size fixed, or fall back. Test in Task 2.
2. **A VA-API driver without BGRX surfaces or `scale_vaapi`** (old Intel i965, some VMs): open must fall back to swscale, not fail the encoder. Test in Task 2 by forcing a converter error.
3. **Odd source sizes** (e.g. 1366×768, 2560×1080 ultrawide scaled to 1080p): the graph scales; NV12 needs even dimensions on the output, which `enc_width/enc_height` already are. Test in Task 1 with a 1366×768 source to 1280×720.
4. **Packets shared across the ring, the tap and an in-flight save**: dropping the ring's copy (trim) while a save still writes must not invalidate the save's bytes. `Arc` guarantees it; Task 4 tests that a snapshot survives a full ring trim.
5. **Encoder probe at startup (`encoder_works`)**: it must exercise the same conversion path the engine will use, so a broken VPP driver is caught as "falls back", not as "encoder unavailable". Task 2 asserts `encoder_works("h264_vaapi")` still returns true when the converter is forced to fail.

---

## File Structure

| File | Responsibility |
|---|---|
| Create `crates/fthrust-media/src/ffi/vpp.rs` | `VaapiConverter`: BGRX surface pool, `buffer → scale_vaapi → buffersink` graph, upload + convert one frame. |
| Modify `crates/fthrust-media/src/ffi/hw.rs` | `VaapiFrames` gains a pool-format parameter and a constructor from an existing frames ref. |
| Modify `crates/fthrust-media/src/ffi/codec.rs` | `RawEncoder::open` takes the hardware frames to use as `hw_frames_ctx`. `packet_bytes` returns `Arc<[u8]>`. |
| Modify `crates/fthrust-media/src/encoder.rs` | Chooses GPU or CPU conversion at open; `encode` and a new `encode_view` route frames. |
| Modify `crates/fthrust-media/src/ring.rs` | `EncodedPacket.data: Arc<[u8]>`. |
| Modify `crates/fthrust-engine/src/engine.rs` | Encode straight from the capture view when no normalisation is needed; tap sends without copying. |
| Modify `crates/fthrust-engine/src/watchdog.rs` | `observe_raw` over a borrowed buffer. |
| Create `crates/fthrust-engine/src/alloc.rs` | `configure()` and `trim()` around `mallopt` / `malloc_trim`. |
| Modify `crates/fthrust-engine/src/main.rs`, `save.rs` | Call `alloc::configure()` at start, `alloc::trim()` after a save or recording ends. |
| Create `crates/fthrust-media/tests/encoder_throughput.rs` | `#[ignore]` measurement: CPU ms per 1080p frame for each path. |

---

### Task 1: `VaapiConverter`, BGRA to NV12 on the GPU

**Files:**
- Create: `crates/fthrust-media/src/ffi/vpp.rs` (register in `ffi/mod.rs`)
- Modify: `crates/fthrust-media/src/ffi/hw.rs`
- Test: `crates/fthrust-media/tests/vpp.rs`, helper in `tests/support/mod.rs`

**Interfaces:**
- Produces:
  - `VaapiFrames::create_on_with(path: &str, width: u32, height: u32, sw_format: ffi::AVPixelFormat) -> Result<VaapiFrames>`; the existing `create_on` calls it with `AV_PIX_FMT_NV12`.
  - `VaapiFrames::device_ref(&self) -> *mut ffi::AVBufferRef` (borrowed, not a new ref).
  - `pub(crate) struct VaapiConverter` with:
    - `VaapiConverter::new(device: &VaapiFrames, src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Result<Self>`
    - `fn convert(&mut self, bgra: &[u8], stride: usize, out: &mut OwnedFrame) -> Result<()>`: `out` receives a VA-API NV12 frame of `dst_w×dst_h`.
    - `fn output_frames_ref(&self) -> *mut ffi::AVBufferRef`: a **new** ref to the buffersink's `hw_frames_ctx`, for the encoder.
    - `fn source_size(&self) -> (u32, u32)`
  - Re-export for tests: `fthrust_media::vaapi::gpu_convert_rgb(src_w, src_h, dst_w, dst_h, bgra: &[u8]) -> Result<Vec<u8>>`: converts one frame and downloads the NV12 result to packed RGB24 (via `av_hwframe_transfer_data` then swscale with BT.709 limited range), for colour checks only. Mark `#[doc(hidden)]`.

- [ ] **Step 1: Write the failing tests** in `tests/vpp.rs`, each starting with the `vaapi_available()` skip:

```rust
#[test]
fn gpu_conversion_matches_bt709_limited_range() {
    // 64×64 solid frames; expected RGB after an NV12 round trip, ±6 per channel.
    for (bgra, rgb) in [
        ([0, 0, 0, 255], [0, 0, 0]),
        ([255, 255, 255, 255], [255, 255, 255]),
        ([0, 0, 255, 255], [255, 0, 0]),      // red
        ([0x20, 0xe3, 0xa0, 255], [0xa0, 0xe3, 0x20]),
    ] {
        let frame = solid(64, 64, bgra);
        let out = gpu_convert_rgb(64, 64, 64, 64, &frame).unwrap();
        assert_rgb_near(&out[..3], rgb, 6);
    }
}

#[test]
fn gpu_conversion_scales_odd_source_to_output_size() {
    let frame = solid(1366, 768, [0, 0, 255, 255]);
    let out = gpu_convert_rgb(1366, 768, 1280, 720, &frame).unwrap();
    assert_eq!(out.len(), 1280 * 720 * 3);
    assert_rgb_near(&out[(360 * 1280 + 640) * 3..][..3], [255, 0, 0], 6);
}

#[test]
fn gpu_conversion_honours_stride_padding() {
    // stride = width*4 + 256, padding bytes filled with 0xFF must not leak.
    let out = gpu_convert_rgb_strided(64, 64, 64 * 4 + 256, [0, 0, 0, 255]).unwrap();
    assert_rgb_near(&out[(63 * 64 + 63) * 3..][..3], [0, 0, 0], 6);
}
```

`solid`, `assert_rgb_near` go in `tests/support/mod.rs`; `gpu_convert_rgb_strided` is a thin test-side wrapper that builds the padded buffer and calls the re-export with the larger stride (give the re-export a `stride` parameter if simpler, and update the three tests to match).

- [ ] **Step 2: Run** `cargo test -p fthrust-media --test vpp`. Expected: compile failure (`gpu_convert_rgb` not found).

- [ ] **Step 3: Implement.**
  - BGRX pool: `create_on_with(path, src_w, src_h, AV_PIX_FMT_BGR0)`. Upload by copying rows from `bgra` (respecting `stride`) into a reusable software `BGR0` `OwnedFrame`, then `av_hwframe_get_buffer` + `av_hwframe_transfer_data`.
  - Graph, built once in `new`: `buffer` with `pix_fmt=vaapi`, `video_size=src`, `time_base=1/1`, and `av_buffersrc_parameters_set` carrying the BGRX pool's `hw_frames_ctx`; then `scale_vaapi=w=<dst_w>:h=<dst_h>:format=nv12:out_color_matrix=bt709:out_range=tv`; then `buffersink`. Use `ffmpeg_next::filter::Graph` the way `export/run.rs:255-330` does; drop to `ffi` only for the buffersrc parameters and `av_buffersink_get_hw_frames_ctx`.
  - `convert`: upload, `av_buffersrc_add_frame_flags(..., AV_BUFFERSRC_FLAG_KEEP_REF)`, `av_buffersink_get_frame` into `out`. Leave pts and keyframe flags to the caller, which sets them on `out` before encoding.

- [ ] **Step 4: Run** `cargo test -p fthrust-media --test vpp`. Expected: 3 passed (on this machine; skips elsewhere).

- [ ] **Step 5: Commit** `feat(media): convert BGRA to NV12 on the GPU with scale_vaapi`

---

### Task 2: The encoder uses the GPU converter on VA-API

**Files:**
- Modify: `crates/fthrust-media/src/ffi/codec.rs`, `crates/fthrust-media/src/encoder.rs`
- Test: `crates/fthrust-media/tests/encoder.rs`
- Create: `crates/fthrust-media/tests/encoder_throughput.rs`

**Interfaces:**
- Consumes: Task 1's `VaapiConverter`, `VaapiFrames::create_on_with`.
- Produces:
  - `RawEncoder::open(name, params, hw: Option<HwInput>)` where `pub(crate) enum HwInput { Pool(VaapiFrames), Converter(VaapiFrames, VaapiConverter) }`. The context's `hw_device_ctx` comes from the `VaapiFrames`; `hw_frames_ctx` is the pool for `Pool` and `converter.output_frames_ref()` for `Converter`.
  - `Encoder::conversion(&self) -> Conversion` with `pub enum Conversion { Gpu, Cpu }` (re-exported from `fthrust_media::encoder`).
  - Test hook: `Encoder::open_forcing_cpu_conversion(cfg) -> Result<Encoder>` (`#[doc(hidden)]`), and the env override `FTHR_CPU_COLOR_CONVERT=1` checked in `open_inner`.

- [ ] **Step 1: Write the failing tests** in `tests/encoder.rs` (VA-API skip as in `vaapi_encodes_when_device_present`):

```rust
#[test]
fn vaapi_encoder_converts_on_the_gpu() {
    let enc = open_vaapi(1920, 1080, 1920, 1080);   // helper: EncoderPref::Vaapi, Codec::H264
    assert_eq!(enc.conversion(), Conversion::Gpu);
}

#[test]
fn gpu_and_cpu_paths_decode_to_the_same_colours() {
    // Encode 30 frames of the support::synthetic_bgra pattern at 320×180 with each path,
    // decode frame 15 of each (support::decode_frame_rgb(extradata, packets, 15)),
    // and compare: mean absolute difference per channel <= 4.
}

#[test]
fn gpu_path_keeps_output_size_when_source_changes() {
    // Open 1920×1080 → 1920×1080; encode 10 frames at 1920×1080, then 10 at 1280×720.
    // All encodes succeed, conversion stays Gpu, and decoded frames are 1920×1080.
}

#[test]
fn falls_back_to_cpu_when_forced() {
    let enc = Encoder::open_forcing_cpu_conversion(&vaapi_cfg(320, 180)).unwrap();
    assert_eq!(enc.conversion(), Conversion::Cpu);
    assert!(Encoder::encoder_works("h264_vaapi"));
}
```

`support::decode_frame_rgb` decodes H.264 packets with `ffmpeg_next`'s decoder (extradata set on the context) and returns packed RGB24 plus its size.

- [ ] **Step 2: Run** `cargo test -p fthrust-media --test encoder`. Expected: compile failure (`Conversion` not found).

- [ ] **Step 3: Implement.**
  - In `try_open`, for `_vaapi` names and unless `FTHR_CPU_COLOR_CONVERT=1` or forced: per device, build `VaapiFrames::create_on(device, …)` for the device, then `VaapiConverter::new(&frames, src_w, src_h, enc_w, enc_h)`, run **one** `convert` of a black frame as a probe, then `RawEncoder::open(name, &params, Some(HwInput::Converter(..)))`. If any of the converter steps fail, log the constraint's `info` line once and open with `HwInput::Pool` as today.
  - `encode()`: on `Conversion::Gpu`, if the source size differs from the converter's, rebuild the converter at the new source size (same output size); if that fails, switch this encoder to `Cpu` (build the `Scaler` and NV12 upload pool then) and log once. Then `convert` into `hw_frame`, set pts/forced keyframe on it, `send_frame`. The CPU path is unchanged.
  - `encoder_works` keeps calling `try_open` + `finish_open`, so it exercises whichever path the engine will get.

- [ ] **Step 4: Write the measurement** `tests/encoder_throughput.rs`, `#[ignore]`: for `Gpu` and `Cpu` (forced), encode 600 frames of 1920×1080 `synthetic_bgra` and print `"{path}: {cpu_ms_per_frame:.2} ms CPU/frame"` using `libc::getrusage(RUSAGE_THREAD)` user+sys before and after. Asserts only that both ran.

- [ ] **Step 5: Run** `cargo test -p fthrust-media --test encoder` (expect all pass) and `cargo test -p fthrust-media --test encoder_throughput -- --ignored --nocapture`. Expected: the GPU line well under 1.0 ms and the CPU line about 3 ms. Paste both lines into the commit message.

- [ ] **Step 6: Commit** `feat(media): VA-API encoder converts colour on the GPU, CPU fallback`

---

### Task 3: Encode straight from the capture buffer

**Files:**
- Modify: `crates/fthrust-wayland/src/frame.rs`, `crates/fthrust-engine/src/engine.rs:880-912`, `crates/fthrust-engine/src/watchdog.rs`
- Test: `crates/fthrust-engine/tests/engine_core.rs` (or the closest existing engine test of the encode loop), `crates/fthrust-engine/src/watchdog.rs` unit tests

**Interfaces:**
- Consumes: `Encoder::encode(bgra, stride, width, height, ts, out)` (unchanged signature).
- Produces:
  - `fthrust_wayland::frame::FrameView::is_encodable_as_is(&self) -> bool`: `layout.is_bgra_bytes() && !y_invert && transform == Normal`.
  - `Watchdog::observe_raw(&mut self, data: &[u8], width: usize, height: usize, stride: usize) -> Option<ContentMetrics>`; `observe(&BgraFrame)` becomes a wrapper.

- [ ] **Step 1: Write the failing tests.**
  - `frame.rs` unit test `encodable_as_is_only_for_upright_bgra_bytes`: true for `Bgrx`/`Bgra` Normal not inverted; false for `Rgbx`, for `y_invert`, for `Rot90`.
  - Watchdog unit test `observe_raw_matches_observe`: same metrics for a frame passed both ways.
  - Engine test `synthetic_capture_encodes_without_normalising`: with the synthetic backend (which delivers BGRX, upright), run 30 frames and assert packets were produced and `Engine::normalized_frames()` is 0. Add `normalized_frames: AtomicU64` to the engine's shared state, incremented on each `normalize` call, and `pub fn normalized_frames(&self) -> u64` on `Engine` (not added to `StatusSnapshot`).

- [ ] **Step 2: Run** the three tests; expect failures (missing items).

- [ ] **Step 3: Implement.** Add `is_encodable_as_is` to `crates/fthrust-wayland/src/frame.rs`. In the loop, when `view.is_encodable_as_is()`, call `watchdog.observe_raw(view.data, …)` and `enc.encode(view.data, view.stride, view.width, view.height, ts, &mut packets)`; otherwise keep `normalize` + the existing calls. `encode` already accepts any stride ≥ `width*4`.

- [ ] **Step 4: Run** `cargo test -p fthrust-engine` and `cargo test -p fthrust-wayland`. Expected: all pass.

- [ ] **Step 5: Commit** `perf(engine): encode upright BGRX frames straight from the capture buffer`

---

### Task 4: Shared packet buffers

**Files:**
- Modify: `crates/fthrust-media/src/ring.rs`, `crates/fthrust-media/src/ffi/codec.rs` (`packet_bytes`, `RawPacket.data`), `crates/fthrust-media/src/encoder.rs:373`, `crates/fthrust-engine/src/engine.rs` (tap), every `EncodedPacket { data: … }` construction in tests (`tests/ring.rs:7-25` and the others `rg 'EncodedPacket \{'` finds)
- Test: `crates/fthrust-media/tests/ring.rs`

**Interfaces:**
- Produces: `EncodedPacket.data: std::sync::Arc<[u8]>`; `RawPacket.data: Arc<[u8]>`; `packet_bytes(p) -> Arc<[u8]>` (copies the payload once, `Arc::from(&[u8])`; empty packets yield `Arc::from([])`).

- [ ] **Step 1: Write the failing tests** in `tests/ring.rs`:

```rust
#[test]
fn snapshot_shares_payloads_with_the_ring() {
    let ring = EncodedRing::new(10_000, 30, 0);
    let p = sized(0, 0, true, 1_000);
    let data = p.data.clone();
    ring.push(p);
    let snap = ring.snapshot(1_000, 0, 0).unwrap();
    assert!(Arc::ptr_eq(&snap.packets[0].data, &data));
}

#[test]
fn snapshot_survives_the_ring_trimming_its_packets() {
    let ring = EncodedRing::new(1_000, 30, 0);
    for i in 0..30 { ring.push(sized(i, i * 33_333_333, i % 10 == 0, 100)); }
    let snap = ring.snapshot(1_000, 0, 0).unwrap();
    for i in 30..120 { ring.push(sized(i, i * 33_333_333, i % 10 == 0, 100)); }
    assert!(snap.packets.iter().all(|p| p.data.len() == 100));
}
```

- [ ] **Step 2: Run** `cargo test -p fthrust-media --test ring`. Expected: type errors (`Vec<u8>` vs `Arc<[u8]>`).

- [ ] **Step 3: Implement** the type change through every construction and use site. Writers take `&p.data` / `&*p.data` unchanged. The engine's tap keeps `p.clone()`, which is now a refcount bump.

- [ ] **Step 4: Run** `cargo test --workspace`. Expected: all pass.

- [ ] **Step 5: Commit** `perf(media): share encoded packet payloads between the ring, saves and recording`

---

### Task 5: Engine allocator settings

**Files:**
- Create: `crates/fthrust-engine/src/alloc.rs` (register in `lib.rs`); add `libc.workspace = true` to `crates/fthrust-engine/Cargo.toml`
- Modify: `crates/fthrust-engine/src/main.rs` (`CliMode::Serve` arm, before `init_logging`), `crates/fthrust-engine/src/save.rs` (after a save's `SaveResult` is sent and after a recording stops)
- Test: `crates/fthrust-engine/tests/alloc.rs`

**Interfaces:**
- Produces: `pub fn configure()`, calling `mallopt(M_ARENA_MAX, 2)` (glibc only, `#[cfg(target_env = "gnu")]`, no-op elsewhere), and `pub fn trim()`, calling `malloc_trim(0)`. Both are safe wrappers and ignore return values except for a `debug!` log.

- [ ] **Step 1: Write the failing test** `trim_returns_freed_memory`: allocate and drop 200 × 1 MiB `Vec<u8>` (touching every page) on 4 threads, call `alloc::trim()`, and assert this process's `RssAnon` from `/proc/self/status` is at most 64 MiB above the value read before the allocations. Also `configure_is_idempotent`: call `configure()` twice.

- [ ] **Step 2: Run** `cargo test -p fthrust-engine --test alloc`. Expected: compile failure.

- [ ] **Step 3: Implement**, then wire `configure()` into the serve path and `trim()` into the two save/recording completion points.

- [ ] **Step 4: Run** `cargo test -p fthrust-engine`. Expected: all pass.

- [ ] **Step 5: Commit** `perf(engine): cap malloc arenas and trim after saves`

---

### Task 6: Measure on the real desktop

**Files:**
- Modify: `docs/capturing.md` (the "How much memory it uses" paragraph, only if numbers change what it says), `docs/architecture.md` ("The engine" → Encoding paragraph: conversion now on the GPU for VA-API)

- [ ] **Step 1: Build** `cargo build --release -p fthrust-ui -p fthrust-engine`.

- [ ] **Step 2: Run alongside the user's own copy without touching it.** Scratch `HOME` with a copy of `~/.fthr/settings.json` (keep `capture_monitor=DP-3`, `framerate=180`, `bitrate_level=high`, `clip_length=45`), and a private runtime dir so sockets don't collide:

```sh
H=$(mktemp -d); mkdir -p $H/.fthr; cp ~/.fthr/settings.json $H/.fthr/
R=$(mktemp -d); chmod 700 $R
HOME=$H XDG_RUNTIME_DIR=$R WAYLAND_DISPLAY=$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY \
  PIPEWIRE_RUNTIME_DIR=$XDG_RUNTIME_DIR HYPRLAND_INSTANCE_SIGNATURE= \
  target/release/fthrust &
```

Open the window on HDMI-A-1 without focus (the user games on DP-3). Clearing `HYPRLAND_INSTANCE_SIGNATURE` stops the managed hotkeys reload from touching the user's config.

- [ ] **Step 3: Record** after 5 minutes of capture and one saved clip: engine `%CPU` from `top -b -n 2 -d 5`, and `VmRSS` and `RssAnon` from `/proc/<engine>/status`. Repeat once with `FTHR_CPU_COLOR_CONVERT=1` for a same-session comparison.

- [ ] **Step 4: Check against targets:** engine ≤ 25% CPU and ≤ 600 MB RSS at the baseline settings; the comparison run shows the CPU path near the 72% baseline. If a target is missed, report the numbers rather than tuning further in this plan.

- [ ] **Step 5: Save a clip in each mode and check it plays** with correct colours (`ffprobe` shows `bt709`, `tv` range; open both in mpv side by side).

- [ ] **Step 6: Update the two doc paragraphs if needed and commit** `docs: engine converts colour on the GPU`

---

## Not in this plan

- **Plan 8b, zero-copy capture:** `zwp_linux_dmabuf_v1` buffers for screencopy / ext-image-copy, imported into VA-API as DRM PRIME surfaces, so frames never touch the CPU. Needs format and modifier negotiation and a fallback to shm.
- NVENC, AMF and Quick Sync taking BGRA input directly.
- App-process memory (~350 MB, mostly wgpu and fonts).
