# FTHRust Plan 3: Capture Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `fthrust-engine` daemon: Wayland capture (ext-image-copy-capture, wlr-screencopy, ScreenCast portal over PipeWire), PipeWire audio, recovery, health, replay save and continuous recording, driven over a private Unix-socket IPC, qualified on Hyprland on this machine.

**Architecture:** Four new crates. `fthrust-ipc` holds the message types, framing and tokio client/server. `fthrust-wayland` holds the raw-frame model, normalisation, bounded dispatch, output tracking, shm pools and the two protocol backends. `fthrust-portal` holds the ScreenCast session (ashpd) and its PipeWire video stream. `fthrust-engine` holds the policies, the audio capture, the generation loop, the save/record workers and the binary. Every piece of logic that the reference tests pin is ported with the same values; everything that needs a compositor, a GPU or an audio server is behind a trait with a synthetic implementation so the full daemon runs in CI.

**Tech Stack:** Rust 1.98 stable, edition 2024; `wayland-client` 0.31 (`dlopen`), `wayland-protocols` 0.32 (`client`, `staging`), `wayland-protocols-wlr` 0.3 (`client`); `pipewire` 0.10 (libpipewire-0.3 at runtime, bindgen at build); `ashpd` 0.13 (`tokio`); `tokio` 1 (`rt-multi-thread`, `net`, `io-util`, `sync`, `time`, `macros`, `signal`, `process`); `serde`, `serde_json`; `nix` 0.31 (`poll`, `signal`, `fs`, `fcntl`); `memmap2`; `crossbeam-channel`; `tracing`. Existing: `fthrust-core`, `fthrust-media`.

**Spec:** `docs/superpowers/specs/2026-10-22-fthrust-design.md` (sections 5.4, 5.5 socket path only, 6.1, 6.3, 7, 9 "Engine", 10). Reference material: `docs/reference/reference-engine-inventory.md` (every `L/src/...` pointer resolves under `reference/FTHRcapture_linux/src/`), `docs/reference/crate-research-2026-10-22.md` sections 1, 2, 4, 5.

## Global Constraints

- "Wayland only, no X11." No X11 crate, no x11grab, no XWayland fallback ("explicitly refuses XWayland/x11grab fallback").
- "FFmpeg stays the LGPL shared-library build and is dynamically loaded, never statically linked." Reuse `fthrust-media`; no new FFmpeg linkage.
- "Runtime dependencies allowed on the host: Mesa or vendor VAAPI driver, libva, PipeWire, Wayland, xdg-desktop-portal". Wayland is reached through `wayland-client` with the `dlopen` feature; libpipewire-0.3 is linked dynamically. A missing libpipewire disables audio and the portal backend with a startup warning, never a crash.
- "Config root stays `~/.fthr`." The portal restore token stays at `~/.fthr/portal_screencast_token` with the reference's semantics. The engine socket is `$XDG_RUNTIME_DIR/fthrust/engine.sock` (`fthrust_core::paths::runtime_dir()`), directory mode 0700, socket mode 0600.
- IPC messages are `serde` structs, length-prefixed (u32 little-endian byte count, then JSON), maximum frame 16 MiB; `PROTOCOL_VERSION = 1`.
- Health flags, status strings and save admission messages keep the reference's values exactly: flags ACTIVE 1, RECOVERING 2, BACKEND_FAILED 4, CONTENT_SUSPECT 8, PAUSED 16 (`fthrust_core::capture_health::flags`).
- Recovery: 3 attempts with 250, 750, 1500 ms backoff; transient conditions wait 2000 ms without consuming budget; healthy reset after `fps * 10` frames; encode failure threshold `max(5, fps / 2)`; backoff sleeps in 50 ms slices.
- Every defect in spec section 9 "Engine" gets a test and a `CHANGES.md` line: save-clip blocking the command loop; PDEATHSIG per thread; `shm_open` without exclusivity (the region is gone; the socket is per-user and bound exclusively); cursor missing on wlr and ext; output transforms ignored; no DMA-BUF path; unbounded audio read on shutdown; recovery never tries a different backend; no startup error code.
- `#![forbid(unsafe_code)]` in `fthrust-ipc` and `fthrust-engine`. `fthrust-wayland` and `fthrust-portal` use `#![deny(unsafe_code)]` with `#[allow(unsafe_code)]` only on the modules that touch memfd mappings, PipeWire buffers or DMA-BUF handles, each `unsafe` block with a `SAFETY` comment.
- Gate for every task: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check`. Hardware- or compositor-gated tests print `skipping: ...` and return when their prerequisite is absent; they must pass, not skip, on this machine (Hyprland, AMD VA-API, PipeWire).
- Commit after every task. Branch `plan-3-engine` from `main`.

## Review Focus

1. The compositor goes away mid-generation (Hyprland restarts, the session ends): the backend returns `Disconnected`; recovery treats it as `CompositorUnreachable`, waits 2 s slices while running, and the engine reports `RECOVERING` with a reason, then `BACKEND_FAILED` with that reason once the budget is spent, never a hang or a spin. Test in Task 11.
2. The selected output is unplugged and replugged: with a user-named output the generation fails `OutputAbsent`, waits without consuming budget, and resumes on the same output when it returns; it never silently captures a different monitor. Test in Task 11.
3. A save is requested in the first seconds after start, before any keyframe has been published: the request is refused with the reference's "Capture restarted; the replay buffer is still rebuilding" message and no file is created. Test in Task 12.
4. The UI disconnects mid-save: the save completes, the result is kept for the next client's `GetStatus`, and the engine accepts a new connection without restarting capture. Test in Task 13.
5. No audio server (PipeWire absent or refusing): the engine emits `StartupWarning { code: "DESKTOP_AUDIO_UNAVAILABLE" }`, keeps capturing video, and a save produces a video-only clip whose `SaveResult` carries `audio_included: false` and the audio error text. Test in Task 10 and Task 12.

---

## File structure

```
crates/fthrust-ipc/src/
  lib.rs            re-exports, PROTOCOL_VERSION, MAX_FRAME_BYTES, IpcError, socket_path()
  messages.rs       Command, Event, StatusSnapshot, EngineConfig, BackendKind, SavedClip, ...
  config.rs         EngineConfig::normalized() (engine_args rules, warnings text)
  framing.rs        encode/decode, FrameReader, FrameWriter
  transport.rs      EngineServer, EngineClient (tokio UnixStream)
crates/fthrust-wayland/src/
  lib.rs            #![deny(unsafe_code)], module list
  frame.rs          RawFrame, PixelLayout, OutputTransform, normalize()  (pure)
  dispatch.rs       WaitResult, WaitOps trait, wait_for_event(), dispatch_until(), bounded_roundtrip()
  outputs.rs        OutputInfo, OutputTracker (wl_output v4 name/mode/transform/scale)
  shm.rs            MemfdPool, ShmBuffer (allow(unsafe_code) for mmap)
  screencopy.rs     WlrScreencopy backend (zwlr_screencopy_manager_v1 v3)
  ext_capture.rs    ExtImageCopy backend (ext_image_copy_capture_manager_v1)
  dmabuf.rs         (Task 17) linux-dmabuf feedback, gbm buffers, DRM_PRIME import
crates/fthrust-portal/src/
  lib.rs
  token.rs          restore-token file (pure)
  screencast.rs     ScreenCastSession (ashpd), startup retry policy
  stream.rs         PipeWireVideoStream (pipewire-rs), FrameLayout, copy_rows (pure parts)
crates/fthrust-engine/src/
  lib.rs            #![forbid(unsafe_code)]
  cli.rs            CliMode, classify(), listing JSON
  devices.rs        PipeWire sink/source enumeration (-> cli JSON)
  policy.rs         backend candidates/override/recovery order, CaptureRecoveryPolicy, EncodeFailurePolicy, next_encode_deadline, wait_for_recovery_backoff, should_repeat_last_frame
  watchdog.rs       ContentWatchdog (core sample_bgra_grid + tracker)
  backend.rs        CaptureBackend, BackendFactory, BackendFailure, CaptureError, adapters for wayland/portal, SyntheticBackend
  audio/mod.rs      AudioProvider trait, AudioRing, AudioStatus, SyntheticAudio
  audio/pipewire.rs PipeWireAudio (streams, default-sink follower, reconnect)
  engine.rs         CaptureEngine: generation loop, health, status, pause/resume, reconfigure
  save.rs           SaveWorker (admission, snapshot, audio windows, write_clip), RecordWorker
  daemon.rs         serve(): IPC loop, heartbeat, parent watch, signals, startup codes
  main.rs           binary entry: handle_probe_argv, cli classify, serve
docs/engine-protocol.md   IPC schema and lifecycle (Task 18)
```

---

### Task 1: Crate scaffolds, dependencies, CI packages

**Files:**
- Create: `crates/fthrust-ipc/Cargo.toml`, `crates/fthrust-ipc/src/lib.rs`
- Create: `crates/fthrust-wayland/Cargo.toml`, `crates/fthrust-wayland/src/lib.rs`
- Create: `crates/fthrust-portal/Cargo.toml`, `crates/fthrust-portal/src/lib.rs`
- Create: `crates/fthrust-engine/Cargo.toml`, `crates/fthrust-engine/src/lib.rs`, `crates/fthrust-engine/src/main.rs`
- Modify: `Cargo.toml` (workspace members and `[workspace.dependencies]`), `.github/workflows/ci.yml`, `deny.toml`
- Test: `crates/fthrust-engine/tests/smoke.rs`

**Interfaces:**
- Produces: the four crates with `fthrust_engine::VERSION: &str = env!("CARGO_PKG_VERSION")` and a `fthrust-engine` binary whose `--version` prints `fthrust-engine <VERSION>`.

- [ ] **Step 1: Write the failing test**

```rust
// crates/fthrust-engine/tests/smoke.rs
#[test]
fn binary_prints_version() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_fthrust-engine")).arg("--version").output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), format!("fthrust-engine {}", fthrust_engine::VERSION));
}
```

- [ ] **Step 2: Run it** — `cargo test -p fthrust-engine --test smoke` — Expected: FAIL (crate missing).

- [ ] **Step 3: Add the crates.** Workspace deps: `wayland-client = { version = "0.31", features = ["dlopen"] }`, `wayland-backend = { version = "0.3", features = ["client_system", "dlopen"] }`, `wayland-protocols = { version = "0.32", features = ["client", "staging", "unstable"] }`, `wayland-protocols-wlr = { version = "0.3", features = ["client"] }`, `pipewire = "0.10"`, `libspa = "0.10"`, `ashpd = { version = "0.13", default-features = false, features = ["tokio", "pipewire"] }`, `zbus = { version = "5", default-features = false, features = ["tokio"] }`, `tokio = { version = "1", features = ["rt-multi-thread", "net", "io-util", "sync", "time", "macros", "signal", "process"] }`, `memmap2 = "0.9"`, `crossbeam-channel = "0.5"`, `libc = "0.2"`. Pin versions that resolve today; record them in the ledger. `fthrust-engine` depends on core, media, ipc, wayland, portal. CI apt step adds `libpipewire-0.3-dev libspa-0.2-dev libclang-dev clang libwayland-dev`. `deny.toml`: add licences the new tree needs (expect `MIT OR Apache-2.0` only; anything else is a ruling).

- [ ] **Step 4: Run** `cargo test -p fthrust-engine --test smoke` — Expected: PASS. Run the gate.

- [ ] **Step 5: Commit** `chore(engine): scaffold ipc, wayland, portal and engine crates`.

---

### Task 2: IPC messages, framing and config normalisation

**Files:**
- Create: `crates/fthrust-ipc/src/messages.rs`, `config.rs`, `framing.rs`
- Test: `crates/fthrust-ipc/tests/framing.rs`, `crates/fthrust-ipc/tests/config.rs`

**Interfaces:**
- Consumes: `fthrust_core::capture_config::{CaptureConfig, Codec, EncoderPref, Scaling}`, `fthrust_core::paths::runtime_dir`.
- Produces (all `Serialize + Deserialize + Clone + Debug + PartialEq`):
  - `pub const PROTOCOL_VERSION: u32 = 1; pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;`
  - `pub enum BackendKind { WlrScreencopy, ExtImageCopy, PortalScreenCast, Synthetic }` with `pub fn name(self) -> &'static str` (`"wlr-screencopy"`, `"ext-image-copy-capture"`, `"ScreenCast portal"`, `"synthetic"`) and `pub fn parse_override(value: &str) -> Option<BackendKind>` (`wlr`, `ext`, `portal`, `synthetic`, case-insensitive; `x11` and unknown → `None`).
  - `pub struct EngineConfig { pub capture: CaptureConfig, pub max_buffer_mb: u32, pub backend_override: Option<BackendKind> }` with `pub fn normalized(&self) -> (EngineConfig, Vec<String>)`.
  - `pub enum Command { Configure(EngineConfig), SaveClip { request_id: u64, path: PathBuf, seconds: u32 }, StartRecording { request_id: u64, path: PathBuf }, StopRecording { request_id: u64 }, Pause, Resume, GetStatus, Shutdown }`
  - `pub enum RecordingState { Idle, Recording { path: PathBuf, started_ns: i64 }, Stopping }`
  - `pub enum SourceStatus { Disabled, Connecting, Streaming, Reconnecting { reason: String } }`
  - `pub struct AudioStatus { pub system: SourceStatus, pub microphone: SourceStatus }`
  - `pub struct StatusSnapshot { pub frames_captured: u64, pub bytes_written: u64, pub encoder_open: bool, pub active_codec: String, pub health_flags: u32, pub capture_generation: u32, pub encoder_generation: u32, pub content_sample_sequence: u32, pub content_suspicious_streak: u32, pub content_luma_mean: f32, pub content_luma_variance: f32, pub capture_failure_detail: String, pub recovery_reason: String, pub paused: bool, pub recording: RecordingState, pub backend: Option<BackendKind>, pub selected_output: String, pub native_width: u32, pub native_height: u32, pub audio: AudioStatus, pub last_save: Option<(u64, Result<SavedClip, String>)> }`
  - `pub struct SavedClip { pub path: PathBuf, pub bytes_written: u64, pub audio_included: bool, pub audio_error: Option<String>, pub audio_tracks: Vec<String> }`
  - `pub enum Event { Hello { protocol_version: u32, pid: u32, version: String }, StartupWarning { code: String, detail: String }, StartupError { code: String, detail: String }, Configured { warnings: Vec<String> }, Status(StatusSnapshot), SaveStarted { request_id: u64 }, SaveProgress { request_id: u64, bytes_written: u64 }, SaveResult { request_id: u64, result: Result<SavedClip, String> }, RecordingStarted { request_id: u64, path: PathBuf }, RecordingStopped { request_id: u64, path: PathBuf, bytes: u64 }, RecordingError { request_id: u64, message: String }, Heartbeat }`
  - `framing::{encode<T: Serialize>(&T) -> Result<Vec<u8>, IpcError>, decode<T: DeserializeOwned>(&[u8]) -> Result<T, IpcError>, FrameReader<R: AsyncRead + Unpin>::new(R) / async fn read<T>(&mut self) -> Result<Option<T>, IpcError>, FrameWriter<W: AsyncWrite + Unpin>::new(W) / async fn write<T: Serialize>(&mut self, &T) -> Result<(), IpcError>}`; `IpcError { Io, Json, FrameTooLarge(usize), Truncated, ProtocolMismatch { expected, got } }`.
  - `pub fn socket_path() -> Result<PathBuf, IpcError>` = `runtime_dir()?.join("engine.sock")`.

- [ ] **Step 1: Write the failing tests.** `framing.rs`: `roundtrip_command_and_event` (every `Command` variant and a full `StatusSnapshot` survive encode→decode); `length_prefix_is_u32_le` (`encode(&Command::Pause)` starts with the byte length of the JSON as LE u32); `oversize_frame_rejected` (a reader given a header claiming `MAX_FRAME_BYTES + 1` returns `FrameTooLarge`); `truncated_stream_is_error` (header plus half the body then EOF → `Truncated`); `clean_eof_is_none` (empty reader → `Ok(None)`). `config.rs`, values from `reference/FTHRcapture_linux/tests/linux_engine_args_test.cpp`: `defaults_when_zero_or_missing` (fps 0 → 60, buffer 0 → 30, bitrate 0 → 16000, preset 0 → 4, each with no warning when the input was the type's default); `clamps_with_one_warning_each` (fps 1000 → 360, buffer 901 → 900, bitrate 100 → 500 and 300000 → 200000, preset 9 → 7, max_buffer_mb 10 → 64 and 20000 → 16384, width 8000 height 100 → native 0×0; exactly one warning line per clamped field, each naming the field and the value used); `half_specified_size_is_native`; `parse_override_cases` (`"WLR"` → `WlrScreencopy`, `"portal"`, `"synthetic"`, `"x11"` → None, `""` → None).

- [ ] **Step 2: Run** `cargo test -p fthrust-ipc` — Expected: FAIL to compile.

- [ ] **Step 3: Implement.** Clamp table (from `engine_args.h`/`engine-startup.md`): fps `1..=360`, buffer `1..=900`, bitrate `500..=200000`, preset `1..=7`, `max_buffer_mb` 0 = none else `64..=16384`, size: both zero = native, one zero or either > 7680×4320 = native. Warning text: `"<field> <value> is out of range; using <used>"`.

- [ ] **Step 4: Run** `cargo test -p fthrust-ipc` — Expected: PASS.

- [ ] **Step 5: Commit** `feat(ipc): message types, framing and config normalisation`.

---

### Task 3: IPC transport

**Files:**
- Create: `crates/fthrust-ipc/src/transport.rs`
- Test: `crates/fthrust-ipc/tests/transport.rs`

**Interfaces:**
- Produces: `pub struct EngineServer` with `pub async fn bind(path: &Path) -> Result<EngineServer, IpcError>` (creates the parent dir 0700, removes a stale socket only if connecting to it fails, binds, chmod 0600), `pub async fn accept(&self) -> Result<Connection, IpcError>`; `pub struct Connection { pub commands: tokio::sync::mpsc::Receiver<Command>, pub events: tokio::sync::mpsc::Sender<Event>, pub closed: tokio::sync::watch::Receiver<bool> }` (reader task decodes commands, writer task encodes events; either task ending flips `closed`); `pub struct EngineClient` with `pub async fn connect(path: &Path) -> Result<EngineClient, IpcError>` (reads the `Hello` first and checks `PROTOCOL_VERSION`), `pub async fn send(&self, Command) -> Result<(), IpcError>`, `pub fn events(&mut self) -> &mut tokio::sync::mpsc::Receiver<Event>`.

- [ ] **Step 1: Write the failing tests** (tokio): `client_receives_hello_then_events` (server accepts, sends `Hello`, then `Status`; client sees both); `server_sees_commands_in_order` (Configure, Pause, Shutdown); `protocol_mismatch_rejected` (server sends `Hello { protocol_version: 99 }` → `connect` returns `ProtocolMismatch`); `stale_socket_file_is_replaced` (a dead socket path exists → bind succeeds); `live_socket_is_not_stolen` (a bound server exists → second `bind` fails); `client_drop_flips_closed`; `socket_mode_is_0600`.

- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement** with `tokio::net::{UnixListener, UnixStream}`, `tokio::io::split`, bounded channels of 64.
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Commit** `feat(ipc): unix-socket server and client`.

---

### Task 4: Engine CLI and device listing JSON

**Files:**
- Create: `crates/fthrust-engine/src/cli.rs`, `crates/fthrust-engine/src/devices.rs`
- Test: `crates/fthrust-engine/tests/cli.rs`

**Interfaces:**
- Produces: `cli::{CliMode { Serve { socket: PathBuf }, ListPlaybackDevices, ListMicrophones, Version, UsageError(String) }, classify(args: &[String]) -> CliMode}`; `devices::{Endpoint { endpoint_id: String, display_name: String, is_default: bool }, listing_json(endpoints: &[Endpoint]) -> String, error_json(message: &str) -> String, enumerate_sinks() -> Result<Vec<Endpoint>, String>, enumerate_sources() -> Result<Vec<Endpoint>, String>}` (PipeWire registry: nodes with `media.class` `Audio/Sink` / `Audio/Source`, default from metadata `default.audio.sink` / `default.audio.source`, `display_name` = `node.description` else `node.nick` else `node.name`, 3 s timeout).

- [ ] **Step 1: Write the failing tests.** `classify_rules` from `linux_engine_args_test.cpp`: `--list-playback-devices` alone → `ListPlaybackDevices`; `--list-microphones` alone; any other `--x` anywhere → `UsageError` mentioning it; `--socket P` → `Serve`; `--socket` missing value → `UsageError`; a lone `-` or `-1` is not a flag; no args → `UsageError("--socket is required")`. `json_shape` from `linux_audio_endpoint_json_test.cpp`: empty list → `{"schema_version":1,"microphones":[]}` exactly; populated list keeps order with keys `endpoint_id, display_name, device_state (always 1), is_default`; `display_name` empty falls back to `endpoint_id`; escaping of `"`, `\`, `\n`, `\r`, `\t`, control bytes as `\u00xx`, UTF-8 passes through; output is a single line; `error_json("x")` → `{"schema_version":1,"error":"x"}`. `live_enumeration_lists_default_sink`: skip unless `enumerate_sinks()` succeeds; otherwise exactly one `is_default`.

- [ ] **Step 2: Run** — Expected: FAIL.
- [ ] **Step 3: Implement.** JSON built with `serde_json` (its escaping matches the required forms); `devices.rs` uses `pipewire::main_loop::MainLoop` on the calling thread with a registry listener and a timeout.
- [ ] **Step 4: Run** — Expected: PASS (live test passes here).
- [ ] **Step 5: Commit** `feat(engine): command line and audio endpoint listing`.

---

### Task 5: Backend and recovery policies

**Files:**
- Create: `crates/fthrust-engine/src/policy.rs`
- Test: `crates/fthrust-engine/tests/policy.rs`

**Interfaces:**
- Produces: `backend_candidates(has_wayland: bool) -> Vec<BackendKind>` (`[WlrScreencopy, ExtImageCopy, PortalScreenCast]`; no Wayland → empty); `recovery_order(last_good: BackendKind, candidates: &[BackendKind]) -> Vec<BackendKind>` (last_good first, then the remaining candidates in order, excluding `PortalScreenCast` unless it is `last_good`; the fallthrough is the spec's fix); `pub struct RecoveryDecision { pub retry: bool, pub exhausted: bool, pub attempt: u32, pub backoff: Duration }`; `CaptureRecoveryPolicy::new(healthy_reset_frames: u64, max_attempts: u32)`, `on_generation_failed(&mut self, produced_frames: u64, transient: bool) -> RecoveryDecision`, `attempts()`; `EncodeFailurePolicy::new(fps: u32)`, `on_encode_result(&mut self, ok: bool) -> bool`, `consecutive_failures()`; `next_encode_deadline(next_encode_ns, frame_ns, frame_timestamp_ns) -> i64`; `wait_for_recovery_backoff(remaining: Duration, is_running: impl Fn() -> bool, sleep: impl FnMut(Duration)) -> bool`; `should_repeat_last_frame(result: fthrust_wayland::dispatch::WaitResult, have_last_frame: bool) -> bool`.

- [ ] **Step 1: Write the failing tests** with the values in `capture_recovery_test.cpp` and `capture_backend_candidates_test.cpp`: backoffs 250/750/1500 then exhausted on the 4th; `fps*10` produced frames resets attempts; transient → retry with 2000 ms and attempts unchanged; `EncodeFailurePolicy(60)` trips at 30, `(4)` trips at 5, success resets; `next_encode_deadline` re-anchors only when the frame is later than `next + 2*frame`; `wait_for_recovery_backoff(Duration::from_millis(120), ..)` calls sleep with 50, 50, 20 and returns true; cancellation after the first slice returns false with one sleep; `recovery_order(Wlr, cands)` = `[Wlr, Ext]`, `(Ext, cands)` = `[Ext, Wlr]`, `(Portal, cands)` = `[Portal, Wlr, Ext]`; `should_repeat_last_frame(Timeout, true)` only.

- [ ] **Step 2–5:** RED, implement, GREEN, commit `feat(engine): backend and recovery policies`.

---

### Task 6: Frame model and normalisation (pure)

**Files:**
- Create: `crates/fthrust-wayland/src/frame.rs`
- Test: `crates/fthrust-wayland/tests/frame.rs`

**Interfaces:**
- Produces: `pub enum PixelLayout { Bgra, Bgrx, Rgba, Rgbx, Abgr, Xbgr, Argb, Xrgb, Xrgb2101010, Xbgr2101010 }` with `pub fn from_wl_shm(format: u32) -> Option<PixelLayout>` (wl_shm enum values; preference order `preferred_wl_shm(formats: &[u32]) -> Option<u32>` = XRGB8888, ARGB8888, XBGR8888, ABGR8888, RGBX8888, RGBA8888, BGRX8888, BGRA8888, then the two 10-bit); `pub enum OutputTransform { Normal, Rot90, Rot180, Rot270, Flipped, Flipped90, Flipped180, Flipped270 }` with `from_wl(u32)`; `pub struct RawFrame { pub data: Vec<u8> or borrowed: use pub struct FrameView<'a> { pub data: &'a [u8], pub stride: usize, pub width: u32, pub height: u32, pub layout: PixelLayout, pub y_invert: bool, pub transform: OutputTransform, pub timestamp_ns: i64 } }`; `pub struct BgraFrame { pub data: Vec<u8>, pub stride: usize, pub width: u32, pub height: u32, pub timestamp_ns: i64 }`; `pub fn normalize(view: &FrameView<'_>, out: &mut BgraFrame) -> Result<(), FrameError>` (BGRA/BGRX top-down with `Normal` transform copies rows only when `stride != width*4`, otherwise `out.data` is replaced by a copy of the input slice (a `Cow` is not used; the engine owns `out` across frames); other orders are swizzled; 10-bit is truncated to 8 bits per channel; `y_invert` flips; rotations/flips are applied so `out` is upright, with `out.width/height` swapped for 90/270); `FrameError { ZeroSize, ShortStride, ShortBuffer, UnsupportedFormat }`.

- [ ] **Step 1: Write the failing tests** (values from `shm_frame_test.cpp` plus transforms): mapping table; preference order; `bgra_passthrough_is_exact`; `rgba_is_swizzled` (pixel R,G,B,A → B,G,R,A); `y_invert_flips_rows`; `ten_bit_is_truncated` (X2RGB10 0x3FF red → 0xFF); `rot90_rotates_and_swaps_dims` (2×3 frame → 3×2 with known pixels); `flipped180_equals_vertical_flip`; `rejects_zero_size_short_stride_short_buffer_unknown_format`.

- [ ] **Step 2–5:** RED, implement, GREEN, commit `feat(wayland): frame layouts, normalisation and output transforms`.

---

### Task 7: Bounded dispatch, outputs, shm pools

**Files:**
- Create: `crates/fthrust-wayland/src/dispatch.rs`, `outputs.rs`, `shm.rs`, `connection.rs`
- Test: `crates/fthrust-wayland/tests/dispatch.rs`, `crates/fthrust-wayland/tests/outputs.rs`

**Interfaces:**
- Produces: `dispatch::{WaitResult { EventReceived, Timeout, Cancelled, Disconnected, Error }, trait WaitOps { fn dispatch_pending(&mut self) -> Result<usize, DispatchError>; fn prepare_read(&mut self) -> bool; fn cancel_read(&mut self); fn read_events(&mut self) -> Result<(), DispatchError>; fn flush(&mut self) -> Result<(), DispatchError>; fn poll(&mut self, want_write: bool, timeout: Duration) -> Result<PollEvents, DispatchError>; fn protocol_error(&self) -> Option<String> }, DispatchError { WouldBlock, Disconnected, Other(String) }, PollEvents { readable, writable, hangup }, wait_for_event(ops: &mut dyn WaitOps, deadline: Instant, keep_running: &dyn Fn() -> bool) -> WaitResult, dispatch_until(ops, deadline, keep_running, complete: &mut dyn FnMut() -> bool) -> WaitResult, WaylandOps(Connection, EventQueue<State>) implementing WaitOps}`; `connection::WaylandConnection::connect() -> Result<Self, ConnectError>` (`WAYLAND_DISPLAY` missing → `NoDisplay`), `bounded_roundtrip(&mut self, deadline, keep_running) -> WaitResult`; `outputs::{OutputInfo { name: String, description: String, width: u32, height: u32, refresh_mhz: u32, scale: i32, transform: OutputTransform, done: bool }, OutputTracker::outputs(&self) -> &[OutputInfo], select(&self, wanted: &str) -> Option<&OutputInfo>}` (empty `wanted` = first done output); `shm::{MemfdPool::new(conn, bytes) -> Result, ShmBuffer { fn as_slice(&self) -> &[u8]; fn wl_buffer(&self) -> &WlBuffer; fn stride/width/height/format }, MemfdPool::buffer(&mut self, width, height, stride, format) -> Result<ShmBuffer>}` (`allow(unsafe_code)` for `memmap2`, memfd via `nix::sys::memfd`, sealed `F_SEAL_SHRINK`).

- [ ] **Step 1: Write the failing tests.** `dispatch.rs` with a scripted `FakeOps` (a `VecDeque` of poll outcomes): `event_before_deadline` (readable → `EventReceived`, exactly one `read_events`); `no_event_is_timeout_without_busy_spin` (deadline 120 ms, polls of ≤ 50 ms, at most 4 poll calls); `cancellation_cancels_prepared_read` (keep_running false after prepare → `Cancelled`, `cancel_read` called once, `read_events` never); `hangup_is_disconnected`; `pending_events_dispatched_before_prepare_read` (`dispatch_pending` returns 1 → `EventReceived` with no poll); `flush_would_block_polls_for_writable`; `dispatch_until_completes_on_predicate`. `outputs.rs`: `lists_outputs_with_names` skip unless `WaylandConnection::connect()` succeeds; otherwise at least one output with a non-empty name, `done == true`, `select("")` is the first, `select("nonexistent")` is `None`.

- [ ] **Step 2–5:** RED, implement, GREEN, commit `feat(wayland): bounded dispatch, output tracking, shm pools`.

---

### Task 8: wlr-screencopy backend

**Files:**
- Create: `crates/fthrust-wayland/src/screencopy.rs`
- Test: `crates/fthrust-wayland/tests/screencopy.rs`

**Interfaces:**
- Produces: `pub struct WlrScreencopy`; `pub fn open(conn: WaylandConnection, output: &str, strict_output: bool, cancel: Arc<AtomicBool>) -> Result<WlrScreencopy, BackendError>` (5 s init deadline: registry roundtrip, outputs, select, probe one frame with `overlay_cursor = 1`); `pub fn capture(&mut self) -> Result<FrameView<'_>, CaptureFailure>` (2 s deadline; requests `capture_output(overlay_cursor = 1)`, waits `buffer_done` (v3) or `buffer` + `flags` (v1/v2: the reference could not finish on v2; here a v1/v2 compositor is handled by issuing `copy` after the `buffer` event), reallocates on size/stride/format change, honours `flags.y_invert` and the output's transform, timestamp = `CLOCK_MONOTONIC` now); `pub fn native_size(&self) -> (u32, u32)`, `selected_output(&self) -> &str`; `BackendError { kind: FailureKind { Error, CompositorUnreachable, OutputAbsent }, reason: String }`; `CaptureFailure { Timeout, Cancelled, Disconnected, Failed(String) }`.

- [ ] **Step 1: Write the failing test** `captures_frames_on_a_wlr_compositor`: skip unless connect succeeds and the registry has `zwlr_screencopy_manager_v1`; open on `""`; capture 5 frames; assert width/height equal `native_size`, `layout` maps to a known `PixelLayout`, timestamps strictly increase, `normalize` succeeds and the resulting BGRA frame is not entirely zero. `strict_missing_output_is_output_absent`: `open(conn, "no-such-output", true, ..)` → `OutputAbsent`. `cancel_during_open_is_cancelled`.

- [ ] **Step 2–5:** RED, implement (`wayland_protocols_wlr::screencopy::v1::client`), GREEN on Hyprland, commit `feat(wayland): wlr-screencopy backend with cursor`.

---

### Task 9: ext-image-copy-capture backend

**Files:**
- Create: `crates/fthrust-wayland/src/ext_capture.rs`
- Test: `crates/fthrust-wayland/tests/ext_capture.rs`

**Interfaces:**
- Produces: `pub struct ExtImageCopy` with the same `open/capture/native_size/selected_output` shape as Task 8. Session options `PAINT_CURSORS`; shm format chosen by `preferred_wl_shm`; one pending frame at a time; `buffer_constraints` failure waits 2 s for a new constraints batch and retries once; `stopped` → `CaptureFailure::Failed("session stopped")`; on `WaitResult::Timeout` with a last frame, the last frame is repeated with a fresh timestamp, paced on `last_delivery + 1/fps` (min 1 ms) so a static desktop still yields `fps` frames (`fps` passed to `open`); the `transform` event is honoured by setting `FrameView.transform`.

- [ ] **Step 1: Write the failing tests** `captures_frames_on_ext_compositor` (skip unless `ext_image_copy_capture_manager_v1` is in the registry; 5 frames as in Task 8, and after the screen is static the frame rate over 1 s is within 20 % of `fps = 30`), `strict_missing_output_is_output_absent`, `repeat_policy_is_timeout_only` (pure; delegates to policy in the engine; here asserts the backend exposes `last_frame_available()`).

- [ ] **Step 2–5:** RED, implement, GREEN on Hyprland, commit `feat(wayland): ext-image-copy-capture backend with cursor and transforms`.

---

### Task 10: Portal ScreenCast session and PipeWire video stream

**Files:**
- Create: `crates/fthrust-portal/src/token.rs`, `screencast.rs`, `stream.rs`
- Test: `crates/fthrust-portal/tests/token.rs`, `crates/fthrust-portal/tests/stream_pure.rs`, `crates/fthrust-portal/tests/portal_live.rs`

**Interfaces:**
- Produces: `token::{path(paths: &FthrPaths) -> PathBuf (root/portal_screencast_token), load(path) -> Result<Option<String>, TokenError> (missing → None; whitespace or control chars → TokenError::Damaged), save(path, token: &str) -> io::Result<()> (empty token deletes the file; else tmp + fsync + rename, mode 0600)}`; `screencast::{ScreenCastSession::start(paths: &FthrPaths, cancel: CancellationToken, timeouts: Timeouts { dialog: 120 s, method: 5 s, request: 10 s }) -> Result<ScreenCastSession, PortalFailure>` (ashpd: `CreateSession`, `SelectSources(types = Monitor, multiple = false, cursor_mode = Embedded, persist_mode = ExplicitlyRevoked, restore_token)`, `Start`, `OpenPipeWireRemote`; the returned token is saved; a restored session that fails to negotiate is retried once without the token = `should_retry_startup(attempt: u32, had_token: bool) -> bool`), `fd(&self) -> OwnedFd`, `node_id(&self) -> u32`, `closed(&self) -> watch::Receiver<bool>`; `PortalFailure { Unavailable(String), UserDeclined, Failed(String) }`}; `stream::{FrameLayout { width, height, layout: PixelLayout, stride }, describe_spa_format(...) -> Option<FrameLayout>, copy_rows(src: &[u8], src_stride, layout, dst: &mut BgraFrame) (R/B swap for RGBx/RGBA, drops padding), PipeWireVideoStream::connect(fd, node_id, fps, cancel) -> Result<Self, StreamError>` (formats BGRx/BGRA/RGBx/RGBA, size 1×1..16384, framerate 0..1000, buffers 2..16, `MemFd | MemPtr` (DMA-BUF added in Task 17), format within 10 s, first frame within 5 s), `capture(&mut self) -> Result<FrameView<'_>, CaptureFailure>` (newest queued buffer; header pts used when within 1 s of now else now; timestamps strictly increasing; static screen repeated at 1/fps anchored on last delivery; a resolution change → `CaptureFailure::Failed("stream resolution changed")`, a format-only change swaps the layout), `native_size`}.

- [ ] **Step 1: Write the failing tests.** `token.rs` (values from `portal_screencast_test.cpp`): save/load roundtrip, 0600, empty token deletes, `"abc\n"` is `Damaged`, `" "` is `Damaged`, missing is `None`; `should_retry_startup(0, true)` only. `stream_pure.rs`: `copy_rows_swaps_rgb_and_drops_padding`, `describe_spa_format_rejects_unknown`, `pts_rule` (helper `choose_pts(header_pts: Option<i64>, now_ns) -> i64`: within 1 s → header, else now; and `monotonic(last, candidate) = max(candidate, last + 1)`). `portal_live.rs`: skip unless `FTHRUST_PORTAL_TEST=1`; start a session (the dialog appears), connect the stream, capture 5 frames, assert strictly increasing timestamps and a known layout; a second run reuses the saved token without a dialog.

- [ ] **Step 2–5:** RED, implement, GREEN (pure tests in CI; the live test on this machine with the env var set once by the executor), commit `feat(portal): ScreenCast session with restore token and PipeWire video stream`.

---

### Task 11: Content watchdog, backend adapters, synthetic backend, capture engine core

**Files:**
- Create: `crates/fthrust-engine/src/watchdog.rs`, `backend.rs`, `engine.rs`
- Modify: `crates/fthrust-engine/src/lib.rs`
- Test: `crates/fthrust-engine/tests/engine_core.rs`

**Interfaces:**
- Consumes: `fthrust_media::{encoder::{Encoder, EncoderConfig}, ring::EncodedRing, replay_interval, capabilities::handle_probe_argv}`, `fthrust_core::capture_health::{flags, sample_bgra_grid, ContentSuspicionTracker}`, Task 5 policies, Task 8/9/10 backends.
- Produces:
  - `watchdog::ContentWatchdog::new(fps: u32)`, `observe(&mut self, frame: &BgraFrame) -> Option<ContentMetrics { sequence: u32, streak: u32, luma_mean: f32, luma_variance: f32, suspect: bool }>` (samples once per `fps` frames; rows bounded by `stride`; suspect after 12 consecutive black-like or uniform samples).
  - `backend::{FailureKind { Error, CompositorUnreachable, OutputAbsent, UserDeclined }, BackendFailure { kind, reason: String }, CaptureError { Timeout, Cancelled, Disconnected, Failed(String) }, trait CaptureBackend: Send { fn kind(&self) -> BackendKind; fn capture(&mut self) -> Result<FrameView<'_>, CaptureError>; fn native_size(&self) -> (u32, u32); fn selected_output(&self) -> String; fn last_frame_available(&self) -> bool }, trait BackendFactory: Send + Sync { fn has_wayland(&self) -> bool; fn create(&self, kind: BackendKind, cfg: &CaptureConfig, strict_output: bool, cancel: Arc<AtomicBool>) -> Result<Box<dyn CaptureBackend>, BackendFailure> }, RealFactory, SyntheticFactory { pub script: Arc<Mutex<SyntheticScript>> }}`; `SyntheticScript { pub width: u32, pub height: u32, pub fail_inits: VecDeque<FailureKind>, pub capture_errors_after: Option<(u64, CaptureError)>, pub frame_interval: Duration, pub outputs: Vec<String> }` (frames are moving gradients at `frame_interval`, timestamps from `CLOCK_MONOTONIC`); `SyntheticFactory` is also selected by `BackendKind::Synthetic` in `RealFactory` when env `FTHRUST_SYNTHETIC_BACKEND=1`.
  - `engine::{EngineDeps { backends: Arc<dyn BackendFactory>, audio: Box<dyn AudioProvider>, sleep: Arc<dyn Fn(Duration) + Send + Sync> }, CaptureEngine::new(config: EngineConfig, deps: EngineDeps) -> CaptureEngine, start(&mut self) -> Result<Vec<StartupNotice>, StartupError { code: &'static str, detail: String }>, status(&self) -> StatusSnapshot, events(&self) -> crossbeam_channel::Receiver<EngineNotice>, set_paused(&self, bool), reconfigure(&mut self, EngineConfig) -> Result<Vec<String>, String>, shutdown(&mut self), fn ring(&self) -> Arc<EncodedRing>, fn encoder_snapshot(&self) -> Option<EncoderSnapshot { generation: u64, codec_id, extradata: Vec<u8>, width, height, fps, bitrate_kbps }>}`; `EngineNotice { StatusChanged, Recovering { reason }, Failed { reason }, GenerationStarted { capture_generation, backend } }`.
  - Generation loop per `capture_engine.cpp:155-343`: select backend (override forced, else candidates in order; portal `UserDeclined` → terminal failure with `retry_pointless`); encode size = target or native, `Scaling::Fit` shrinks to fit keeping aspect, both dims masked even; reuse the encoder when geometry unchanged, else reopen (`encoder_generation += 1`, `ring.clear()`); loop: paused → sleep 50 ms with flags `ACTIVE | PAUSED`; capture; `next_encode_deadline` throttle; watchdog; `Encoder::encode` → ring push (and the recording tap, Task 12); `EncodeFailurePolicy` trip → close encoder, end generation. Recovery per Task 5 with `recovery_order` fallthrough; `Disconnected`/connect failures map to `CompositorUnreachable`; strict output on recovery when the user named one.
  - Startup codes (stderr line `FTHR_STARTUP_ERROR: <CODE>: <detail>` and the `StartupError` event): `WAYLAND_UNAVAILABLE` (no `WAYLAND_DISPLAY`), `CAPTURE_BACKEND_UNAVAILABLE` (every candidate failed; detail = joined reasons), `ENCODER_UNAVAILABLE` (`EncoderUnavailable` text), `CAPTURE_DECLINED` (portal declined). Warnings (`FTHR_STARTUP_WARNING: <CODE>: <detail>`): `DESKTOP_AUDIO_UNAVAILABLE`, `MICROPHONE_UNAVAILABLE`.

- [ ] **Step 1: Write the failing tests** (software encoder, `SyntheticFactory`, `SyntheticAudio`, an injected `sleep` that records durations and returns immediately): `healthy_generation_publishes_frames_and_flags` (after 1 s: `frames_captured ≥ 20` at `fps 30`, `health_flags == ACTIVE`, `encoder_open`, `active_codec` non-empty, `capture_generation == 1`); `fit_scaling_masks_even_and_keeps_aspect` (synthetic 1001×601, target 640×360 fit → encoder 598×358? compute: fit 1001×601 into 640×360 = 599.6×360 → 598×360 after even mask; stretch → 640×360); `pause_sets_flag_and_stops_frames`; `init_failures_follow_backoffs_then_fall_through` (script: `fail_inits = [Error, Error, Error, Error]` for Wlr then success on Ext: sleeps recorded `[250, 750, 1500]`, then `GenerationStarted { backend: ExtImageCopy }`, `capture_generation == 1`); `transient_failures_wait_without_budget` (`[CompositorUnreachable × 3]` then success: sleeps `[2000, 2000, 2000]`, attempts stay 0); `output_absent_waits_and_resumes_same_output` (strict output "DP-9": absent twice then present → `selected_output == "DP-9"`); `disconnect_mid_generation_recovers` (`capture_errors_after: (10, Disconnected)` → `Recovering` notice then a new generation); `exhausted_budget_is_failed_with_reason` (every candidate fails 4× → `health_flags & BACKEND_FAILED`, `capture_failure_detail` non-empty, status still served); `encode_failures_reopen_encoder` (inject via `EncoderConfig` bitrate 0? no: make the synthetic script deliver `stride < width*4` frames for 20 frames → `Encoder::encode` errors → `EncodeFailurePolicy` trips → `encoder_generation == 2`); `content_watchdog_flags_black_screen` (script black frames; after 13 s of synthetic time at 30 fps: `CONTENT_SUSPECT`); `startup_without_wayland_is_coded` (`RealFactory` with `WAYLAND_DISPLAY` unset in a child env → `StartupError { code: "WAYLAND_UNAVAILABLE" }`).

- [ ] **Step 2–5:** RED, implement, GREEN, commit `feat(engine): capture engine core with recovery, health and synthetic backend`.

---

### Task 12: Multi-track clip audio (media), audio capture, save and record workers

**Files:**
- Modify: `crates/fthrust-media/src/clip_writer.rs`, `crates/fthrust-media/src/ffi/mux.rs` (stream `title`/`handler_name` tags), `crates/fthrust-media/tests/clip_writer.rs`
- Create: `crates/fthrust-engine/src/audio/mod.rs`, `audio/pipewire.rs`, `save.rs`
- Test: `crates/fthrust-engine/tests/audio.rs`, `crates/fthrust-engine/tests/save_record.rs`

**Interfaces:**
- Media change: `ClipAudio` gains `pub title: &'a str`; `write_clip(final_path, &ClipVideo, tracks: &[ClipAudio<'_>], progress)` writes one AAC stream per non-empty track in order with stream tags `title` and `handler_name` = the title; `ClipWriteResult.audio_tracks: Vec<String>` lists the titles written; a track whose encoder fails is skipped and named in `audio_error` while the others are written. Titles: `"Default Mix"` for the system track, `"Microphone"` for the microphone.
- `audio::{AudioSourceKind { System, Microphone }, AudioTarget { DefaultSinkMonitor, SinkMonitor(String), Source(String) }, AudioConfig { system: Option<AudioTarget>, microphone: Option<AudioTarget>, keep_seconds: u32 }, AudioRing::new(capacity_samples) / push(AudioBlock) / window(end_ns, duration_ms) -> Vec<f32> (via fthrust_media::audio_timing::assemble_window) / prune, trait AudioProvider: Send { fn start(&mut self, cfg: &AudioConfig) -> Vec<(String, String)> (warning code, detail); fn window(&self, kind: AudioSourceKind, end_ns: i64, duration_ms: u32) -> Option<Vec<f32>>; fn status(&self) -> AudioStatus; fn stop(&mut self) }, SyntheticAudio (440 Hz tone, blocks every 10 ms from a thread, obeys `keep_seconds`), pipewire::PipeWireAudio}`: one `pw_stream` per source on a `pw_thread_loop`, props `media.type Audio`, `media.category Capture`, `media.role Music`, `stream.capture.sink = true` + `target.object = <sink name>` for a sink monitor (empty = default), `target.object = <source>` for a mic; format F32LE 48000 Hz 2 ch (mono mics upmixed by PipeWire); per `process` callback: `pw_stream_get_time_n` → `chunk_start_ns = now - delay_ns - queued_ns - frames/rate` (`fthrust_media::audio_timing::chunk_start_ns`), pushed as `AudioBlock`; default sink follower: metadata `default.audio.sink` change → reconnect the system stream; stream error/disconnect → `Reconnecting { reason }` and a 1 s retry loop; `stop()` disconnects and joins the loop within 2 s (bounded, the spec's fix).
- `save::{SaveRequest { request_id: u64, path: PathBuf, seconds: u32 }, admission(status: &StatusSnapshot, snapshot_present: bool, ring_has_current_generation: bool) -> Result<(), &'static str>` with the reference's exact messages (`capture_engine.cpp:435-460`); `SaveWorker::spawn(engine_handles..., events: Sender<Event>)`, `submit(&self, SaveRequest)` (one at a time; a second request while busy is answered `SaveResult(Err("A clip save is already in progress"))`); the worker: `save_end_ns = now`, `ring.wait_until_published(save_end_ns, max(100, 3000/fps) ms)`, snapshot `(seconds * 1000, save_end_ns, encoder_generation)`, audio windows for both sources ending at `presentation_end_ns` with duration `ceil((presentation_end_ns - presentation_start_ns)/1e6)`, `write_clip` with the tracks, `SaveStarted` first, `SaveProgress` per 1 MiB, then `SaveResult`; `RecordWorker` consumes `EncodedPacket`s and audio blocks from channels fed by the capture thread and `AudioProvider` taps, drives `fthrust_media::recorder::Recorder`, answers `RecordingStarted/Stopped/Error`; a packet backlog over 8 s or a write error ends the recording with `RecordingError`.

- [ ] **Step 1: Write the failing tests.** Media: `two_tracks_are_written_with_titles` (3 streams; `descriptors_from_file` titles `["Default Mix", "Microphone"]`), `one_failing_track_is_skipped_and_named`. `audio.rs`: `ring_window_pads_and_trims` (pure, values from Task 3 of plan 2), `synthetic_audio_delivers_tone` (1 s window has RMS ≈ 0.212 for amplitude 0.3), `live_system_capture_streams` (skip unless `PipeWireAudio::start` reports no warning; `status().system == Streaming` within 3 s; a 500 ms window has 48000 samples; `stop()` returns within 2 s). `save_record.rs` (engine with synthetic backend + synthetic audio): `save_before_first_keyframe_is_refused` (request at t=0 → `SaveResult(Err("Capture restarted; the replay buffer is still rebuilding"))`, no file); `save_writes_requested_seconds_with_two_tracks` (after 6 s capture, `seconds = 3` → MP4 whose probe duration is 3 ± 1/fps, 3 streams, `audio_tracks == ["Default Mix", "Microphone"]`, `SaveStarted` precedes `SaveResult`); `commands_keep_flowing_during_save` (a 25 s ring at 20 Mbps synthetic: while the save runs, `status()` returns within 50 ms and `frames_captured` keeps increasing); `save_while_paused_or_failed_is_refused` (messages verbatim); `video_only_when_audio_unavailable` (`AudioProvider` that reports the warning → `audio_included == false`, `audio_error` set, `StartupWarning { code: "DESKTOP_AUDIO_UNAVAILABLE" }`); `record_start_stop_produces_fragmented_mp4` (2 s recording → file plays, duration ≥ 1.5 s, `RecordingStopped.bytes == file size`); `record_refuses_existing_path`.

- [ ] **Step 2–5:** RED, implement, GREEN (the live audio test passes on this machine), commit `feat(engine): PipeWire audio capture, save and record workers; multi-track clip audio`.

---

### Task 13: The daemon binary

**Files:**
- Create: `crates/fthrust-engine/src/daemon.rs`
- Modify: `crates/fthrust-engine/src/main.rs`
- Test: `crates/fthrust-engine/tests/daemon.rs`

**Interfaces:**
- Produces: `daemon::serve(socket: PathBuf, deps: EngineDeps) -> Result<ExitCode, DaemonError>`: binds (`EngineServer`), prints `FTHRUST_ENGINE_READY <socket>` on stdout, accepts clients one at a time, sends `Hello`, replays `StartupWarning/Error` for a client that connects after startup, pushes `Status` on every `StatusChanged` notice and at least every 500 ms (`Heartbeat` when nothing changed), handles every `Command` (a `Configure` before the first one starts the engine; a later one is `reconfigure`), keeps `last_save` in the snapshot across client reconnects; exits on `Shutdown` (code 0), on stdin EOF (parent gone; the reference's PDEATHSIG replacement; code 0), on SIGTERM/SIGINT (code 0), on a `StartupError` (code 1 after sending the event and printing the `FTHR_STARTUP_ERROR:` line). `main.rs`: `fthrust_media::capabilities::handle_probe_argv()` first, then `cli::classify`, then `serve` or the listings (`--list-*` print JSON to stdout, exit 0; errors print `error_json` to stderr, exit 1), `--version`.

- [ ] **Step 1: Write the failing tests** (spawn `CARGO_BIN_EXE_fthrust-engine` with `FTHRUST_SYNTHETIC_BACKEND=1`, `FTHR_CAPTURE_BACKEND=synthetic`, a temp `XDG_RUNTIME_DIR`, stdin piped): `end_to_end_configure_save_record_shutdown` (Hello → Configure(software H.264, fps 30, buffer 10) → `Configured` → a `Status` with `ACTIVE` within 5 s → after 4 s `SaveClip(seconds 2)` → `SaveStarted`, `SaveResult(Ok)` with a probe-able file → `StartRecording`, 1 s, `StopRecording` → `RecordingStopped` → `Shutdown` → exit 0 within 3 s and the socket file is gone); `client_reconnect_keeps_last_save` (disconnect right after `SaveClip`; reconnect; `GetStatus` → `last_save` is `Some((id, Ok(..)))`, `capture_generation` unchanged); `stdin_eof_exits` (close stdin → exit 0 within 2 s); `sigterm_exits`; `startup_error_is_coded` (`FTHR_CAPTURE_BACKEND=wlr` with `WAYLAND_DISPLAY` removed → `StartupError { code: "WAYLAND_UNAVAILABLE" }`, stderr contains `FTHR_STARTUP_ERROR: WAYLAND_UNAVAILABLE:`, exit 1); `heartbeat_cadence` (10 events within 6 s with no commands).

- [ ] **Step 2–5:** RED, implement (tokio runtime for IPC; engine threads behind `std::thread`; notices bridged with `tokio::sync::mpsc` via `spawn_blocking`), GREEN, commit `feat(engine): fthrust-engine daemon`.

---

### Task 14: Real backend adapters and the Hyprland qualification test

**Files:**
- Modify: `crates/fthrust-engine/src/backend.rs` (`RealFactory` wiring for the three backends, `has_wayland`, override)
- Test: `crates/fthrust-engine/tests/live_capture.rs`

- [ ] **Step 1: Write the failing test** `live_wayland_capture_saves_a_real_clip`: skip unless `WAYLAND_DISPLAY` is set; run `CaptureEngine` with `RealFactory` and `PipeWireAudio`, `EncoderPref::Auto` (VA-API expected here, software allowed when `FTHRUST_LIVE_SOFTWARE=1`), `fps 60`, `buffer 10`; after 5 s: `health_flags == ACTIVE`, `frames_captured ≥ 200`, `backend` is `ExtImageCopy` or `WlrScreencopy`, `native_size` matches the first output; save 3 s → probe duration 3 ± 1/60, width/height even and ≤ native; `active_codec` contains `vaapi` unless software was forced. `override_env_selects_backend` (`FTHR_CAPTURE_BACKEND=wlr` → `WlrScreencopy`).

- [ ] **Step 2–5:** RED, wire, GREEN on this machine, commit `feat(engine): real backend factory, Hyprland live qualification`.

---

### Task 15: DMA-BUF capture and VA-API import (hardware-gated)

**Files:**
- Create: `crates/fthrust-wayland/src/dmabuf.rs`; modify `screencopy.rs`, `ext_capture.rs` (dmabuf frame path when `FTHRUST_DMABUF != 0`), `crates/fthrust-media/src/ffi/hw.rs` (`VaapiFrames::import_drm_prime(&DmaBufPlanes) -> Result<OwnedFrame>` via `av_hwframe_map`), `crates/fthrust-media/src/encoder.rs` (`encode_dmabuf(&mut self, planes: &DmaBufPlanes, wall_time_ns, out)`), `crates/fthrust-engine/src/engine.rs` (prefer the dmabuf path when the backend offers it and the encoder is VA-API).
- Test: `crates/fthrust-wayland/tests/dmabuf.rs`, `crates/fthrust-media/tests/dmabuf_import.rs`

**Interfaces:**
- Produces: `dmabuf::{DmaBufPlanes { width, height, fourcc: u32, modifier: u64, planes: Vec<Plane { fd: OwnedFd, offset: u32, stride: u32 }> }, Feedback (zwp_linux_dmabuf_v1 v4 default feedback: main device + format/modifier table), GbmAllocator::new(render_node) / allocate(width, height, fourcc, modifiers) -> Result<GbmBuffer>}` (`gbm` crate, `drm-fourcc`); screencopy uses `linux_dmabuf` event + `copy` into a gbm-backed `wl_buffer`; ext uses `dmabuf_device`/`dmabuf_format` events.
- Env: `FTHRUST_DMABUF=0` disables; the shm path stays the fallback on any import error (logged once per generation).

- [ ] **Step 1: Write the failing tests** `allocates_and_exports_dmabuf` (skip unless a render node opens with gbm; allocate 256×256 XRGB8888 linear → one plane, stride ≥ 1024), `screencopy_dmabuf_frames_on_hyprland` (skip unless compositor advertises `zwp_linux_dmabuf_v1` v4; capture 3 dmabuf frames), `vaapi_imports_dmabuf_and_encodes` (skip unless VA-API; import the gbm buffer and encode 10 frames → packets).

- [ ] **Step 2–5:** RED, implement, GREEN on this machine (AMD). **If the path cannot be qualified on the executor's machine after a bounded effort (one working day), ledger a `Ruling:` deferring it to a follow-up plan with the shm path as default; the env flag and tests stay.** Commit `feat(engine): DMA-BUF capture with VA-API import`.

---

### Task 16: Final gate, CHANGES, protocol doc, roadmap

**Files:**
- Create: `docs/engine-protocol.md` (every `Command`/`Event` with an example JSON frame, the lifecycle, exit codes, startup codes, env vars `FTHR_CAPTURE_BACKEND`, `FTHR_VAAPI_DEVICE`, `FTHRUST_SYNTHETIC_BACKEND`, `FTHRUST_DMABUF`, `FTHRUST_PORTAL_TEST`, `FTHRUST_LIVE_SOFTWARE`).
- Modify: `CHANGES.md` (one line per spec-9 engine defect and per fix made along the way), `docs/superpowers/plans/2026-10-22-fthrust-roadmap.md` (row 3 shipped), `README.md` (how to run the engine standalone for debugging).

- [ ] Gate: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace && cargo deny check`; the live tests (Tasks 4, 7, 8, 9, 12, 14) pass on this machine and skip in CI; push the branch and confirm CI green.
- [ ] Commit `chore(engine): protocol docs, CHANGES, roadmap`.

---

## Self-review notes

- Spec coverage: 6.1 (two processes, IPC commands/events, handshake, heartbeat) → Tasks 2, 3, 13. 6.3 backends and recovery → 5, 8, 9, 10, 11, 14; DMA-BUF → 15; frame normalisation/transforms → 6; encoded ring and save → 12 (ring from plan 2); audio → 12; manual recording → 12; device listing JSON → 4. 5.5 socket hotkeys are UI-side (plan 5). Section 9 engine defects → tests in 8 (cursor), 6/9 (transforms), 12 (save off the command loop, bounded audio stop), 13 (parent watch instead of PDEATHSIG, startup codes), 5/11 (recovery fallthrough), 15 (DMA-BUF), 3 (exclusive socket replaces shm_open).
- Type consistency: `FrameView`/`BgraFrame`/`PixelLayout`/`OutputTransform` defined in Task 6 and consumed by 8, 9, 10, 11; `WaitResult` in 7 consumed by 5's `should_repeat_last_frame`; `BackendKind` in 2 consumed everywhere; `CaptureError`/`FailureKind` in 11 mirror the backend crates' `CaptureFailure`/`BackendError` through adapters in `backend.rs`.
- Review Focus 1–5 are pinned in Tasks 11, 11, 12, 13, 12.
- Proportion: the plan is shorter than the engine it describes; bodies appear only for values the tests pin.
