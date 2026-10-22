# FTHRust — design

Date: 2026-10-22. Status: draft for approval.

## 1. Intent

FTHRust is a from-scratch Rust rewrite of the Linux side of FTHR Clips
(the `linux-2.0` branch of `FTHR-Community/FTHR-Clips`, commit `aa0dc9b`).
FTHR Clips is a local-first instant-replay recorder: it captures the
screen into a rolling buffer, a hotkey saves the last N seconds as a
clip, and a built-in library and editor let the user trim, export and
optionally upload clips. No account, no cloud, no telemetry.

The owner wants the entire Linux app in one language that is faster and
better than the current Python-plus-C++ split. The visible product must
be the same app: same screens, same colours, same fonts, same behaviour,
same files on disk. Where the current code is broken on Linux, the Rust
version fixes it rather than reproducing the bug.

FTHRust lives in its own repository, `kalopsia-space/FTHRust`, private
to start. It inherits the GPL-3.0-only licence and credits
FTHR-Community as the origin.

## 2. Goals

1. One Rust workspace builds the whole Linux app: capture engine, UI,
   uploader, tooling.
2. Pixel-faithful UI. Every Linux screen matches the Qt reference in
   layout, dimensions, spacing, colour tokens, Oswald faces and sizes,
   icons, radii, motion timings and interaction. Text rasterisation and
   edge antialiasing may differ; nothing else may.
3. Full Linux feature parity with `linux-2.0`, as listed in section 5.
4. Wayland only. Hyprland is the primary target, KDE Plasma second.
5. Reads and writes the same on-disk state under `~/.fthr` so an
   existing user upgrades in place and can go back.
6. Fixes every defect found in the reference during the port (section
   9) and records each fix.
7. Ships as an AppImage with CI that builds, tests and gates releases.

## 3. Non-goals and anti-goals

Non-goals (not in this project, may come later):

- Windows. All Windows code, docs and packaging are omitted, not ported.
- X11. No X11 capture backend, monitor enumeration or hotkeys.
- GNOME. Capture there would be portal-only and hotkeys need a newer
  portal; it is not tested or promised in this project.
- Flatpak or distro packages. AppImage only.
- The retired "Lustful" hardware-ID service. It is orphaned dead code
  and is dropped.
- Windows-only features that the branch lists: per-app audio stems,
  third-party keyboard overlay window, DPI handling, autostart.

Anti-goals (failure even if it works):

- A UI that is "close enough". A reviewer putting the Qt screenshot
  and the FTHRust screenshot side by side must not find a layout,
  colour, size, font or icon difference.
- Breaking a user's existing library, settings, presets, theme, hotkeys
  or upload history.
- Reintroducing Python, C++ or Qt at runtime.
- Capture that works only on one GPU vendor. VAAPI on AMD and Intel,
  NVENC on NVIDIA, software on everything, with automatic fallback.
- Trading A/V sync or clip-duration accuracy for simplicity.

## 4. Constraints

- Licence: GPL-3.0-only, same `LICENSE` text. FFmpeg stays the LGPL
  shared-library build and is dynamically loaded, never statically
  linked. THIRD_PARTY_NOTICES is regenerated for Rust crates
  (`cargo-about` or `cargo-deny` licences output).
- Toolchain: stable Rust, current at build time, pinned with
  `rust-toolchain.toml`. Release AppImage built in an Ubuntu 22.04
  container for a glibc 2.35 baseline, matching the current gate.
- Runtime dependencies allowed on the host: Mesa or vendor VAAPI driver,
  libva, PipeWire, Wayland, xdg-desktop-portal with a backend that
  implements ScreenCast, Screenshot and GlobalShortcuts. Everything
  else is bundled or pure Rust.
- Assets: the Oswald fonts, SVG and PNG icons, sounds and images are
  copied from the reference unchanged. The reference `ui-preview`
  screenshots (now under `docs/reference/qt-screens/`) are the visual
  spec.
- Config root stays `~/.fthr`. File names and formats in section 7 are
  frozen; new state goes in new files or new keys, never by changing
  existing ones.
- Reference source: the builder has the original code. `tools/fetch-
  reference.sh` clones `FTHR-Community/FTHR-Clips` at commit `aa0dc9b`
  (branch `linux-2.0`) into `reference/`, which is gitignored. Every
  `path:line` pointer in the inventories resolves against that
  directory. The older checkout at `/home/kalopsia/FTHR-Clips` is 49
  commits behind and its uncommitted edits are Linux-compatibility
  patches that upstream has since superseded; it is not the reference.
- Reference screenshots: the 19 PNGs under `docs/reference/qt-screens/`
  were rendered by the reference's `tools/render_ui_preview.py` under
  PySide6 6.12 in a venv (`opencv-python-headless numpy sounddevice
  jeepney` on top of system PySide6). Before building any screen that
  has no PNG (Clips empty state, each dialog, screenshot editor, camera
  overlay editor, error bar states, capture card, export jobs, upload
  settings), the builder extends that script as
  `tools/render-qt-reference.py` to capture it at the same window sizes
  and commits the PNG. No screen is built from the prose alone.

## 5. What the app does (feature inventory for parity)

Everything below exists on `linux-2.0` and must work in FTHRust. Deep
per-feature detail, with line pointers into the reference, is in
`docs/reference/reference-ui-inventory.md` (UI, data formats, Linux
features, tests) and `docs/reference/reference-engine-inventory.md`
(capture, encode, ring, save, audio). Those two files are part of this
design; a builder reads them before planning any area.

### 5.1 Main window

Frameless window, 1100x720 default, opens maximised, geometry persisted.
36 px title strip (logo, "FTHR Clips", Ko-fi button, minimise, maximise,
close); 64 px capture toolbar (Record button, capture-settings summary
dropdown, Source dropdown, Game detection dropdown, Hotkeys dropdown,
status text on the right); 94 px navigation rail (Clips, Settings, up
to three parked editor sessions); page stack; 36 px error bar at the
bottom. Compositor-driven move and resize with an 8 px resize border.
Single instance: a second launch activates the first.

### 5.2 Clips page

Filter bar (category filter, sort), per-day sections of 16:9 clip cards
(300 to 440 px wide, virtualised past 96 cards), card hover state,
inline rename (double-click, F2), share and copy-link, confirmed delete,
drag to share window, thumbnails cached under `~/.fthr/thumbnails`,
library index with linked-folder protection, coalesced filesystem
watching of the clips root and game folders, smooth scrolling with the
reference's exponential easing.

### 5.3 Settings page

Seven tabs with the reference's exact content and order: General, Clip,
Audio, Visuals, Customize (six accordions), Performance, Shortcuts.
Every control maps to a key in `settings.json` or `theme.json`. Presets
save, load and delete full capture snapshots. Export presets have
built-ins plus user-defined entries. Shortcuts shows both global hotkeys
(portal) and application-local navigation bindings with the conflict
rules of the reference. Customize edits the theme (palette, fonts,
icons, sounds) with the version-4 palette migration rules.

### 5.4 Capture

Rolling replay buffer up to 5 minutes, up to 240 fps, Low, Medium, High
or custom bitrate, H.264, HEVC or AV1, output and scaling selection,
cursor included. Save-clip writes the last N seconds as MP4 with system
audio and, if enabled, a microphone track. Manual start/stop recording
writes fragmented MP4 continuously so a crash keeps what was recorded.
Screenshots via the Screenshot portal, saved to `<root>/Screenshots`,
with the screenshot editor. Capture-card notification appears over
other windows when a clip is saved, with the reference's slide-in
timings and sounds.

### 5.5 Hotkeys

Global: save clip, start/stop recording, screenshot, dismiss
notification, bound through the XDG GlobalShortcuts portal. Local:
the navigation binding set from the reference (section 4.1 of the UI
inventory), configurable, with the same conflict rules.

Compositor behaviour differs. The reference already handles this with
two delivery paths and FTHRust keeps both (UI inventory section 6.1):

- Portal path (Plasma and any desktop whose portal implements
  GlobalShortcuts): the stored combo is sent as the preferred trigger,
  the compositor's answer is adopted back into `hotkeys.json`, the
  "DESKTOP KEPT ITS SHORTCUT" notice and the `EDIT IN DESKTOP SETTINGS`
  button behave as in the reference, cleared actions are remembered in
  `hotkeys-portal-cleared.json`.
- Socket path (always on, and the only path on Hyprland): a private
  Unix socket at `$XDG_RUNTIME_DIR/fthrust/hotkey.sock` receives the
  action name; on Hyprland FTHRust writes the managed file
  `~/.config/hypr/fthr-hotkeys.conf` with `bind = MODS, KEY, exec,
  <client command>` lines, appends `source = …/fthr-hotkeys.conf` to
  `hyprland.conf` once, honours the Lua-config variant, and runs
  `hyprctl reload`, exactly as the reference does. The client command
  is `fthrust --hotkey <action>` instead of `echo | nc -U`, removing
  the runtime dependency on `nc`.

In addition, each global hotkey row in the Shortcuts tab gets a
"copy config line" button on Hyprland that places that row's bind line
on the clipboard, for users who manage their config by hand. The
action ids are the reference's: `save_clip`, `save_screenshot`,
`start_recording`, `stop_recording`, `confirm_game_detection`,
`dismiss_game_detection`, `save_custom_clip_<N>`.

### 5.6 Game detection and focus

Detect a fullscreen game window, offer to switch capture focus to it,
name clips after it, per-game folders, blacklist rules, per-game crop
profiles. Focus pause and resume of capture.

### 5.7 Editor

Persistent editor window (960x600 minimum) with up to three parked
sessions. Video surface with playback, 46 px control bar, 116 px trim
timeline with filmstrip, segments, split, delete segment, undo and
redo, zoom, minimap. Sidebar panels: Clip speed (rate with pitch
preserve), Edit tools (crop, quick crop, stretch, reset), Audio mix
(master plus per-track, from the audio manifest), Color and effects,
Clip details, File details, Delete. Share clip, export preset selector,
Export with a jobs list, transactional export to `<root>/Exported`.
Drafts persist in `clip_metadata.sqlite3`.

### 5.8 Overlays and post-processing

Watermark overlay, webcam picture-in-picture with the camera overlay
editor, Gary mode overlay. These composite into the saved or exported
clip the way the reference does.

### 5.9 Upload

Optional uploader, enabled only after the user accepts terms in the
two-checkbox dialog. Providers: Catbox, SFTP (password, fingerprint pin
or known_hosts), custom server (multipart plus auth header). Upload
history keyed by local path. Copy link after upload. Upload settings
widget as in the reference.

### 5.10 Diagnostics and errors

The 23 public error codes (`Error 001` to `Error 023`) with the same
titles, recovery behaviour and keyed incidents in the error bar.
Diagnostics sessions under `~/.fthr/diagnostics` with `events.jsonl`,
the privacy-redacted diagnostic report export, A/V sync diagnostics,
field diagnostics, crash log, rotating `fthr.log` and `engine.log`.

## 6. Architecture (decided)

### 6.1 Two processes, one workspace

FTHRust runs as two processes:

- `fthrust` — the egui application. Owns the window, library, editor,
  settings, hotkeys, portals, uploader launching, diagnostics.
- `fthrust-engine` — the capture daemon. Owns Wayland capture, audio
  capture, encoding, the replay ring, saving, manual recording.

Reason: capture touches GPU drivers and compositor protocols that can
crash or hang; a separate process keeps the window alive, lets the UI
restart capture with a new generation, and matches the operational
model users and docs already have. The UI launches the engine, as today.

The IPC is new and private to this repository. The reference's
byte-exact 4272-byte shared-memory struct is not carried over, because
both sides are now Rust and nothing else reads it. The engine listens
on a Unix domain socket under `$XDG_RUNTIME_DIR/fthrust/engine.sock`;
messages are length-prefixed `serde` structs (the `fthrust-ipc` crate
defines them). Commands: Configure, SaveClip, StartRecording,
StopRecording, Screenshot-free (screenshots are UI side), SetFocus,
Pause, Resume, Shutdown, GetStatus. The engine pushes Status, SaveResult
and Incident events; no polling. The 500 ms status poll of the
reference becomes event-driven with a 500 ms heartbeat for liveness.

Startup handshake, generation counters, save state machine timeouts
(1 s acknowledgement, 60 s completion with a non-verdict "slow" notice)
and the health states (INITIALIZING, HEALTHY, DEGRADED, CONTENT_SUSPECT,
STALLED, FAILED, RECOVERING, STOPPED, focus-paused) keep the reference's
semantics and status strings exactly.

### 6.2 Workspace crates

```
Cargo.toml                 workspace
crates/
  fthrust-core             settings, presets, theme, hotkey/binding model,
                           library index, clip metadata (sqlite), error
                           codes, diagnostics, paths, migrations
  fthrust-ipc              engine <-> UI message types and framing
  fthrust-media            FFmpeg wrappers: decode, encode (VAAPI, NVENC,
                           software), mux, replay-interval selection,
                           audio timing, playback mixer, thumbnails,
                           ffprobe-equivalent metadata
  fthrust-wayland          wl registry, output tracking, wlr-screencopy,
                           ext-image-copy-capture, foreign-toplevel-list,
                           dmabuf and shm buffer pools, layer-shell surface
  fthrust-portal           ashpd wrappers: ScreenCast + PipeWire stream,
                           Screenshot, GlobalShortcuts, restore tokens
  fthrust-engine           the capture daemon binary
  fthrust-ui               the egui application binary (`fthrust`)
  fthrust-uploader         the consent-gated uploader binary
assets/                    fonts, icons, sounds, images (verbatim copies)
packaging/                 AppDir skeleton, desktop file, AppImage script
tools/                     release gates (licences, glibc, version, assets)
docs/                      this design, reference inventories, screenshots
```

### 6.3 Capture engine

Backend candidates in order, selectable by override: ext-image-copy-
capture (preferred), wlr-screencopy, ScreenCast portal over PipeWire.
Recovery policy as in the reference: three attempts with 250, 750 and
1500 ms backoff, transient conditions wait 2 s without consuming
budget, encoder failure after max(5, fps/2) consecutive misses, luma
black-screen watchdog. Recovery may fall through to the next backend
after exhausting the last good one (a fix; the reference only retries
the same backend).

Frames arrive as DMA-BUF when the compositor offers it and the encoder
can import it (VAAPI via `AV_PIX_FMT_DRM_PRIME` and `av_hwframe_map`),
otherwise as wl_shm or PipeWire memfd and are uploaded. Both paths
normalise to the encoder's format with correct handling of 10-bit,
y-invert and output transforms. Frame pacing re-anchors on throttle as
in the reference; PTS derive from `CLOCK_MONOTONIC`. GOP 2 s with forced
IDR. Encoder candidate policy: Auto prefers hardware (VAAPI, NVENC),
software only when chosen. BT.709 studio range.

Encoded ring: time-bounded at buffer length plus 5 s, optional byte
budget, drops whole GOPs. Save-clip selects the keyframe before the
window using the ported `replay_interval` logic, muxes to
`<path>.partial`, fsyncs, renames with no-replace, fsyncs the
directory; audio is AAC 128 kbps from float32 PCM. Saving runs on its
own thread so commands keep flowing (a fix).

Audio: PipeWire capture streams on the default sink monitor and the
chosen microphone, 48 kHz float32 stereo, SPA timestamps, latency
correction, gaps over 50 ms filled with silence, default-sink follower,
1 s reconnect loop. Device listing returns the same JSON shape as the
reference's `--list-playback-devices`.

Manual recording writes fragmented MP4 continuously.

### 6.4 UI

egui 0.37 on eframe with the wgpu backend, winit on Wayland. A single
`Tokens` struct holds the 25 colour tokens, font faces and sizes, radii
and motion timings from `docs/reference/reference-ui-inventory.md`
section 2, and a `Theme` loaded from `theme.json` overrides them.
Custom widgets reproduce the Qt ones: toggle switch, dropdown button,
number spin box, outline and filled buttons, settings accordion, clip
card, trim slider, filmstrip, minimap, error bar, title strip. SVG
icons render through egui_extras at the device pixel ratio with the
reference's tint rules. Oswald is loaded from `assets/fonts`.

Fidelity verification: `egui_kittest` snapshot tests render every
screen in the visual spec at the same window sizes as the Qt
screenshots, at device pixel ratio 1, and a comparison tool checks
them against `docs/reference/qt-screens/`. The Qt PNGs are the golden
images. A screen passes when, for every widget listed in its
acceptance checklist: the bounding box matches the Qt box within 1 px
on each edge; colours sampled at the checklist's fixed points (fills,
borders, text colour, accent) are exactly the token value; glyph
height and baseline of each text run are within 1 px; icons are the
same asset at the same rendered size. A whole-image perceptual diff
(SSIM) is reported for information but is not the gate, because glyph
rasterisation differs between Qt and egui. The three customize renders
at 1920, 2560 and 3840 pin responsive layout widths and are checked
the same way. Higher device pixel ratios are verified manually on this
machine, not by snapshot.

Playback: a Rust pipeline replaces Qt Multimedia and the C++ mixer.
`fthrust-media` decodes with FFmpeg on a worker thread into a bounded
frame queue; frames upload to a wgpu texture registered with egui (no
`ColorImage` path). Audio tracks from the clip's `.fthr-audio.json`
manifest are decoded, mixed with per-track gains, rate-changed with
pitch preservation through libavfilter `atempo`, and played through a
PipeWire playback stream which is the master clock. Video follows the
audio clock with the reference's plus or minus 120 ms resync rule.
Seek keeps the reference's proxy-transcode cache under
`~/.fthr/playback` for clips that seek slowly.

Out-of-window surfaces: the capture-card notification and Gary overlay
are layer-shell surfaces drawn by egui over wgpu using
`fthrust-wayland`, not eframe viewports, because winit cannot place or
pin windows on Wayland. On a compositor without layer-shell the capture
card falls back to a desktop notification and Gary overlay is disabled
with a logged notice.

Webcam: V4L2 through `nokhwa`. Screenshots: Screenshot portal, never a
window grab (fixes the black screenshots of the reference).

Game detection on Wayland: `ext-foreign-toplevel-list` gives app-id and
title of every toplevel; fullscreen state comes from the compositor's
toplevel state where exposed. Rules match on title and app-id (the
reference's `exe_path`, `folder_path` and `exe_name` fields are kept in
the rule format but only `title_contains` and a new `app_id` field are
evaluated on Wayland). Compositors without the protocol get "Game
detection unavailable" in the dropdown.

### 6.5 Uploader

`fthrust-uploader` is a separate binary with the reference's contract:
one JSON request on stdin, one JSON line on stdout, actions `upload`,
`test_connection`, `sftp_save`, `sftp_forget`, clip path must be under
the allowed roots. It ships inside the AppImage next to the main binary
instead of as a hash-sealed plugin download; the consent dialog gates
enabling it, and the main app never links upload code. Secrets go to
the Secret Service through `keyring-core`, fixing the reference's
Windows-only credential store. SFTP uses `russh`, HTTP uses `reqwest`
with rustls.

### 6.6 Packaging and CI

`packaging/build-appimage.sh` builds release binaries in an Ubuntu
22.04 container, fetches the pinned BtbN LGPL FFmpeg build (sha256
checked, same manifest format as the reference), assembles the AppDir
with `$ORIGIN` rpaths, runs `linuxdeploy` and the pinned `appimagetool`,
and writes the `.sha256`. GitHub Actions: fmt, clippy with warnings
denied, `cargo test` for every crate, `cargo-deny` for licences and
advisories, the AppImage job, a glibc symbol gate that inspects the
extracted AppDir (fixes the reference gate that scanned the wrong
file), version consistency (Cargo workspace version vs tag), and the
asset manifest hash gate.

## 7. On-disk compatibility contract

Frozen paths and formats, documented in full in the UI inventory
section 4:

- `~/.fthr/settings.json` every key, default, one-level merge, atomic
  replace, `.corrupt` quarantine, retired-key removal, migrations.
- `~/.fthr/presets.json`, `export_presets.json`,
  `encoder_capabilities.json`, `hotkeys.json`.
- `~/.fthr/theme/theme.json` with palette version 4 and the icons,
  sounds and fonts folders.
- `~/.fthr/clip_metadata.sqlite3` schema for tags, descriptions and
  editor drafts.
- `~/.fthr/thumbnails/<md5>.jpg` plus `.meta-v3` and `.failed`.
- Clips root layout `<root>/{Desktop,Recordings,Exported,Shared,
  Screenshots,<game>}` with `.fthr-audio.json` manifests and file
  naming `<prefix>_clip_from_%d%b%Y_%H-%M-%S.mp4` with `_2`, `_3`
  suffixes.
- `~/.fthr/uploader/settings.json` and `upload_history.json`.
- `~/.fthr/logs/*` and `~/.fthr/diagnostics/*`.
- `~/.fthr/playback` proxy cache.

The engine IPC and the engine's command line are not part of this
contract and change freely.

FTHRust is a replacement, not a side-by-side install. Running it at the
same time as the Python FTHR Clips is unsupported: both would write
`~/.fthr` and contend for the clips root. FTHRust uses its own
single-instance lock (`$XDG_RUNTIME_DIR/fthrust/instance.sock`), its
own desktop-file id (`fthrust.desktop`) and AppImage name, and on
first start it logs a warning if the Python app's lock or engine
shared-memory segment is present. Switching back to the Python app
after using FTHRust works because no frozen file changes format.

## 8. Testing strategy

- Every C++ test in the engine inventory section 8 and every
  Linux-relevant Python test in the UI inventory section 7 becomes a
  Rust test pinning the same behaviour, in the crate that owns it.
  Timing modules (`replay_interval`, `audio_timing`, recovery policy,
  save state machine, navigation bindings, settings migrations, theme
  migrations) are ported with their tests before anything that depends
  on them.
- UI snapshot tests via `egui_kittest` against the Qt references.
- Integration tests for the IPC (engine stub), the uploader contract,
  the transactional save, and MP4 output validated by probing the
  written file (duration, streams, keyframe placement).
- Hardware qualification is manual on this machine: Hyprland on the
  AMD RX 7900 for VAAPI and DMA-BUF, the RTX 2060 for NVENC, Plasma
  for the portal path, documented in `docs/qualification.md`.

## 9. Defects in the reference that FTHRust fixes

Found during inventory; each gets a test and a line in `CHANGES.md`.
Pointers are in the inventories.

Engine: save-clip blocks the command loop; PDEATHSIG set per thread;
`shm_open` without exclusivity; cursor missing on wlr and ext backends;
output transforms ignored; no DMA-BUF path (CPU copy ceiling at high
fps); `renameat2` EINVAL on filesystems without RENAME_NOREPLACE
support (fall back to link plus unlink); unbounded `pa_simple_read` on
shutdown; recovery never tries a different backend; Linux engine never
prints a startup error code, so failures always land on the generic
Error 004.

UI: screenshot hotkey grabs the window and produces black images on
Wayland; game detection never starts on Linux; uploader install root
and bundle hash are Windows-only so the Linux uploader cannot be
installed; "test connection" treats any HTTP error as success; KWin
portal picker triggers a false "CAPTURE STALLED"; error-bar dismiss
semantics differ by platform (Linux behaviour kept).

Packaging: glibc gate scans the AppImage file instead of its contents;
source package omits the Linux engine and build script; builder
rewrites a tracked manifest during CI.

## 10. Left to the builder

- Crate-level module layout inside each workspace crate.
- Exact IPC message schema beyond the command and event list above.
- Threading model inside the engine (one thread per capture, encode,
  audio source, save), as long as save never blocks commands.
- The DMA-BUF modifier negotiation and the per-vendor fallbacks.
- How the egui widgets are structured, as long as the token struct is
  the only source of colours, fonts and sizes.
- Order of implementation, phase boundaries and milestones.
- Whether thumbnails are generated in the UI process or a helper.

## 11. Decisions made by the designer, not the owner

Listed so the owner can veto any of them while reading:

1. Two processes over a Unix socket instead of the shared-memory
   struct.
2. The title strip and window title keep the string "FTHR Clips" for
   visual identity; "FTHRust" appears in the repo, binaries, About text
   and desktop file name.
3. Ko-fi button stays, pointing where the reference points, toggleable
   as today.
4. Uploader ships inside the AppImage behind the consent dialog instead
   of the hash-sealed plugin download.
5. Hardware-ID service dropped.
6. `~/.fthr` kept as the config root rather than a new `~/.fthrust`.
7. Game detection rule format gains an `app_id` field; `exe_*` fields
   are preserved but ignored on Wayland.
8. Capture card and Gary overlay use layer-shell, with desktop
   notification fallback.
9. Repository is private at creation.
10. Fidelity gate is per-widget boxes, sampled colours, glyph metrics
    and icon identity within 1 px, not a whole-image pixel diff.
11. Missing reference screens are captured from the Qt app before they
    are built, never reconstructed from prose.
12. Side-by-side running with the Python app is unsupported and
    warned about, not engineered for.
13. Hyprland hotkeys keep the reference's managed-config and socket
    approach (the only file FTHRust writes outside `~/.fthr`), with a
    copy-line helper added; the socket client is the app binary, not
    `nc`.
