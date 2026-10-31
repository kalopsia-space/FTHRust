# FTHRust audit fixes (2026-10-31)

Fixes everything in `docs/superpowers/audit-2026-10-31.md` (the audit). The audit has the evidence for each item; this plan splits the work into tasks with owned files so tasks can run in parallel worktrees and merge cleanly.

Spec: `docs/superpowers/specs/2026-10-22-fthrust-design.md`. The Qt reference screenshots in `docs/reference/qt-screens/` stay the visual spec.

## Global constraints

1. The machine is shared with a running game. Build and test at low priority: prefix every cargo command with `nice -n 19 ionice -c 3` and set `CARGO_BUILD_JOBS=3`.
2. Run tests with `env -u WAYLAND_DISPLAY` (some live Wayland tests otherwise capture the desktop session).
3. Never start the app, the engine or the uploader against the desktop session. UI checks go through the headless bench only: `tools/uidrive/bench.sh` (see `tools/uidrive/README.md`).
4. Every fix gets a regression test that fails before and passes after, unless the path needs hardware the tests can't have (VA-API, NVENC, layer-shell, a portal). Then unit-test the logic you can isolate and say in the report what stays unverified.
5. Items labelled **reported** are unverified reviewer claims. Confirm each against the code first. If one is wrong, skip it and say why in the report.
6. `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings` are clean for every crate you touched, and those crates' full test suites pass.
7. Behaviour and looks stay as the Qt reference has them unless the item says otherwise.
8. Update `docs/*.md` where behaviour changes. Do not edit `CHANGES.md`; put proposed `CHANGES.md` lines (one sentence each, in that file's style: what was wrong in the reference or alpha, what happens now) in your report.
9. Commits: one concern each, subject in the existing style (`fix(engine): …`, `perf(media): …`), a body that says why, ending with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
10. Stay inside your task's files. If a fix needs a change elsewhere, make the smallest one and list it under "Outside my area" in the report.

## Task 1: Uploader

Files: `crates/fthrust-uploader/**`, `crates/fthrust-ui/src/services/upload.rs` (response handling only), `tools/check-upload-isolation.sh`, `docs/uploads.md`.

1. **HTTP uploads over 120 s always fail** (tier 1 #10, verified). reqwest 0.13's `read_timeout` is one timer from request start to response headers; request-body progress never resets it (`http.rs:44-58,344`). Keep `connect_timeout`, drop `read_timeout` on upload requests, and add a stall watchdog on the streamed body: fail when no bytes have been taken for `IO_TIMEOUT`, and bound the wait for response headers after the body ends. Test: a local server that reads slowly but steadily for longer than the (test-shortened) timeout succeeds; one that stops reading fails with a stall message.
2. **History write failure plus auto-delete loses the link** (tier 1 #13, verified; `http.rs:568-590`). When `History::update` fails, don't auto-delete, and return the link in the response together with the error so the UI can still show it.
3. **reported:** auto-delete accepts any non-empty `url` (`queued`, `file:///…`, multi-line). Only a link that `history::link` would accept (https, or the valid `sftp://` form) allows auto-delete.
4. **reported:** success messages such as "no link" and "local delete failed" are dropped by the UI, leaving a dead share button.
5. **reported:** interval mode plus imported folders plus auto-delete uploads and deletes whole imported libraries. Ruling: auto-delete only removes files inside the clips directory. Imported folders still upload but are never deleted. Say so in `docs/uploads.md`.
6. **reported:** the SFTP host-key algorithm order is fixed with Ed25519 first, so ECDSA or RSA `known_hosts` entries read as untrusted. Prefer the algorithms `known_hosts` holds for that host.
7. **reported:** client-builder errors are retried as network errors.
8. **reported:** `describe()` leaks URL query strings (and possibly userinfo) into messages.
9. **reported:** a non-UTF-8 `known_hosts` breaks SFTP trust. Read bytes, skip lines that don't parse.
10. **reported:** a SIGKILL leaves `.part` files on SFTP servers. At least remove them on every error path, and pick a name that a later upload of the same clip cleans up.
11. **reported:** the release binary honours `FTHRUST_UPLOADER_CATBOX_URL`. Keep it for tests and the bench (check how they build), not for release builds.
12. **reported:** SFTP throughput is capped near 512 KiB per round trip (16×32 KiB in flight). Raise the window and measure against the test server.
13. **`tools/check-upload-isolation.sh` fails open** (verified): `2>/dev/null … || true` means a `cargo tree` failure prints "upload isolation ok". Make any failure fatal, and widen the deny-list (hyper, h2, rustls, reqwest, russh, keyring) for the UI, engine, core and media crates.
14. Add `--version` to `fthrust-uploader` (prints `fthrust-uploader <version>` and exits 0).

## Task 2: Core

Files: `crates/fthrust-core/**` except `clip_metadata.rs`, `clip_readiness.rs`, `library_index.rs`, `capture_config.rs` and `settings/migrate.rs`; the diagnostics wiring in `crates/fthrust-ui/src/main.rs`.

1. **The Hyprland Lua keybind file can be wiped** (tier 1 #9, verified). `hotkeys/hyprland.rs:156,192`: `read_to_string(path).unwrap_or_default()` turns non-UTF-8, EACCES or EIO into `""`, then the file is rewritten in place with only the managed block. Only `NotFound` counts as empty; any other read error, and non-UTF-8 content, aborts without writing. Write through a temp file and rename, and keep a `.bak` of the previous contents. Check the `.conf` writer for the same pattern.
2. **Secrets are written world-readable** (verified). `atomic_json.rs:35` uses `File::create`, so `~/.fthr/uploader/settings.json` (holds `upload_auth_header`, `catbox_userhash`) ends up 0644 and each save resets a chmod. Create temp files 0600 (keep an existing file's stricter mode), and make `~/.fthr/uploader` 0700. Test the modes.
3. **Diagnostics are half wired** (verified for the first two). `init_logging` and `init_crash_log` are never called by the UI, so `fthr.log` and `fthr-crash.log` are never written. `active-session.json` is written but never removed, so every launch reports the previous session as unclean. The UI's `EnvFilter::from_default_env()` is ERROR-only without `RUST_LOG`: default to info for our crates and warn for the rest. The support ZIP reads `fthr.log` and a session `engine.log` that nothing writes: make the paths it reads match what is written (check where the UI writes the engine's stderr).
4. **reported:** an unreadable store is treated as empty and then overwritten; `settings.json` is saved at every exit, so a transient read failure replaces it with defaults. On an I/O error don't save over the file; on corrupt JSON keep a copy of the bad file before defaults are written.
5. **reported:** theme ZIP import has no total-size or entry-count cap, and deletes the old assets before writing the new ones. Cap it, extract to a temp directory, then swap.
6. **reported:** egui key names (PageUp, Equals, Backtick, Quote, brackets…) aren't mapped to xkb keysyms, so Hyprland binds and portal triggers come out invalid; the Lua writer only upper-cases. One mapping table used by the `.conf` writer, the Lua writer and the portal trigger strings, with a test over every egui key.
7. **reported:** `cleanup_stale_partials` has no caller and is too narrow, so crashed saves and recordings leak hidden `.partial` files. Call it at startup and cover the recording names too; only delete files old enough that no running save could own them.
8. **reported:** startup calls `exit(2)` when the migration save or theme directory creation fails, so a full disk stops the app opening (and the user can't delete clips from it). Carry on and report the error instead.
9. **reported:** redaction misses `sftp_password=`, `client_secret=`, `upload_token=`, a `userhash` in free text, and multi-pair `Cookie` headers.
10. **reported:** `events.jsonl` is re-redacted as text, which produces invalid JSON. Redact it per line as JSON.

## Task 3: Media FFI, encoders and export

Files: `crates/fthrust-media/**` except `ring.rs`, `recorder.rs`, `payload.rs`, `audio/**` and `ffi/capture_surfaces.rs`. FFmpeg log setup may add one call each to the engine's and the UI's `main.rs`.

1. **The AAC encoder hands swresample `frame->data` (8 pointers) instead of `extended_data`** (verified; `ffi/aac.rs:179-185`). More than 8 planar channels writes through garbage pointers. Use `extended_data`, and validate the channel count up front.
2. **Software encoders run on one thread** (verified; `ffi/codec.rs:125-148` never sets `thread_count`). Set `thread_count = 0` (auto) for software encoders; leave hardware encoders alone.
3. **reported:** CPU colour conversion is single-threaded (the NVENC, QSV, AMF, software and VA-API-fallback path; plan 9 measured 3.29 ms per 1080p frame). Use a threaded swscale context (`sws_alloc_context` with `threads`, `sws_scale_frame`) and measure before and after.
4. **reported:** QSV preset 7 maps to `slowest`, which FFmpeg rejects, and libkvazaar has no `preset` option (it takes `kvazaar-params`). Test the option strings per encoder.
5. **reported:** a failed `encode()` swallows a pending keyframe request.
6. **reported:** frame-size checks use unchecked arithmetic (`encoder.rs:607`, `scaler.rs:98,106`, `vpp.rs:210`).
7. **reported:** `encoder_works` passes without producing a packet.
8. **reported:** a failed `renameat2` plus a failed hard-link fallback deletes the fully written `.partial` (for example on FUSE without rename2), so the clip is lost (`transactional_save.rs:136-160,237-239`). Never delete the only copy.
9. **Stream-copy trims with audio demux the whole rest of the file** (verified; `export/run.rs:739-788`). The `break` needs `copy_audio` empty, and skipped packets don't refresh `last_output`, so long recordings on slow storage hit the 120 s inactivity timeout. Stop once every stream is past the end, and count any progress as activity.
10. **reported:** `export::run` never checks the output duration, and finalize renames the result over the original clip with sink errors swallowed. Check the duration before replacing, and pass sink errors on.
11. **reported:** the camera ring is capped by time only (90 s): 400-800 MB in the UI process at 1080p30 MJPEG. Add a byte cap.
12. **reported:** `Job::max_rate_kbps` is never read. Wire it into rate control, or remove it if nothing should set it.
13. **reported:** export staging files are never reclaimed after a crash.
14. **FFmpeg logging is never configured** (no `av_log_set_level` or callback), so every INFO-level encoder and demuxer line goes to stderr. Default to warnings, route FFmpeg's log into `tracing`, and allow an override through an environment variable. The "UDTA parsing failed retrying raw" line on every clip open is a harmless stock FFmpeg quirk; keep it out of the warning level.

## Task 4: Save and recording pipeline

Files: `crates/fthrust-engine/src/save.rs`, `daemon.rs` and the recording code; `crates/fthrust-media/src/ring.rs`, `recorder.rs`, `payload.rs`, `audio/**`; the status-lock region of `crates/fthrust-engine/src/engine.rs` (around 327-355).

1. **Recordings drift** (tier 1 #5, verified twice; `save.rs:328-358`, `recorder.rs:171-203`). Each 100 ms audio step asks for `floor(elapsed_ms)` but advances the cursor by the full elapsed time, dropping ~0.45%: audio ends about 2.7 s early per 10 min. Audio also starts 0.1-0.2 s before the first video keyframe, there's 21 ms of un-trimmed AAC priming in the fragmented MP4, and empty windows (PipeWire reconnects) are skipped instead of filled. Count the cursor in samples, start it at the first written keyframe's `wall_time_ns`, write silence for gaps, and compensate the priming. Test with a simulated clock: 10 minutes of steps gives duration × rate samples within one AAC frame, and the first sample lines up with the first keyframe.
2. **Engine shutdown can hang forever** (tier 1 #7, verified). With no client, events go to a 1-slot dummy channel whose receiver is alive but never read (`daemon.rs:391`); the shutdown save-wait does `events.send(e).await` with no timeout (`daemon.rs:477-485`), so `service.shutdown()` never runs. Trigger: the UI dies during a save. Events without a client must never block, and every shutdown wait needs a bound.
3. **The shutdown save-wait can drop `SaveResult`** (E2, observed: `daemon::shutdown_waits_for_a_save_in_flight` failed 6 of 40 runs). The save thread sends the result, then clears `busy`; the wait exits on `!is_busy()` while the event is still crossing the forwarder thread (`daemon.rs:473-490`, `save.rs:76-81`). Wait for the in-flight request's result, not the flag. The test must pass 100 of 100 isolated runs.
4. **The disk replay buffer silently reverts to RAM** after any encoder reopen or geometry change: `ring.rs:299-303` `clear()` replaces `Inner`, dropping the disk store (verified). Clear the contents, keep the store.
5. **reported:** full-length saves anchor audio and the start keyframe on the request time, not the newest packet, so audio leads video by the encoder's latency (`ring.rs:226-265`, `save.rs:127-169`).
6. **reported:** clips end on 20-50 ms of zero-padded silence because the save waits for video but not audio.
7. **reported:** stopping a recording drops queued packets and the last ~200-300 ms of audio.
8. **reported:** the microphone is never recorded in full recordings.
9. **reported:** the keyframe is requested before the recording tap is installed.
10. **reported:** `Recorder::start` failures leave an empty `FTHR_recording_*.mp4`; recordings are never fsynced; the recorder writes dts = pts.
11. **reported:** disk-mode `pwrite` and `sync_file_range` run on the capture thread under the ring mutex (`ring.rs:150-165`, `payload.rs:183-226`), so a save to the same disk can stall capture.
12. **reported:** long saves stall the capture thread and the PipeWire RT callback: the status mutex is held across `audio.status()` (`engine.rs:327,348-355`), and `window()` copies every block under the source lock, about 46 MB for a 120 s clip (`audio/pipewire.rs:146-153,375-394`).
13. **reported:** a save-thread panic leaves `busy` set forever.
14. **reported:** StartRecording does file I/O on the daemon loop.
15. **reported:** a configure-job `JoinError` wedges the daemon.
16. **reported:** SIGTERM is honoured only once.
17. **reported optimisations:** a single-generation fast path for `snapshot()`; encode AAC straight from the slice (halves peak audio memory on long saves). Ruling: skip "keep FFmpeg's packet buffer as the payload" — about 12 MB/s of memcpy at 50 Mbps and 180 fps is well under 1% of a core, and carrying `AVBufferRef` lifetimes through the ring and the disk store isn't worth the risk.

## Task 5: Capture loop, Wayland and portal

Files: `crates/fthrust-engine/src/engine.rs` (capture loop, watchdog, startup backend choice), `portal_backend.rs` and the other backends; `crates/fthrust-wayland/**` except `layer_shell.rs`; `crates/fthrust-portal/**`; `crates/fthrust-media/src/ffi/capture_surfaces.rs`.

1. **Portal capture can't be reopened inside one engine process** (tier 1 #6, verified twice). Each open builds a tokio runtime that is dropped with the backend (`portal_backend.rs:16-31,65-73`); ashpd caches one process-wide zbus connection whose socket reader was spawned on that runtime, so every reopen (recovery, reconfigure) talks to a dead connection and fails with BACKEND_FAILED. The failed retry also deletes the restore token (`fthrust-portal/src/screencast.rs:116-136`), so the next start shows the picker. Use one long-lived portal runtime for the process, never delete the token on transport errors, and save the new token as soon as `Start` returns.
2. **reported:** the portal session is never closed on error paths.
3. **Portal capture copies every frame twice** (verified). `stream.rs:326` `self.last = taken.clone()` deep-copies `BgraFrame { data: Vec<u8> }` (14.7 MB at 1440p) and drops the original: move it instead. `copy_rows` converts BGRx per pixel under the slot mutex (`stream.rs:51-67`).
4. **Wayland waits busy-spin their last sub-millisecond** (verified). `dispatch.rs:286-287` builds `PollTimeout` from `as_millis()`, which truncates, so the final <1 ms becomes a `poll(0)` loop. Round up.
5. **Flipped90 and Flipped270 un-transforms are swapped** relative to the code's own Rot90 convention (`frame.rs:308,310`), so flipped outputs come out rotated 180° (verified by derivation). Test all eight transforms on a small asymmetric buffer.
6. **reported:** `ext_capture.rs:666-692` waits 2 s for a constraints batch that already arrived, on every resolution change.
7. **reported:** after a mode switch there is no repeat source, so a static screen can time out three times and push the engine off ext and zero-copy.
8. **reported:** the repeat deadline races fresh frames when the source rate is close to the capture rate, causing judder.
9. **reported:** capture requests are not paced (`engine.rs:957-986`). Frames are requested back to back and those before `next_encode_ns` are dropped; at 60 fps on a 144/240 Hz output, 58-75% of compositor copies are wasted.
10. **reported:** the watchdog reads back the whole surface once a second in zero-copy mode, with an allocation, zero-fill and copy, to sample 144 pixels, on the capture thread (`engine.rs:1008-1013`, `capture_surfaces.rs:174-212`).
11. **reported:** the watchdog never triggers recovery, though the docs say it does. Make it work as documented.
12. **reported:** startup falls through to the portal after a transient ext/wlr failure, which risks a picker dialog.
13. **reported:** create the two unused VA pools lazily (about 45 MB of VRAM at 4K).

## Task 6: UI library, editor and playback

Files: `crates/fthrust-ui/src/library/**`, `editor/**`, `video_texture.rs`, `services/playback/**`; `crates/fthrust-core/src/clip_metadata.rs`, `clip_readiness.rs`, `library_index.rs`.

1. **The Clips grid goes blank after scrolling** (tier 1 #1, observed on the bench). `library/page.rs:490-594`: `ScrollArea::show_viewport` passes a content-relative viewport, but the page builds screen-space rects from `origin = ui.cursor().min` and compares them directly. Even unscrolled, the second day header and row-2 cards don't draw; after about 2k px of scroll nothing draws. Translate the viewport into the same space as the rects (check egui 0.37 `scroll_area.rs:878,912` for the frames), and add a test that seeds about 60 clips over two days and checks headers and cards are drawn at the top and after scrolling.
2. **Renaming a clip loses its tag, description and editor draft** (tier 1 #11, verified). `rename_clip` (`page.rs:243-278`) moves the file, manifest, upload key and thumbnails, but never calls `ClipMetadataStore::rename_path` (`clip_metadata.rs:291`, no callers). Move the metadata and the draft too.
3. **reported:** the Clips page does O(library) work per frame: `sections.clone()`, `records.iter().any(...)` per built card, and `can_access()` (canonicalize + stat) per card (`page.rs:547,629-637,87-122`; `clip_readiness.rs:36-46,124-138`). JPEG thumbnails decode on the UI thread, the queue is FIFO with stale jobs, and `thumb_failed` never expires.
4. **reported:** the library watcher rescans continuously during a recording: every `.partial` Modify event re-arms the debounce, and each scan canonicalizes every file and re-walks the tree for `watched_dirs` (`library/scan.rs:209-251`, `library_index.rs:141-265`).
5. **reported:** day-section labels have no year, so the same date in different years merges into one section. Group by full date; keep the label the reference shows.
6. **reported:** Export, Share and Ctrl+E before the player loads export a 1 ms range (`duration_ms = 1` placeholder).
7. **reported:** Esc in the Share picker also closes the editor.
8. **reported:** library rename and delete ignore parked editor sessions and queued exports.
9. **reported:** the trim-handle grab prefers Start when the handles are close; pick the nearer handle.
10. **Each closed editor leaks its GPU video texture** (verified). `free_texture` runs only on a size change (`video_texture.rs:86-128`); `editor_video = None` just drops it (`editor/sessions.rs:106,135`): 8 MB at 1080p, 33 MB at 4K per editor session.
11. **reported:** closing or parking the editor joins a decode worker mid-GOP: a 0.4-0.8 s UI stall.
12. **reported:** every pause re-seeks and re-decodes a GOP.
13. **reported:** the editor video worker polls every 4 ms, 250 wakeups a second per open editor (`services/playback/mod.rs:275-378`).
14. **reported:** opening the editor SHA-256s the whole clip twice.
15. **reported:** a clip-metadata DB failure is retried every frame, with a log line each time.

## Task 7: CI, build, dependencies, repo, scripts, docs and licences

Files: `.github/**`, `Cargo.toml` (profiles and workspace dependencies), `deny.toml`, `.gitignore`, `packaging/**`, `tools/uidrive/bench.sh`, `README.md`, `docs/**` (except docs owned by other tasks' behaviour changes), `CHANGES.md` (links only), `THIRD_PARTY_NOTICES.md`, `licenses/**`.

1. **Release profile** (measured: −33% binary size): `lto = "fat"`, `codegen-units = 1`, `strip = "debuginfo"`. `panic = "abort"` is out (`catch_unwind` is used). Check the release and packaging builds still work.
2. **CI `check` job:** add `Swatinem/rust-cache`; install cargo-deny and cargo-about prebuilt (taiki-e/install-action) instead of `cargo install`; add a `concurrency` group that cancels superseded runs; stop the double runs (push plus `pull_request` on the same branch, and tags); `--locked` on cargo commands; `timeout-minutes` on every job.
3. **Release isn't gated on CI** (reported): a tag push publishes even if CI is red; `release.yml` runs no fmt, clippy or tests. Gate it.
4. Pin actions to commit SHAs (with the version in a comment) and add `.github/dependabot.yml` for github-actions and cargo.
5. **reported:** CI tests against Ubuntu's FFmpeg 6.1 while the release ships BtbN 8.1.2. Test against the pinned BtbN build the packaging uses.
6. `tools/uidrive` is never fmt'd, linted or tested in CI.
7. **Dependencies:** drop the unused `eframe` and `tracing-appender` workspace dependencies and `egui_kittest`'s `eframe` feature; `zip` with `default-features = false, features = ["deflate"]` (check that theme import/export and the support ZIP still work); `cargo update -p clipboard-win` (yanked 5.4.1); remove the unused licence allowances in `deny.toml` (GPL-3.0, MPL-2.0, OFL-1.1, Ubuntu-font-1.0) after confirming with `cargo deny` that nothing needs them.
8. **Repo:** add `__pycache__/` and `*.pyc` to `.gitignore` and untrack `tools/__pycache__/test_render_qt_reference.cpython-314-pytest-9.1.1.pyc`.
9. **Release notes and provenance:** release notes are only a compare link; use the tag's `CHANGES.md` section. Add a build provenance attestation for the AppImage and tarball.
10. **`bench.sh`** (reported): `rm -rf "$BENCH_DIR"` runs on whatever it's given; it uses predictable `/tmp/uidrive-$$` names; teardown can abort under `set -e`; `--engine real` sets the invalid backend name `screencopy` (valid: `wlr`).
11. **Docs** (reported): `CHANGES.md` links five docs that don't exist; the test count is stated three different ways; the design spec still describes eframe and nokhwa; the tarball's notices links dangle; the launcher name "FTHR Clips" collides with the original app. Ruling: name the launcher "FTHRust Clips", which is the window title.
12. **Licences** (reported, legal conclusion unconfirmed): the bundled FFmpeg's configure line lists about 60 external libraries, but `THIRD_PARTY_NOTICES.md` names only openh264, kvazaar, svtav1 and ffnvcodec. Generate the notices from the bundled build's actual configure line, with each library's licence text. The source route points at a BtbN monthly build that expires around July 2028: archive the matching FFmpeg source with each release and document it.

## Task 8: UI services and settings

Files: `crates/fthrust-ui/src/services/**` except `upload.rs` and `playback/**`; `settings/**`, `popups/**`, `dialogs/**`, `widgets/**`, `state.rs`, `icons.rs`, `fidelity.rs`, `tokens.rs`; `crates/fthrust-core/src/capture_config.rs`, `settings/migrate.rs`; `crates/fthrust-media/src/capabilities.rs` callers.

1. **Encoder probe never runs, device lists are never filled, five actions do nothing** (tier 1 #2).
2. **Quitting during a save loses the clip** (tier 1 #3).
3. **Changing Clip length doesn't resize the replay buffer** (tier 1 #4).
4. **"USE DEFAULT" for the clips folder breaks saving** (tier 1 #12).
5. Restart/apply requests dropped while another is pending; the engine stderr reader stops at the first non-UTF-8 line; `pending_engine_events` never drained; every service event repaints the whole UI.
6. Reported UI-services items: no auto-restart of a crashed engine; saves accepted while stopping; `engine_config` errors swallowed; socket unlink; blocking starts on the UI thread; window not hidden during `prepare_exit`; activation repaints a throwaway Context; `xdg-open` children never reaped; GlobalShortcuts calls after the first bind have no timeout; recording "Finalizing" has no timeout; idle wakeups from polling loops.
7. UI frame time: Customize icon cache rebuilt every frame (and the dialog helpers' take-and-overwrite); settings and theme fsynced on every slider step; icon lookups call `assets_dir()` every time; the General tab reads preset files every frame; fidelity markers built in production; `Tokens` cloned per widget.

## Task 9: Overlay

Files: `crates/fthrust-ui/src/overlay/**`, `crates/fthrust-wayland/src/layer_shell.rs`.

1. **The second and later capture cards are invisible, or crash the overlay thread** (tier 1 #8).
2. A new Vulkan instance, adapter, device and renderer per card and per Gary appearance.
3. Gary deep-copies its image every 33 ms tick, and loads it at 2048 px for an ~820 px on-screen size.
4. The capture card eats clicks in the game's top-right corner (`Placement::Corner` sets no input region).

## Task 10: GitHub settings and branches

Secret scanning, push protection, Dependabot alerts and security updates on; a ruleset on `main` that blocks force pushes and deletion; delete the merged branches (local `custom-server-polish`, `plan-7b-appimage`; remote `plan-1-core`, `plan-7b-appimage`).

## Task 11: Trimmed FFmpeg for the AppImage

After Task 7. Build FFmpeg in the packaging container with only what the app uses (the hardware encoders, the software encoders it offers, the h264/hevc/av1 decoders, aac, mjpeg, mp4/mov, v4l2, the filters it uses), so the AppImage shrinks and the notices list shrinks with it. The existing `packaging/verify-appdir.sh` gates must pass.

## Task 12: Integration

Merge each task, run the whole workspace suite on the merged result, push. At the end: regenerate the docs screenshots on the bench, write the `CHANGES.md` entries, run a final review over the whole range, update the audit with what was fixed.
