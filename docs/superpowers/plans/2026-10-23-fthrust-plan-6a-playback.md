# FTHRust Plan 6a: Playback Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Rust playback pipeline the editor (plan 6b) plays clips through: per-track audio decoding and mixing with pitch-preserving rate change, video decoding into a bounded frame queue uploaded to a wgpu texture, the reference's audio-clock sync rule, and the seek-proxy cache for long-GOP clips.

**Architecture:** `fthrust-media::playback` holds everything FFmpeg: the audio `Mixer` (one decoder per manifest track, resampled to 48 kHz stereo f32, per-track gains, an `atempo` filter chain), the `VideoDecoder` (RGBA frames with presentation times, seeking, single-frame extraction) and the `proxy` cache (probe keyframe gaps, transcode with 30-frame GOPs, prune). `fthrust-ui::services::playback` drives them on worker threads: an audio worker pulls 1024-frame blocks into a bounded PCM queue that a PipeWire playback stream drains (the master clock), a video worker decodes ahead by a few frames and the UI picks the frame whose time has come, and the sync rule holds or reseeks audio when the two drift. Frames go to the screen through `fthrust-ui::video_texture` (a wgpu texture registered with egui-wgpu, re-uploaded when the frame changes).

**Tech Stack:** Rust 2024, `ffmpeg-next` 9 (decode, swscale, libavfilter `atempo`, software H.264 encode for the proxy through the existing `encoder`), `pipewire` 0.10 (playback stream), `egui-wgpu` 0.37 (`Renderer::register_native_texture`), `sha2`.

**Spec:** `docs/superpowers/specs/2026-10-22-fthrust-design.md` (5.7, 6.4 "Playback", 7 "`~/.fthr/playback` proxy cache"); reference `reference/FTHR_UI/core/ffmpeg_playback.py`, `reference/FTHRcapture_common/ffmpeg_playback_bridge.cpp`, `reference/FTHR_UI/core/playback_proxy.py`.

## Global Constraints

- Canonical audio is 48 000 Hz, 2 channels, f32 interleaved (`CANONICAL_SAMPLE_RATE`, `CANONICAL_CHANNELS`); blocks of 1024 frames (`_FRAMES_PER_BLOCK`); the decoded queue holds at most `48000 * 3 / 20 = 7200` frames (`_MAX_QUEUED_FRAMES`); the output buffer target is 40 ms (`_OUTPUT_BUFFER_MS`).
- Sync: drift = audio position − video position; drift > 120 ms holds audio (suspend) until drift ≤ 40 ms (`_SYNC_RESUME_MARGIN_MS`); drift < −120 ms reseeks audio to the video position; a guard of 0.75 s (`_SYNC_RECOVERY_SECONDS`) after any seek or rate change suppresses sync.
- Rates 0.25–4.0; pitch preservation through `atempo` stages each within [0.5, 2.0] (chained); without preservation the block is resampled linearly by the rate (the reference's fallback).
- Proxy cache: `~/.fthr/playback/<sha256(identity|gop30-v2)>.mp4`; built only when a measured keyframe gap in the first 8 s exceeds 2.1 s, the clip is ≤ 300 s and not HDR (`smpte2084`/`arib-std-b67`); the result must be within 0.2 s of the source duration and under 2 GiB; keep at most 8 entries / 2 GiB, newest first; a failure keeps the original path.
- The UI never calls FFmpeg on its thread: decoding, probing and transcoding run on worker threads; the UI thread only pulls ready frames and PCM state.
- No `ColorImage` upload for video: frames go to a `wgpu::Texture` through `Queue::write_texture` and are drawn by `TextureId`.
- Tests generate their own clips with the media crate's writers (`clip_writer`, `recorder`) from synthetic frames and PCM; no binary fixtures.

## Review Focus

1. A clip whose manifest names a track the container does not have (deleted or re-muxed): the mixer opens with the other tracks, reports that source as failed, and the UI shows the track as unavailable rather than refusing playback. Test in Task 1.
2. Seeking past the end, or to a position before the first keyframe of a long-GOP clip: the video worker returns the nearest decodable frame and the position clamps to the duration; no spin, no hang. Test in Task 2.
3. A proxy build interrupted (cancel or crash) leaves a `.partial.mp4` under the cache: the next open ignores partials and prunes them; a cache entry whose source changed (identity differs) is never used. Test in Task 3.
4. The PipeWire stream disappears mid-play (device unplugged, PipeWire restarted): the service reports the failure once, stops cleanly and lets the editor show the error instead of freezing the control bar. Test in Task 4 (fake output failure).
5. Rate change while playing at a position deep in the clip: the audio worker restarts from the current position with the new rate (no jump to 0), and video follows within the guard window. Test in Task 4.

---

## File structure

```
crates/fthrust-media/src/playback/mod.rs        constants, SourceSpec, PlaybackError
crates/fthrust-media/src/playback/mixer.rs      Mixer (per-track decode, resample, gains, atempo chain, seek)
crates/fthrust-media/src/playback/video.rs      VideoDecoder (open, metadata, seek, next_frame, frame_at)
crates/fthrust-media/src/playback/proxy.rs      cache_path, needs_seek_proxy, probe, build, prune
crates/fthrust-media/tests/playback_mixer.rs
crates/fthrust-media/tests/playback_video.rs
crates/fthrust-media/tests/playback_proxy.rs
crates/fthrust-media/tests/common/mod.rs        synthetic clip generator shared by the three
crates/fthrust-ui/src/services/playback/mod.rs  PlaybackService (API, workers, sync)
crates/fthrust-ui/src/services/playback/audio_out.rs  AudioOutput trait, PipeWire impl, null impl
crates/fthrust-ui/src/services/playback/sync.rs       pure sync rule
crates/fthrust-ui/src/video_texture.rs          VideoTexture over egui-wgpu
crates/fthrust-ui/tests/playback_service.rs
crates/fthrust-ui/tests/video_texture.rs
```

---

### Task 1: Audio mixer with rate change

**Files:**
- Create: `crates/fthrust-media/src/playback/{mod,mixer}.rs`, `crates/fthrust-media/tests/common/mod.rs`, `crates/fthrust-media/tests/playback_mixer.rs`
- Modify: `crates/fthrust-media/src/lib.rs` (`pub mod playback;`)

**Interfaces (Produces):**
- `playback::{CANONICAL_SAMPLE_RATE: u32 = 48_000, CANONICAL_CHANNELS: usize = 2, FRAMES_PER_BLOCK: usize = 1024, MAX_QUEUED_FRAMES: usize = 7200}`.
- `playback::SourceSpec { source_id: String, container_index: usize }` (from the audio manifest's `container_index`).
- `mixer::Mixer::open(path: &Path, sources: &[SourceSpec]) -> Result<Mixer, MediaError>`: fails with `MediaError::Invalid("This clip has no decodable audio stream.")` when no source opens; a source whose index is missing or undecodable is recorded in `failed_sources() -> Vec<usize>` (indexes into `sources`) and mixed as silence.
- `Mixer::pull(&mut self, frames: usize, gains: &[f32], master: f32) -> Result<Option<Vec<f32>>>`: `frames` output frames at the canonical format with each track scaled by `gains[i] * master`; `Ok(None)` once every track hit EOF (a short last block is returned before that).
- `Mixer::seek(&mut self, position_ms: u64) -> Result<()>` (flushes decoders and the tempo chain; the next pull starts at `position_ms`).
- `Mixer::set_rate(&mut self, rate: f32, preserve_pitch: bool) -> Result<bool>`: returns `true` when an `atempo` chain is active (rate ≠ 1 and preserve), `false` when the caller must resample; rates are clamped to 0.25–4.0; the chain is `ceil(log2(rate))` stages of `atempo=<rate^(1/n)>` so each stage is within [0.5, 2.0].
- `Mixer::source_count() -> usize`, `Mixer::position_ms() -> u64` (source time of the next frame to be returned).
- `tests/common/mod.rs::write_test_clip(dir, seconds: f64, tracks: &[(&str /*label*/, f32 /*tone Hz*/)]) -> (PathBuf /*mp4*/, AudioManifest)`: a `w×h` 320×180 synthetic video (frame counter drawn as a solid colour cycling per frame) at 30 fps with one AAC track per tone, written through `clip_writer`, plus the manifest with container indexes.

- [ ] Step 1: Write the failing tests in `tests/playback_mixer.rs`:
  - `mixer_mixes_tracks_with_gains`: clip with tones 440 Hz and 880 Hz; `pull(4800, &[1.0, 0.0], 1.0)` has energy near 440 Hz and none near 880 Hz (a 4800-point DFT bin check); `pull(4800, &[0.0, 1.0], 0.5)` has the 880 Hz energy at about half the amplitude of a `[0,1],1.0` pull.
  - `mixer_reports_missing_track_and_plays_the_rest`: `SourceSpec` with `container_index: 7` plus a valid one → `open` succeeds, `failed_sources() == vec![0]`, pulls are non-silent.
  - `mixer_no_tracks_is_an_error`: both indexes invalid → `Err` with the exact message.
  - `mixer_seek_moves_position`: 4 s clip; `seek(2500)`; `position_ms()` in 2400..=2600; the pulled samples match a fresh mixer's samples at 2.5 s (first 480 frames equal within 1e-3).
  - `mixer_rate_with_pitch_preserved_keeps_frequency`: `set_rate(2.0, true) == true`; a 4800-frame pull consumes about 9600 source frames (`position_ms` advances ≈ 200 ms) and the dominant bin stays at 440 Hz; `set_rate(0.5, true)` consumes about 2400; `set_rate(3.0, true)` builds two stages (position advances ≈ 300 ms).
  - `mixer_rate_without_pitch_returns_false`: `set_rate(1.5, false) == false` and pulls are unchanged in length (the caller resamples).
  - `mixer_eof_returns_none`: pull past the end → a short block, then `Ok(None)`.
- [ ] Step 2: Run `cargo test -p fthrust-media --test playback_mixer` → FAIL: unresolved module `fthrust_media::playback`.
- [ ] Step 3: Implement `playback/mod.rs`, `playback/mixer.rs` (decoders through `codec::context::Context::from_parameters`, `frame_to_stereo` + `resample_stereo` reused from `audio_decode` by making them `pub(crate)`, the `atempo` chain through `ffmpeg_next::filter::Graph` with `abuffer`/`abuffersink`), and `tests/common/mod.rs`.
- [ ] Step 4: Run the test file → PASS; `cargo clippy -q -p fthrust-media --all-targets -- -D warnings` clean.
- [ ] Step 5: Commit `feat(media): playback mixer with per-track gains and atempo rate change`.

---

### Task 2: Video decoder and frame access

**Files:**
- Create: `crates/fthrust-media/src/playback/video.rs`, `crates/fthrust-media/tests/playback_video.rs`

**Interfaces:**
- Consumes: `tests/common::write_test_clip`.
- Produces: `video::VideoInfo { width: u32, height: u32, fps: f64, duration_ms: u64 }`; `video::Frame { pts_ms: u64, width: u32, height: u32, rgba: Vec<u8> }`; `video::VideoDecoder::open(path: &Path) -> Result<VideoDecoder>`, `info(&self) -> &VideoInfo`, `seek(&mut self, position_ms: u64) -> Result<()>` (seeks to the keyframe at or before the position and decodes forward so the next `next_frame` is the first frame with `pts_ms >= position_ms`, clamped to the last frame at or past the end), `next_frame(&mut self) -> Result<Option<Frame>>` (`None` at EOF), `frame_at(path: &Path, position_ms: u64) -> Result<Frame>` (one-shot, for paused seeks; the `extract_frame` logic without scaling).

- [ ] Step 1: Write the failing tests in `tests/playback_video.rs`:
  - `decoder_reports_info_and_frames_in_order`: 2 s 30 fps clip → `info` 320×180, fps ≈ 30, duration 2000±40; the first 10 frames have strictly increasing `pts_ms` starting at 0 with steps ≈ 33; the pixel colour of frame n matches the generator's colour for n.
  - `seek_lands_on_the_requested_frame`: `seek(1000)` → next frame `pts_ms` in 1000..=1034 and its colour matches frame 30; `seek(1000)` again after reading further returns the same frame (idempotent); `seek(5000)` past the end → the last frame; `next_frame` after it → `None`.
  - `frame_at_matches_decoder`: `frame_at(path, 1500)` equals the decoder's frame after `seek(1500)` (same pts and bytes).
- [ ] Step 2: Run → FAIL (no `video` module).
- [ ] Step 3: Implement `video.rs` (one decoder, `ictx.seek` with the flag range `..ts` so the keyframe before is chosen, decode-forward loop, swscale to RGBA once per size).
- [ ] Step 4: Run → PASS; clippy clean.
- [ ] Step 5: Commit `feat(media): playback video decoder with seek and frame_at`.

---

### Task 3: Seek-proxy cache

**Files:**
- Create: `crates/fthrust-media/src/playback/proxy.rs`, `crates/fthrust-media/tests/playback_proxy.rs`

**Interfaces:**
- Produces: `proxy::cache_dir(home: &Path) -> PathBuf` (`<home>/.fthr/playback`); `proxy::cache_path(cache_dir, source: &Path) -> Result<PathBuf>` (sha256 of `format!("{:?}|gop30-v2", identity)` where identity is `(dev, inode, size, mtime_ns)`); `proxy::KeyframeSample { pts_s: f64, keyframe: bool }`; `proxy::needs_seek_proxy(samples: &[KeyframeSample]) -> bool` (a gap > 2.1 s between keyframes); `proxy::probe(source, seconds: f64 = 8.0) -> Result<Probe { samples, duration_s, hdr: bool }>`; `proxy::prepare(cache_dir, source, cancel: &AtomicBool) -> PathBuf` (returns the cached file if it exists and is non-empty, else the source unless a build is needed and succeeds; builds `<cache>.<pid>-<thread>.partial.mp4` then renames; verifies the duration within 0.2 s and size ≤ 2 GiB; prunes partials and keeps 8 entries / 2 GiB); `proxy::discard(cache_dir, source, cached)`.
- The transcode uses `encoder::Encoder` software H.264 with `gop = 30` and the existing `clip_writer` to mux the re-encoded video with the source's audio packets copied.

- [ ] Step 1: Write the failing tests:
  - `needs_proxy_only_for_long_keyframe_gaps`: samples `[K@0, @1, K@2.0, @3, @4.5, K@4.6]` → true (the 2.0→4.6 gap is 2.6 s); `[K@0, K@1.5, K@3.0]` → false; `[@0.5, K@1.0, K@2.0]` → false (frames before the first keyframe are ignored); `[]` → false.
  - `cache_path_changes_with_identity`: two files with different content → different paths; the same file → the same path; the name ends with `.mp4` and is 64 hex + `.mp4`.
  - `prepare_builds_a_proxy_for_a_long_gop_clip`: a 4 s clip written with `gop = 240` (one keyframe) → `probe` says needed; `prepare` returns a path under the cache dir, the proxy has a keyframe at least every 1.1 s (probe it), its duration is within 0.2 s; a second `prepare` returns the same path without rebuilding (mtime unchanged); `discard` removes it.
  - `prepare_keeps_the_source_for_short_gop_and_cancel`: a 2 s clip with `gop = 30` → `prepare` returns the source and nothing is written; a cancelled build (flag set before the call) returns the source.
  - `partials_are_pruned_and_entries_capped`: write 9 fake `.mp4` entries and one `.partial.mp4` in the cache, run `prune` → the partial is gone and 8 newest remain.
- [ ] Step 2: Run → FAIL.
- [ ] Step 3: Implement `proxy.rs`.
- [ ] Step 4: Run → PASS; clippy clean.
- [ ] Step 5: Commit `feat(media): seek-proxy cache for long-GOP clips`.

---

### Task 4: Playback service: workers, clock, sync

**Files:**
- Create: `crates/fthrust-ui/src/services/playback/{mod,audio_out,sync}.rs`, `crates/fthrust-ui/tests/playback_service.rs`
- Modify: `crates/fthrust-ui/src/services/mod.rs` (`pub mod playback;`), `crates/fthrust-ui/src/services/sounds.rs` (lift the PipeWire stream setup into a shared `pipewire_out` helper the sink and the playback output both use)

**Interfaces:**
- Consumes: `Mixer`, `VideoDecoder`, `proxy::prepare`, `AudioManifest` (`fthrust_core::audio_manifest::read_verified`).
- Produces:
  - `sync::SyncDecision { Hold, Resume, Reseek(u64), None }`; `sync::decide(audio_ms: i64, video_ms: i64, holding: bool, guard_active: bool) -> SyncDecision` with the constants from Global Constraints.
  - `audio_out::AudioOutput` trait: `fn start(&mut self, queue: Arc<PcmQueue>) -> Result<(), String>`, `fn suspend(&mut self)`, `fn resume(&mut self)`, `fn buffered_frames(&self) -> usize`, `fn stop(&mut self)`; `audio_out::PipeWireOutput` (the master clock; `buffered_frames` from the stream's queued frames plus the 40 ms target) and `audio_out::NullOutput { clock: Arc<AtomicU64> }` for tests (drains the queue at real time from a test-controlled clock).
  - `audio_out::PcmQueue` (bounded, `MAX_QUEUED_FRAMES`): `push(block) -> bool`, `read(frames) -> Vec<f32>` (zero-filled when short), `frames()`, `clear()`.
  - `playback::PlaybackService::open(path, home, output: Box<dyn AudioOutput>, ctx: egui::Context) -> Result<PlaybackService, String>` (reads the manifest; prepares the proxy on a worker and switches the video decoder to it when ready; the audio always decodes the original), `play()`, `pause()`, `seek_ms(u64)`, `set_rate(rate, preserve_pitch)`, `set_master_percent(u8)`, `set_source(id, gain_percent: Option<u8>, muted: Option<bool>)`, `position_ms() -> u64` (audio clock when audio is live, else the video clock), `duration_ms()`, `playing() -> bool`, `take_frame() -> Option<Frame>` (the newest frame whose `pts_ms <= position_ms + 16`, at most one per call), `events() -> Vec<PlaybackEvent>` (`Eof`, `AudioFailed(String)`, `SourceFailed(String)`, `ProxyReady`), `close()`.
  - Workers: the audio worker loop ports `_DecodeWorker.run` (play/pause/seek/rate commands, generation counter, 1024-frame blocks, resample fallback, EOF); the video worker decodes ahead up to 4 frames into a `Mutex<VecDeque<Frame>>`, drops frames older than the clock, reseeks on command; the UI thread's `tick()` runs `sync::decide` on every call and applies it (suspend/resume/reseek).

- [ ] Step 1: Write the failing tests in `tests/playback_service.rs` (clips from `fthrust_media` test support; a `NullOutput` with a manual clock):
  - `sync_rule_matches_reference` (pure): drift 150 → Hold; holding and drift 60 → None; holding and drift 30 → Resume; holding and drift −150 → Reseek; drift −130 → Reseek; guard active → None for every input.
  - `playback_advances_and_delivers_frames_in_order`: open a 2 s clip, `play()`, advance the null clock 500 ms in 20 ms steps calling `tick()` each step → `position_ms` in 480..=560 and the frames taken have increasing `pts_ms` ending near 500.
  - `seek_and_rate_keep_position`: `seek_ms(1000)`; `play()`; advance 200 ms → position ≈ 1200; `set_rate(2.0, true)`; advance 200 ms → position ≈ 1600 (not 400, not 0); `set_rate(1.0, true)` → continues from ≈ 1600.
  - `eof_is_reported_once_and_playing_stops`: play to the end → one `Eof` event, `playing() == false`, `position_ms() == duration_ms()`.
  - `audio_failure_stops_cleanly`: a `NullOutput` configured to fail at start → `open` succeeds (video only), `play()` works, `events()` carries `AudioFailed` once, `position_ms` follows the video clock.
  - `missing_track_is_reported_not_fatal`: a manifest naming a missing index → `SourceFailed(id)` once; playback continues.
- [ ] Step 2: Run → FAIL.
- [ ] Step 3: Implement the three modules and the shared PipeWire helper.
- [ ] Step 4: Run → PASS; `cargo test -p fthrust-ui -q` green; clippy clean. Then a live check on this machine: `cargo test -p fthrust-ui --test playback_service pipewire_live -- --ignored` (plays 1 s of a test clip through PipeWire; asserts `buffered_frames` moves and no error).
- [ ] Step 5: Commit `feat(ui): playback service with audio clock and sync`.

---

### Task 5: Video texture and the frame path to egui

**Files:**
- Create: `crates/fthrust-ui/src/video_texture.rs`, `crates/fthrust-ui/tests/video_texture.rs`
- Modify: `crates/fthrust-ui/src/lib.rs` (`pub mod video_texture;`, `App::render_state: Option<egui_wgpu::RenderState>` set by the runner), `crates/fthrust-ui/src/runner.rs` (pass `painter.render_state()` after `set_window`), `docs/capture-and-library.md` → new `docs/playback.md` (the pipeline, the sync rule, the proxy cache, the live test), `CHANGES.md`, roadmap row 6 split into 6a/6b/6c.

**Interfaces:**
- Produces: `video_texture::VideoTexture::new(render_state: &RenderState) -> VideoTexture`; `update(&mut self, render_state, frame: &Frame)` (recreates the wgpu texture when the size changes, `write_texture` otherwise, re-registers the `TextureId` only on recreation); `id(&self) -> Option<egui::TextureId>`; `size(&self) -> Option<(u32, u32)>`; `clear(&mut self, render_state)` (frees the texture).

- [ ] Step 1: Write the failing test `video_texture_uploads_and_resizes` in `tests/video_texture.rs`: create a headless wgpu device (`Instance` + `request_adapter` with no surface + `request_device`) and an `egui_wgpu::Renderer`; build a `RenderState` for them; `update` with a 320×180 red frame → `id()` is `Some`, `size() == Some((320,180))`; `update` with a 640×360 frame → `size()` changes and `id()` changed; read the texture back with a copy to a buffer and check the first pixel is the frame's colour. Skip with a message when no adapter exists.
- [ ] Step 2: Run → FAIL.
- [ ] Step 3: Implement `video_texture.rs`, the runner/App plumbing, the docs.
- [ ] Step 4: Run → PASS; full gate (`cargo fmt --check`, clippy workspace, `cargo test --workspace`, `cargo deny check`).
- [ ] Step 5: Commit `feat(ui): wgpu video texture; docs: playback`.
