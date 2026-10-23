# Changes relative to FTHR Clips linux-2.0 (aa0dc9b)

## Fixed

- The uploader ships inside the app as `fthrust-uploader` and is enabled by the consent dialog; the reference's install root, bundle hash and executable were Windows-only, so uploading could not be turned on under Linux.
- Saved SFTP passwords work on Linux through the desktop keyring (the reference required Windows Credential Manager).
- Test connection reports a refused or missing endpoint: your server's 401/403 ("rejected the authorization header") and other 4xx/5xx replies fail, as do Catbox 5xx replies (the reference reported any HTTP reply as reachable).
- Uploads retry only network errors, 429 and 5xx replies, and never past 540 s (the reference retried every failure, including a bad URL, and could outlive its own 600 s timeout).
- A card's "Uploading…" state clears when the upload finishes or fails, so the share button and RETRY NOW work again without a restart.
- The clip card's right-click menu has the "Upload" entry the settings label promises.
- Interval uploads also scan imported folders, which were already allowed upload roots.
- Quitting stops a running upload instead of leaving the uploader process behind.
- The editor's Upload button appears and disappears as uploads are switched on or off, instead of being fixed when the editor opened.

- Image layers and the webcam are burned into a saved clip in one re-encode (the reference re-encoded the whole clip once per overlay), with every audio track, its title and the audio manifest kept.
- Webcam frames are placed at their capture times; the reference wrote them at a fixed 30 fps, so a 15 fps camera played at double speed and drifted.
- An unreadable image layer is skipped with its own warning ("Image overlay could not be read: …") instead of failing every layer.
- Cameras are listed with their V4L2 names (libavdevice) instead of "Camera 1…8" from probing OpenCV indices; the running camera is never reopened by a refresh.
- Every Settings browse button works through the desktop file chooser portal; theme, font, icon and sound import failures show a message instead of only printing to the console. Custom icons are resized and stored without the reference's crop dialog.
- The saved card's thumbnail refreshes after its overlays are burned in.
- Gary mode draws on a click-through layer-shell surface (Qt's stay-on-top hints do nothing on Wayland).

- Exports run in-process through FFmpeg's libraries instead of the `ffmpeg` command line; the same graphs, encoders and validation apply.
- A sped-up export of several kept ranges keeps every range's video: speed now applies to each range's audio before the concat (the reference sped audio up after it, so `-shortest` cut the later ranges).
- Ctrl+Q in the editor is only the quick crop: egui's built-in Ctrl+Q quit (which closed the whole app while a clip was still loading) and its Ctrl+plus/minus zoom are off.
- The reference's "FFMPEG NOT FOUND" error cannot occur: there is no `ffmpeg` binary to find.
- A ready share window offers "Copy path" and "Open folder" instead of dragging the file out: winit cannot start a file drag on Wayland.
- Leaving an editor (minimize, clip navigation) is refused only while one of its shares runs; queued exports keep running while you browse, as the jobs window promises. The reference's guard watched a per-editor export thread that nothing starts any more.

- The editor's "Ready" status clears after 900 ms as intended (the reference compared it to "READY" and never cleared it).
- The editor's Color & Effects panel title shows "COLOR & EFFECTS" instead of Qt's mnemonic rendering "COLOR_EFFECTS".

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

- Clip editor (`docs/editor.md`): timeline with splits, deleted segments, trim handles, zoom, minimap and filmstrip; undo and redo; drafts in `clip_metadata.sqlite3`; speed with pitch preservation, quick crop, crop and stretch dialogs, audio mix, colour and effect values, clip and file details, delete; up to three parked sessions in the rail; clip navigation and the editor keyboard shortcuts. The editor is a layer inside the one window; the reference's basic viewer (editor disabled) is not ported.

- Playback pipeline for the editor (`docs/playback.md`): per-track mixer with `atempo` rate change, keyframe-aware video decoder, the seek-proxy cache for long-GOP clips, the playback service with the PipeWire audio clock and the reference's sync rule, and the wgpu video texture.

- Clips page: the virtualised card grid with thumbnails, filters and sorting, rename and delete, keyboard navigation chords, and the file watcher that keeps it current.
- Engine supervision, replay saves, screenshots, manual recording, hotkey delivery (socket, Hyprland config, GlobalShortcuts portal), game detection, focus pause, the capture card and sounds, single instance and diagnostics export (`docs/capture-and-library.md`).

- `fthrust-ui` crate: the egui shell. Theme tokens, fonts and icons from the same assets, a frameless Wayland window with the title strip, toolbar, navigation rail, error bar, all seven Settings tabs, the four toolbar popups and the dialogs, each held to the PySide6 reference by a marker-for-marker fidelity gate (`docs/ui-fidelity.md`). Visuals change on Apply theme; performance presets and keyboard navigation chords are recorded and validated through the core crate.

- `fthrust-media` crate: FFmpeg encoder with hardware candidates, encoded GOP ring, transactional MP4 writer, fragmented recorder, probe, thumbnails, capability cache and combined-audio policy, tested against reference fixtures and synthetic frames.
- `fthrust-ipc`, `fthrust-wayland`, `fthrust-portal` and `fthrust-engine` crates: the capture daemon with ext-image-copy-capture, wlr-screencopy and ScreenCast-portal backends, PipeWire audio, recovery and health, replay saves, continuous recording and a JSON-over-Unix-socket protocol (`docs/engine-protocol.md`). A synthetic backend (`FTHRUST_SYNTHETIC_BACKEND=1`) lets the whole daemon run in CI.
