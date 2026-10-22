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
| 4 | plan-4-ui-shell | `fthrust-ui`: tokens, theme application, fonts, icons, custom widgets, frameless window, title strip, toolbar, nav rail, error bar, all seven Settings tabs, popups, dialogs, snapshot harness against the Qt PNGs. | 1 |
| 5 | plan-5-ui-library-capture | Clips page, cards, thumbnails, library watching, engine launch and status, save flow, capture card (layer-shell), screenshots portal, GlobalShortcuts portal, game detection, single instance, diagnostics UI. | 3, 4 |
| 6 | plan-6-editor | Editor window, playback pipeline (decode to wgpu, PipeWire audio clock, mixer, atempo), trim timeline, sessions, export queue and presets, watermark, webcam PIP, Gary overlay, drafts. | 2, 4, 5 |
| 7 | plan-7-uploader-packaging | `fthrust-uploader` (Catbox, SFTP, custom), consent flow, upload settings UI wiring, AppImage build, CI gates, THIRD_PARTY_NOTICES, docs, qualification notes. | 5, 6 |

Rules that apply to every plan:

- Every task is TDD against values taken from the reference, through
  fixtures generated from the reference Python (plan 1) or the C++
  tests (plan 2 and 3), never from memory.
- Every defect listed in spec section 9 is fixed in the plan that owns
  the code and gets a line in `CHANGES.md`.
- No crate may depend on Qt, Python or the C++ engine.
