# Changes relative to FTHR Clips linux-2.0 (aa0dc9b)

## Fixed

- Screenshots go through the Screenshot portal, so the hotkey no longer produces black images on Wayland.
- Game detection and focus pause work on Wayland through the compositor's toplevel list instead of `xdotool`.
- Manual recording works on Linux (the reference disabled the button there).
- Hyprland hotkey binds call the app's own `--hotkey` client instead of depending on `nc`.
- The capture card is a layer-shell overlay instead of a Qt tool window that compositors could hide; without layer-shell it falls back to a desktop notification.

- Settings pages keep their real width when a scrollbar appears instead of reflowing wrapped labels under it; the Audio page hides the scrollbar when it fits.
- A navigation chord that conflicts with another binding is reported next to the bindings and the old binding is kept, instead of silently overwriting it.
- Theme edits are a draft until Apply theme; Reset all restores every default in one step.

- Settings file that is valid JSON but not an object is quarantined instead of crashing the loader.
- A failed quarantine rename and undecodable settings files no longer stop startup; defaults are used as in the reference's intent.
- Hotkeys loader drops non-string bindings instead of failing on the whole file.
- Theme loader replaces invalid colour strings with the token default instead of propagating them.
- Theme import validates archive entry names and the asset names inside `theme.json`, so a crafted theme cannot read or write outside the theme folders.
- Re-selecting an already installed custom icon, sound or font no longer truncates the file; unknown sound keys are rejected.
- Clip metadata store surfaces a locked database instead of falling back to stale JSON, and patches rows without dropping unknown keys or unparseable drafts.
- Library scan ignores an imported folder that duplicates the clips root or another import, so owned clips are not flagged as imported.
- Clip rename restores the media name when the sidecar step fails, and the rollback is covered by a test.
- Thumbnail cache keys hash raw path bytes, so clips with non-UTF-8 names reuse their thumbnails.
- Diagnostic event fields named like secrets are redacted wholesale, not only when their value looks like a credential.
- Error-bar titles are normalised (trimmed, upper-cased) before code lookup exactly as the reference does.
- A transient read error on `settings.json` or `presets.json` no longer renames the file; corrupt `presets.json` is quarantined before any rewrite instead of being overwritten.
- Theme zip import validates colour values and caps each asset at 64 MiB.
- Export presets this version cannot parse are carried through rewrites untouched instead of being deleted.
- `hotkeys.json` that cannot be read loads defaults instead of blocking startup.
- Diagnostic events and exported log tails replace the home directory with `%USERPROFILE%`, so the export's `paths_redacted` attestation is true.
- Saving a clip on a filesystem without `renameat2(RENAME_NOREPLACE)` (overlayfs, some NFS and FUSE mounts) falls back to `link` + `unlink`, so the no-replace guarantee holds instead of failing the save.
- When the AAC encoder cannot be set up the clip is still written video-only, as before, but the caller is now told (`ClipWriteResult::audio_error`) instead of silently producing a silent clip.
- Continuous recording to a recoverable fragmented MP4 is available on Linux; the reference only built it for Windows. A crash loses at most the group of pictures in progress.
- The encoder validates the source stride and buffer length and rebuilds its scaler when the source size changes, instead of reading past a short buffer or feeding a stale geometry.
- VA-API device selection tries `FTHR_VAAPI_DEVICE`, then `/dev/dri/renderD128`, then every other render node, until one of them actually opens the encoder, so a mixed-GPU machine (NVIDIA on the first node, AMD on the second) still gets hardware encoding.
- Media metadata treats bitrates or dimensions at or above 2^63 as unknown instead of wrapping.
- Encoder capability discovery keys its cache on the loaded libavcodec, since FTHRust links FFmpeg instead of shelling out to an `ffmpeg` executable, and runs each one-frame probe in a child process (the app re-invoked with `--fthrust-probe-encoder`) so a crashing or hanging GPU driver costs one candidate instead of the whole app.
- Starting a continuous recording refuses a path that already exists instead of truncating that file.
- The FFmpeg wrappers are private to the media crate; the scaler and muxer validate frame geometry and stream indexes, and a zero-size encoder packet is copied as empty instead of dereferencing a null pointer.
- Saving a clip no longer blocks the engine's command loop: saves run on their own thread, so status, pause, resume and shutdown keep flowing during a long mux.
- The engine no longer relies on `PR_SET_PDEATHSIG` (which is per parent thread): it exits when the UI closes its stdin.
- The shared-memory region (created with `shm_open` without exclusivity) is replaced by a per-user Unix socket that refuses to replace a live engine.
- The cursor is composited into wlr-screencopy and ext-image-copy-capture frames (the reference captured without it).
- Output transforms (rotated or flipped monitors) are undone before encoding; the reference encoded them rotated.
- Recovery falls through to the next capture backend once the last good one has spent its retry budget, instead of retrying the same backend forever; transient conditions (compositor not answering, selected output absent) are waited out without consuming the budget.
- Audio capture stops within a bounded time (the reference's unbounded `pa_simple_read` could wedge shutdown); audio runs on PipeWire directly, with both the desktop mix and the microphone captured by the engine.
- The engine prints `FTHR_STARTUP_ERROR: <CODE>: <detail>` (and sends the same event) for every startup failure, so the UI can show a specific error instead of the generic Error 004.
- A recording starts on an immediately requested keyframe instead of waiting up to a GOP for the next one.
- v1/v2 wlr-screencopy compositors are handled (the reference could only finish on v3).
- Clip saves carry one AAC track per captured source, labelled `Default Mix` and `Microphone`, and report which tracks were written.

## Added

- Playback pipeline for the editor (`docs/playback.md`): per-track mixer with `atempo` rate change, keyframe-aware video decoder, the seek-proxy cache for long-GOP clips, the playback service with the PipeWire audio clock and the reference's sync rule, and the wgpu video texture.

- Clips page: the virtualised card grid with thumbnails, filters and sorting, rename and delete, keyboard navigation chords, and the file watcher that keeps it current.
- Engine supervision, replay saves, screenshots, manual recording, hotkey delivery (socket, Hyprland config, GlobalShortcuts portal), game detection, focus pause, the capture card and sounds, single instance and diagnostics export (`docs/capture-and-library.md`).

- `fthrust-ui` crate: the egui shell. Theme tokens, fonts and icons from the same assets, a frameless Wayland window with the title strip, toolbar, navigation rail, error bar, all seven Settings tabs, the four toolbar popups and the dialogs, each held to the PySide6 reference by a marker-for-marker fidelity gate (`docs/ui-fidelity.md`). Visuals change on Apply theme; performance presets and keyboard navigation chords are recorded and validated through the core crate.

- `fthrust-media` crate: FFmpeg encoder with hardware candidates, encoded GOP ring, transactional MP4 writer, fragmented recorder, probe, thumbnails, capability cache and combined-audio policy, tested against reference fixtures and synthetic frames.
- `fthrust-ipc`, `fthrust-wayland`, `fthrust-portal` and `fthrust-engine` crates: the capture daemon with ext-image-copy-capture, wlr-screencopy and ScreenCast-portal backends, PipeWire audio, recovery and health, replay saves, continuous recording and a JSON-over-Unix-socket protocol (`docs/engine-protocol.md`). A synthetic backend (`FTHRUST_SYNTHETIC_BACKEND=1`) lets the whole daemon run in CI.
