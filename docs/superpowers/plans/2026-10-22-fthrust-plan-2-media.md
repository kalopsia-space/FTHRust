# FTHRust Plan 2: `fthrust-media` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `fthrust-media` crate: the FFmpeg-backed encode, ring, save and probe layer that the capture engine (plan 3) and the UI (plans 5 and 6) call, proven with synthetic frames and the C++ engine's own test cases.

**Architecture:** One crate with pure modules first (replay interval selection, audio window assembly, encoder candidate policy, encoded ring, transactional save) and FFmpeg-backed modules on top (encoder with VAAPI/NVENC/software candidates, plain-MP4 clip writer with negative pre-roll, fragmented-MP4 recorder, probe and metadata, thumbnail decode, encoder capability cache). `ffmpeg-next` 9 is the binding; hardware frame contexts go through its raw `ffi` module behind a small safe wrapper. Every FFmpeg-touching test skips cleanly when the encoder it needs is absent, exactly as the reference tests do.

**Tech Stack:** Rust 1.98, `ffmpeg-next` 9 (system FFmpeg 9 on this machine, BtbN LGPL 8.x in the AppImage), `image` + `fast_image_resize` for thumbnails, `serde_json`, `nix` (`renameat2`, `fsync`), `tracing`. Depends on `fthrust-core` for paths, settings, audio manifests and the capture config.

**Spec:** `docs/superpowers/specs/2026-10-22-fthrust-design.md` sections 5.4, 6.3, 7, 9. Reference detail: `docs/reference/reference-engine-inventory.md` sections 4, 5, 6.1, 8 (tests 1, 6, 7, 8, 9, 11, 15, 16, 20, 22), 9; `docs/reference/reference-ui-inventory.md` sections 4.7, 4.10, and the `test_media_metadata` / `test_combined_audio_policy` entries in section 7. C++ sources under `reference/FTHRcapture_linux/src`, `reference/FTHRcapture_linux/tests`, `reference/FTHRcapture_common`; Python under `reference/FTHR_UI/core/{media_metadata,combined_audio_policy,encoder_capabilities,ffmpeg_tools}.py`.

## Global Constraints

- FFmpeg is used through `ffmpeg-next`; `unsafe` is allowed only inside `crates/fthrust-media/src/ffi/` (hardware frame contexts and anything the safe wrapper lacks) and every `unsafe` block carries a `// SAFETY:` comment. All other modules keep `#![forbid(unsafe_code)]` at the module level via a crate-level `#![deny(unsafe_code)]` plus `#[allow(unsafe_code)]` on the `ffi` module only.
- Video pixel input to the encoder is BGRA with a stride ≥ width×4; output colour is BT.709, studio (MPEG) range, chroma siting left; GOP = 2×fps with a forced IDR; `AV_CODEC_FLAG_GLOBAL_HEADER` set; PTS derive from the wall clock as `round((wall - epoch) * fps / 1e9)` forced to be strictly increasing.
- Encoder candidate order and names are the reference's (section 4 of the engine inventory): Auto = NVENC, VAAPI/AMF, QSV in that order, filtered by codec; software only when asked, list `libopenh264, libx264, libkvazaar, libx265, libsvtav1, libaom-av1, librav1e` (x264/x265 are additions for distro builds; the AppImage FFmpeg lacks them and the candidate loop skips absent encoders).
- Saved clips are plain MP4 (not fragmented) with `avoid_negative_ts` disabled so pre-roll packets carry negative pts and the stream starts at 0; metadata `comment=fthr-audio-mode=combined|separated`, `fthr_frame_rate=<fps>/1`, `fthr_video_bitrate_bps=<bps>`; audio is native `aac` 128 kbps from float32 stereo 48 kHz; audio setup failure degrades to video-only and is reported in the result, never silently.
- Transactional save: write `<final>.partial`, fsync file, rename with no-replace, fsync directory (failure tolerated); refuse when the final or the partial exists; `renameat2(RENAME_NOREPLACE)` with a `link + unlink` fallback on `EINVAL`/`ENOSYS`.
- Timing modules are ported before anything that depends on them, with the C++ tests' cases as Rust tests.
- Every public item has a doc comment; `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `cargo test --workspace`, `cargo deny check` pass at the end of every task.
- Tests that need a real encoder or decoder call a shared `skip_unless_encoder("libx264" | "libopenh264")` helper and return early with an `eprintln!` when none is available.

## Review Focus

1. A capture gap longer than the ring (engine paused for minutes, then a save): `TakeSnapshot` must return a partial-history snapshot starting at the first keyframe, never an empty or wrongly timed one. Test in Task 4.
2. Audio arriving later than video after a reconnect: the assembled window must lead with silence and keep later audio at capture time, never shift it. Test in Task 3.
3. A save requested while the previous `.partial` still exists (crash during an earlier save): refused with a typed failure, never overwritten. Test in Task 5.
4. A source frame whose size changes mid-session (game resolution change): the encoder keeps its output size and rescales; stride smaller than width×4 is rejected. Test in Task 6.
5. A clip whose container reports an absurd bitrate or no duration: the probe must fall back the way the Python does (never invent a bitrate from audio) rather than returning garbage to the library. Test in Task 9.

---

### Task 1: Crate bootstrap and reference fixtures

**Files:**
- Create: `crates/fthrust-media/Cargo.toml`, `crates/fthrust-media/src/lib.rs`, `crates/fthrust-media/src/ffi/mod.rs`, `crates/fthrust-media/tests/support/mod.rs`
- Modify: `Cargo.toml` (workspace deps: `ffmpeg-next = "9"`, `image = "0.25"`, `fast_image_resize = "6"`), `tools/gen-reference-fixtures.py`, `.github/workflows/ci.yml` (apt install `libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev libavfilter-dev libavdevice-dev pkg-config`)
- Fixtures: `crates/fthrust-media/tests/fixtures/media-metadata.json`, `combined-audio-policy.json`, `encoder-capabilities.json`

**Interfaces:**
- Produces: crate `fthrust-media` depending on `fthrust-core`; `pub fn init() -> Result<()>` calling `ffmpeg_next::init` once; `pub enum MediaError { Ffmpeg(ffmpeg_next::Error), Io(std::io::Error), Core(fthrust_core::CoreError), Invalid(String), EncoderUnavailable(String) }`, `pub type Result<T>`; test helper `support::skip_unless_encoder(names: &[&str]) -> Option<&'static str>` and `support::synthetic_bgra(width, height, frame_index) -> Vec<u8>` (moving gradient so consecutive frames differ).
- Fixtures: `media-metadata.json` = cases `[{ffprobe: <json the reference test feeds>, configured: {...}|null, expected: {duration, width, height, fps, video_bitrate, total_bitrate, vfr}}]` produced by running `core.media_metadata`'s parser on the exact inputs of `reference/tests/test_media_metadata.py`; `combined-audio-policy.json` = cases `[{manifest|null, labels, expected_filter, expected_inputs}]` from `reference/tests/test_combined_audio_policy.py`; `encoder-capabilities.json` = the backend table (`key, label, codecs, codec_names, supports_presets`) and the Linux relabel rule from `core/encoder_capabilities.py`.

- [ ] Step 1: Add the crate and workspace deps; `cargo build -p fthrust-media` succeeds with an empty lib.
- [ ] Step 2: Extend the generator with the three fixtures (each case's expected value computed by the Python module, inputs copied from the Python tests); run it twice; `git diff --exit-code` on fixtures.
- [ ] Step 3: Write `support` helpers with a test `synthetic_frames_differ`.
- [ ] Step 4: Commit `feat(media): crate bootstrap and reference fixtures`.

---

### Task 2: Replay interval selection

**Files:** `crates/fthrust-media/src/replay_interval.rs`; test `tests/replay_interval.rs`. Reference `reference/FTHRcapture_common/replay_interval.h` and `reference/FTHRcapture_common/tests/replay_interval_test.cpp` (port every case).

**Interfaces:**
- `pub struct Sample { pub wall_time_ns: i64, pub keyframe: bool }`
- `pub struct Selection { pub start_index: usize, pub end_index: usize /*inclusive*/, pub full_history: bool, pub requested_start_ns: i64, pub presentation_start_ns: i64, pub presentation_end_ns: i64 }`
- `pub fn select(samples: &[Sample], duration_ms: u32, target_end_ns: i64) -> Option<Selection>` (None when no sample ≤ target or no keyframe at/before the chosen end).
- `pub fn presentation_start_pts(newest_pts: i64, duration_ms: u32, fps: u32, full_history: bool, decode_start_pts: i64) -> i64` = `newest_pts + 1 - duration_ms*fps/1000` when full history else `decode_start_pts`.

- [ ] Tests: `exact_window_normal_rate`, `decode_starts_at_keyframe_before_window`, `insufficient_history_is_partial_from_first_keyframe`, `irregular_timestamps_and_nonzero_epoch`, `ring_and_save_boundaries`, `rapid_saves_do_not_mutate_input`, `invalid_inputs_are_none`, `presentation_offset_math` (values from the C++ test).
- [ ] RED → implement → GREEN → commit `feat(media): replay interval selection`.

---

### Task 3: Audio timing and window assembly

**Files:** `src/audio_timing.rs`; test `tests/audio_timing.rs`. Reference `reference/FTHRcapture_linux/src/audio_timing.{h,cpp}`, `tests/audio_timing_test.cpp`.

**Interfaces:**
- `pub const SAMPLE_RATE: u32 = 48_000; pub const CHANNELS: usize = 2; pub const FRAMES_PER_CHUNK: usize = 480; pub const GAP_FILL_THRESHOLD_NS: i64 = 50_000_000;`
- `pub fn chunk_start_ns(delivery_end_ns: i64, latency_us: i64, frames: usize, rate: u32) -> i64` (negative latency → 0).
- `pub struct AudioBlock { pub start_ns: i64, pub samples: Vec<f32> /*interleaved*/ }`
- `pub fn assemble_window(blocks: &[AudioBlock], end_ns: i64, duration_ms: u32, rate: u32, channels: usize) -> Vec<f32>` (exactly `duration_ms` worth, zero-padded; empty when nothing overlaps; silence inserted for gaps > threshold; straddling blocks trimmed on frame boundaries).
- `pub fn ring_capacity_samples(keep_seconds: u32, rate: u32, channels: usize) -> usize` = `(keep+10)*rate*channels`.

- [ ] Tests: `chunk_start_subtracts_latency_and_length`, `negative_latency_is_zero`, `contiguous_jittered_blocks_join`, `gap_over_50ms_gets_silence_and_later_audio_stays_at_capture_time`, `late_audio_start_pads_leading_silence`, `straddling_block_trimmed_keeps_interleave`, `no_overlap_is_empty`, `ring_capacity_formula`.
- [ ] RED → implement → GREEN → commit `feat(media): audio timing and window assembly`.

---

### Task 4: Encoded ring buffer

**Files:** `src/ring.rs`; test `tests/ring.rs`. Reference `ring_buffer.{h,cpp}`, `tests/linux_encoded_ring_duration_test.cpp`, `capture_engine.cpp:26-51` (`MakeRing`).

**Interfaces:**
- `#[derive(Clone)] pub struct EncodedPacket { pub data: Vec<u8>, pub pts: i64, pub dts: i64, pub keyframe: bool, pub wall_time_ns: i64, pub generation: u64 }`
- `pub struct EncodedRing` with `pub fn new(max_duration_ms: u64, fps: u32, max_bytes: u64 /*0 = time-bounded only*/) -> Self`, `pub fn push(&self, packet: EncodedPacket)` (mutex inside; time trim from the front while span > max_duration; byte budget trims whole GOPs from the front while a newer keyframe remains; counts `budget_trims`), `pub fn snapshot(&self, duration_ms: u32, target_end_ns: i64, generation: u64 /*0 = all*/) -> Option<Snapshot>`, `pub fn wait_until_published(&self, target_end_ns: i64, timeout: Duration) -> bool`, `pub fn clear(&self)`, `pub fn stats(&self) -> RingStats { packets, bytes, span_ms, budget_trims, latest_wall_time_ns }`.
- `pub struct Snapshot { pub packets: Vec<EncodedPacket>, pub full_history: bool, pub presentation_start_pts: i64, pub presentation_start_ns: i64, pub presentation_end_ns: i64, pub media_end_ns: i64, pub fps: u32 }`.

- [ ] Tests (from test 6): `full_window_uses_keyframe_before_window`, `variable_timing_and_delayed_publication`, `clear_drops_old_generation`, `snapshot_filters_by_generation`, `byte_budget_drops_whole_gops_from_front`, `never_drops_current_gop`, `no_budget_and_clear_reset_counters`, plus Review Focus 1 `long_gap_then_save_is_partial_history`.
- [ ] RED → implement → GREEN → commit `feat(media): encoded ring buffer`.

---

### Task 5: Transactional save

**Files:** `src/transactional_save.rs`; test `tests/transactional_save.rs`. Reference `reference/FTHRcapture_common/transactional_save.h`, `reference/FTHRcapture_linux/tests/transactional_save_test.cpp` (16 cases incl. sync ordering).

**Interfaces:**
- `pub enum SaveFailure { DestinationExists, PartialExists, Writer(String), Sync(String), Rename(String), Cleanup { original: Box<SaveFailure>, cleanup: String } }` with `Display` text matching `DescribeFailure`.
- `pub trait SaveFs` (file ops injectable for tests: `write_file`, `fsync_file`, `rename_noreplace`, `fsync_dir`, `remove`, `exists`) with `pub struct RealFs`.
- `pub fn run(final_path: &Path, writer: impl FnOnce(&Path) -> std::result::Result<(), String>) -> std::result::Result<PathBuf, SaveFailure>` and `pub fn run_with(fs: &dyn SaveFs, ...)`.
- `RealFs::rename_noreplace` uses `renameat2(RENAME_NOREPLACE)` and falls back to `link` + `unlink` on `EINVAL`/`ENOSYS`.

- [ ] Tests: `success_path`, `writer_failure_cleans_temp`, `rename_failure_is_error`, `cleanup_failure_preserves_original_error`, `existing_destination_untouched`, `existing_partial_refused` (Review Focus 3), `missing_temporary_cannot_commit`, `file_fsync_before_rename_dir_fsync_after` (recording fake fs), `sync_failure_reported_as_writer_failure`, `noreplace_fallback_on_einval` (fake fs returning EINVAL once).
- [ ] RED → implement → GREEN → commit `feat(media): transactional save with no-replace fallback`.

---

### Task 6: Encoder

**Files:** `src/encoder_policy.rs`, `src/ffi/hw.rs`, `src/encoder.rs`; tests `tests/encoder_policy.rs`, `tests/encoder.rs`. Reference `encoder_policy.{h,cpp}`, `encoder.{h,cpp}`, tests 7, 8, 9, 11.

**Interfaces:**
- `pub enum EncoderPref { Auto, Nvenc, Vaapi, Qsv, Software }` (map from `fthrust_core::capture_config::EncoderPref`), `pub enum CodecPref { Auto, H264, Hevc, Av1 }`.
- `pub fn encoder_candidates(codec: CodecPref, encoder: EncoderPref) -> Vec<&'static str>`.
- `pub struct EncoderConfig { pub width: u32, pub height: u32, pub fps: u32, pub bitrate_kbps: u32, pub preset: u8, pub codec: CodecPref, pub encoder: EncoderPref }`.
- `pub struct Encoder` with `pub fn open(cfg: &EncoderConfig) -> Result<Encoder>` (tries candidates in order; `active_codec()` name), `pub fn encode_bgra(&mut self, bgra: &[u8], stride: usize, width: u32, height: u32, wall_time_ns: i64, generation: u64, out: &mut Vec<EncodedPacket>) -> Result<()>` (rescale on size change; stride check; wall-clock PTS; forced IDR every 2×fps), `pub fn flush(&mut self, out: &mut Vec<EncodedPacket>) -> Result<()>`, `pub fn extradata(&self) -> &[u8]`, `pub fn codec_id(&self) -> ffmpeg_next::codec::Id`, `pub fn apply_preset(ctx, name, preset)` behaviour per the reference table (VAAPI: nothing).
- `ffi::hw`: `pub struct VaapiFrames { .. }` wrapping `av_hwdevice_ctx_create(VAAPI)`, `av_hwframe_ctx_alloc` (NV12, pool 8), `av_hwframe_get_buffer`, `av_hwframe_transfer_data`; `pub fn vaapi_available() -> bool` (device create succeeds on `/dev/dri/renderD*`).

- [ ] Tests: policy `auto_is_hardware_only_in_backend_order`, `codec_filtering`, `software_pref_lists_only_software`, `unknown_prefs_never_add_software`; encoder `keeps_output_size_when_source_changes` (larger, smaller, back), `rejects_short_stride` (Review Focus 4), `first_pts_zero_strictly_increasing_keyframe_every_gop`, `capture_gap_visible_in_media_time`, `vaapi_preset_leaves_compression_level_unset` and `vaapi_encodes_when_device_present` (skip without device).
- [ ] RED → implement → GREEN → commit `feat(media): FFmpeg encoder with hardware candidates`.

---

### Task 7: Clip writer (plain MP4 with pre-roll)

**Files:** `src/clip_writer.rs`; test `tests/clip_writer.rs`. Reference `save_clip.cpp`, test 15 (`short_save_mp4_integration_test.cpp`).

**Interfaces:**
- `pub struct ClipAudio { pub samples: Vec<f32> /*interleaved stereo 48k*/, pub separate_mode: bool }`
- `pub struct ClipWriteResult { pub bytes_written: u64, pub audio_included: bool, pub audio_error: Option<String> }`
- `pub fn write_clip(final_path: &Path, snapshot: &Snapshot, codec_id: ffmpeg_next::codec::Id, extradata: &[u8], bitrate_bps: u64, audio: Option<&ClipAudio>) -> std::result::Result<ClipWriteResult, SaveFailure>` using `transactional_save::run`.

- [ ] Tests: `short_save_actual_mp4` (encode 10 s synthetic at 30 fps, snapshot last 5 s: full history, starts on keyframe, `presentation_start_pts == 7*fps`-style value for the chosen window, pre-roll negative pts, probe start_time 0 and duration 5 s ± one frame), `audio_failure_degrades_to_video_only_and_is_reported`, `metadata_tags_present`.
- [ ] RED → implement → GREEN → commit `feat(media): plain MP4 clip writer with negative pre-roll`.

---

### Task 8: Fragmented recorder

**Files:** `src/recorder.rs`; test `tests/recorder.rs`.

**Interfaces:**
- `pub struct Recorder` with `pub fn start(path: &Path, codec_id, extradata, fps, bitrate_bps, with_audio: bool) -> Result<Recorder>` (fMP4 `movflags=frag_keyframe+empty_moov+default_base_moof+write_colr`), `pub fn write_video(&mut self, packet: &EncodedPacket) -> Result<()>`, `pub fn write_audio(&mut self, samples: &[f32]) -> Result<()>` (AAC), `pub fn finish(self) -> Result<u64>`; dropping without `finish` leaves a playable file up to the last fragment.

- [ ] Tests: `fragments_survive_abrupt_drop` (write 3 s, drop, probe duration ≥ 2 s), `finish_writes_trailer_and_duration`.
- [ ] RED → implement → GREEN → commit `feat(media): fragmented MP4 recorder`.

---

### Task 9: Probe and media metadata

**Files:** `src/probe.rs`; test `tests/probe.rs`. Reference `reference/FTHR_UI/core/media_metadata.py`, `core/ffmpeg_tools.py` (probe JSON fields read), fixture `media-metadata.json`.

**Interfaces:**
- `pub struct MediaInfo { pub duration: f64, pub width: u32, pub height: u32, pub fps: f64, pub vfr: bool, pub video_bitrate: u64, pub total_bitrate: u64, pub audio_streams: Vec<AudioStreamInfo { index, channels, sample_rate, codec }>, pub start_time: f64 }`
- `pub fn probe(path: &Path) -> Result<MediaInfo>` (libavformat; `avg_frame_rate` vs `r_frame_rate` rule, FTHR metadata overrides `fthr_frame_rate`/`fthr_video_bitrate_bps`, bitrate fallbacks never derived from audio).
- `pub fn parse_probe_json(json: &serde_json::Value, configured: Option<&serde_json::Value>) -> MediaInfo` (the pure rule set, tested against the fixture).

- [ ] Tests: `fixture_cases_match`, `probe_real_file_matches_writer` (file from Task 7), `absurd_bitrate_and_missing_duration_fall_back` (Review Focus 5).
- [ ] RED → implement → GREEN → commit `feat(media): probe and media metadata rules`.

---

### Task 10: Thumbnails, encoder capability cache, combined-audio policy

**Files:** `src/thumbnail.rs`, `src/capabilities.rs`, `src/combined_audio.rs`; tests `tests/thumbnail.rs`, `tests/capabilities.rs`, `tests/combined_audio.rs`. Reference `clip_grid.py` poster extraction, `core/encoder_capabilities.py` (section 4.7 of the UI inventory), `core/combined_audio_policy.py` (section 4.10).

**Interfaces:**
- `pub fn extract_frame(path: &Path, at_seconds: f64, max_width: u32) -> Result<image::RgbaImage>` (seek to keyframe before, decode forward, resize with `fast_image_resize`).
- `pub struct EncoderCapability { pub key: String, pub label: String, pub codecs: Vec<String>, pub codec_names: Vec<(String, String)>, pub supports_presets: bool }`; `pub fn probe_capabilities() -> Vec<EncoderCapability>` (by `find_by_name` per backend table, Linux relabel of `amf` to "VA-API (AMD / Intel)" with `*_vaapi` first); `pub fn load_cached(paths: &FthrPaths, signature: &CacheSignature) -> Option<Vec<..>>`, `pub fn store_cached(...)` writing `encoder_capabilities.json` with `version: 3, platform: "linux", ffmpeg: <path>, ffmpeg_size, ffmpeg_mtime_ns, capabilities`.
- `pub fn select_combined_streams(manifest: Option<&AudioManifest>, labels: &[String]) -> Vec<usize>` and `pub fn build_amix_filter(inputs: &[usize]) -> String` matching the Python strings exactly (single input → `anull`).

- [ ] Tests: `thumbnail_from_written_clip_has_expected_size`, `capability_table_matches_fixture`, `cache_round_trip_and_signature_mismatch`, `combined_policy_cases_match_fixture`.
- [ ] RED → implement → GREEN → commit `feat(media): thumbnails, encoder capability cache, combined audio policy`.

---

### Task 11: Crate surface, CHANGES, CI, final gate

- [ ] `lib.rs` doc table, `#![deny(missing_docs)]`, `#![deny(unsafe_code)]` with the `ffi` allowance; CI apt step; `CHANGES.md` entries for: rename no-replace fallback (fixes reference bug 19), audio failure reported to the caller (bug 21), fragmented recording on Linux (reference refused it), `EncodeFrame` honouring the source stride/format checks (bug 23).
- [ ] `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace && cargo deny check`, fixture regeneration diff clean; roadmap row marked shipped; commit and push.
