# FTHRust roadmap

Spec: `docs/superpowers/specs/2026-10-22-fthrust-design.md`

The spec covers seven subsystems. Each gets its own plan that produces
working, tested software on its own. Plans are written one at a time,
each after the previous one has shipped, because later plans depend on
the exact Rust interfaces the earlier ones end up with.

| # | Plan | Delivers | Depends on |
|---|------|----------|------------|
| 1 (shipped 2026-10-22) | `2026-10-22-fthrust-plan-1-core.md` | Repo bootstrap, reference fixtures, `fthrust-core`: every on-disk format, migrations, error codes, state machines, library index, metadata DB. No UI, no hardware. | none |
| 2 (shipped 2026-10-22) | `2026-10-22-fthrust-plan-2-media.md` | `fthrust-media`: FFmpeg bindings, replay-interval selection, audio timing, encoder candidates (VAAPI, NVENC, software), encoded GOP ring, MP4 mux with pre-roll, transactional save, fragmented MP4 recorder, thumbnails, probe and media-metadata parsing, `encoder_capabilities.json` cache, combined-audio mix policy. Tested with synthetic frames. | 1 |
| 3 (shipped 2026-10-22; Hyprland qualified, Plasma/portal picker/DMA-BUF pending, see plan 3b) | `2026-10-22-fthrust-plan-3-engine.md` | `fthrust-ipc`, `fthrust-wayland` (registry, outputs, wlr-screencopy, ext-image-copy-capture, foreign-toplevel, dmabuf/shm pools), `fthrust-portal` (ScreenCast + PipeWire), PipeWire audio capture, recovery policy, health, the `fthrust-engine` daemon. Qualified on Hyprland and Plasma on this machine. | 1, 2 |
| 4 (shipped 2026-10-23; fidelity gate `cargo test -p fthrust-ui --test fidelity_all`, 20 screens) | `2026-10-22-fthrust-plan-4-ui-shell.md` | `fthrust-ui`: tokens, theme application, fonts, icons, custom widgets, frameless window, title strip, toolbar, nav rail, error bar, all seven Settings tabs, popups, dialogs, snapshot harness against the Qt PNGs. | 1 |
| 5 (shipped 2026-10-23; gate `cargo test --workspace`, fidelity 25 screens, live overlay and toplevel tests on Hyprland) | `2026-10-23-fthrust-plan-5-ui-library-capture.md` | Clips page, cards, thumbnails, library watching, engine launch and status, save flow, capture card (layer-shell), screenshots portal, GlobalShortcuts portal, game detection, single instance, diagnostics UI. | 3, 4 |
| 6a (shipped 2026-10-23; gate `cargo test -p fthrust-media -p fthrust-ui`, live PipeWire test on this machine) | `2026-10-23-fthrust-plan-6a-playback.md` | Playback pipeline: per-track mixer with atempo rate change, video decoder with keyframe-aware seek, seek-proxy cache, playback service (audio clock, hold/resume/reseek sync, manifest tracks), wgpu video texture (`docs/playback.md`). | 2, 4 |
| 6b (shipped 2026-10-23; gate `cargo test --workspace`, fidelity incl. 6 editor screens, bench `editor.sh`) | `2026-10-23-fthrust-plan-6b-editor.md` | Editor: the editor layer and control bar, trim timeline and segments, undo, drafts, sidebar panels, crop and stretch dialogs, sessions in the rail, clip navigation, keyboard shortcuts (`docs/editor.md`). | 5, 6a |
| 6c | plan-6c-export | Export queue and presets, watermark, webcam PIP, Gary overlay, export progress and the library hand-off; the live crop/stretch/colour preview on the editor's video surface; the editor's export-in-progress checks. | 6b |
| 7 | plan-7-uploader-packaging | `fthrust-uploader` (Catbox, SFTP, custom), consent flow, upload settings UI wiring, AppImage build, CI gates, THIRD_PARTY_NOTICES, docs, qualification notes. | 5, 6c |

Rules that apply to every plan:

- Every task is TDD against values taken from the reference, through
  fixtures generated from the reference Python (plan 1) or the C++
  tests (plan 2 and 3), never from memory.
- Every defect listed in spec section 9 is fixed in the plan that owns
  the code and gets a line in `CHANGES.md`.
- No crate may depend on Qt, Python or the C++ engine.
