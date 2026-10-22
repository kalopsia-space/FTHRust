# FTHRust Plan 4: UI Shell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `fthrust` application shell in egui: tokens and theme, fonts, icons, the custom widget set, the frameless window with title strip, capture toolbar, navigation rail, page stack and error bar, the four top-bar popups, all seven Settings tabs and the generic dialogs, each verified against the Qt screenshots by a per-widget fidelity gate.

**Architecture:** One crate, `fthrust-ui` (library plus the `fthrust` binary). A `Tokens` struct is the only source of colours, fonts, sizes and timings; a `Theme` loaded through `fthrust-core` overrides it. Every custom widget draws itself with the painter from token values and registers a named fidelity marker (its rect and sampled colour points) under the Qt object name it reproduces. A Python tool extends the reference's own offscreen renderer to dump the same markers from Qt (`docs/reference/qt-screens/<screen>.json`); the egui_kittest snapshot tests render each screen at the Qt size and compare marker by marker. Pages are pure functions of the app state (`settings`, `theme`, engine status) so the harness renders them without an engine.

**Tech Stack:** egui 0.37, eframe 0.37 (`wgpu`, `wayland`, `default_fonts`, `persistence` off), egui_extras 0.37 (`svg`), egui_kittest 0.37 (`wgpu`, `snapshot`), image 0.25, `fthrust-core` (settings, theme, navigation, window geometry, error codes, presets, export presets, hotkeys), `fthrust-ipc` (status types for the toolbar), PySide6 6.12 in `.venv-reference` for the reference extractor.

**Spec:** `docs/superpowers/specs/2026-10-22-fthrust-design.md` (sections 5.1, 5.3, 6.4, 7, 8, 9 "UI", 11 items 2, 3, 10, 11). Reference material: `docs/reference/reference-ui-inventory.md` sections 1.1, 1.2, 1.5, 1.8, 1.9, 2, 3 (every value below is taken from there; the section pointer is the authority when this plan abbreviates), `docs/reference/crate-research-2026-10-22.md` section 4, screenshots in `docs/reference/qt-screens/`.

## Global Constraints

- "egui 0.37 on eframe with the wgpu backend, winit on Wayland." `eframe` features `wgpu`, `wayland`, `default_fonts`; no `x11` feature.
- "A single `Tokens` struct holds the 25 colour tokens, font faces and sizes, radii and motion timings … and a `Theme` loaded from `theme.json` overrides them." No colour, size or font literal outside `tokens.rs` and `theme.rs`.
- "Oswald is loaded from `assets/fonts`." Assets are verbatim copies of `reference/FTHR_UI/assets/{fonts,icons,sounds,fthr_logo.png,favicon.ico,gary.png,gatze.png}` at `assets/` (section 4 of the spec: "copied from the reference unchanged").
- "The title strip and window title keep the string "FTHR Clips"" (spec 11.2); the desktop-file id is `fthrust.desktop` (spec 7).
- Fidelity gate (spec 6.4, 11.10): for every widget in a screen's acceptance JSON, the egui bounding box matches the Qt box within 1 px on each edge; colours sampled at the JSON's fixed points equal the token value exactly; glyph height and baseline of each text run within 1 px; icons are the same asset at the same rendered size. Renders at the Qt sizes (1440×900 for pages, 460×363 for the capture popup, 1920×1080, 2560×1440, 3840×2160 for Customize) at pixels-per-point 1. SSIM is reported, not gated.
- "No screen is built from the prose alone" (spec 4): a screen without a PNG and JSON is captured from the reference first with `tools/render-qt-reference.py` and committed.
- Motion timings are the reference's: hover 120 ms InOutSine, page slide 220 ms InOutSine, popup fade 180 ms OutCubic but skipped on Wayland, smooth scroll `pos += (target − pos)·(1 − e^(−dt/35 ms))` stop at |Δ| < 0.5; `reduce_ui_animations` jumps every animation to its end.
- `#![forbid(unsafe_code)]` in `fthrust-ui`. Gate for every task: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check`. Snapshot tests run in CI on lavapipe (`mesa-vulkan-drivers`); a missing adapter is a test failure, not a skip.
- Commit after every task. Branch `plan-4-ui-shell` from `main`.

## Review Focus

1. A theme with a corrupt or partial `theme.json` (missing tokens, bad colour strings): the app starts with the default palette for the missing values and never a black or unstyled screen. Test in Task 3.
2. A 4K display at pixels-per-point 2 (the three Customize widths show the layout, not the scale): text and icons must not blur or shift against the 1× layout; icons rasterise at the device ratio. Test in Task 5 (icon raster size) and Task 6 (layout equality across ppp).
3. The window restored from a saved geometry that no longer fits a monitor (monitor removed): the window is clamped onto a visible screen and the maximised flag is honoured. Test in Task 9.
4. A settings value outside the UI's option list (an fps of 75 in `settings.json`, a clip length not in the preset list): the control shows the stored value without rewriting it until the user changes it, and the summary text still formats. Test in Task 11.
5. Keyboard chord recording in Shortcuts with a conflicting combo: the conflict message from `fthrust_core::navigation` is shown and the old binding stays. Test in Task 14.

---

## File structure

```
assets/                       fonts/, icons/, sounds/, fthr_logo.png, favicon.ico, gary.png, gatze.png (verbatim)
tools/render-qt-reference.py  renders + extracts acceptance JSON from the reference (PySide6)
tools/fidelity-report.py      optional human report (SSIM + marker table) from a harness dump
docs/reference/qt-screens/    <screen>.png + <screen>.json
crates/fthrust-ui/src/
  lib.rs                      #![forbid(unsafe_code)], module list, App
  tokens.rs                   Tokens (colours, typography, geometry, motion), Colors enum
  theme.rs                    Theme -> Tokens overrides, font files, icon overrides and tints
  fonts.rs                    Oswald families, FontId helpers, text metrics
  icons.rs                    Icon names, ICON_ASSETS map, tinted SVG rasterisation, logo inversion
  fidelity.rs                 Marker registry, sample points, screen definitions
  motion.rs                   Easing, Animation (duration + reduce-motion), SmoothScroll
  widgets/mod.rs              re-exports
  widgets/buttons.rs          primary/outline/secondary/pill/ghost/icon buttons, TopBarButton
  widgets/toggle.rs           ToggleSwitch
  widgets/check.rs            Checkbox, Radio
  widgets/input.rs            LineEdit, NumberSpinBox
  widgets/slider.rs           Slider (settings + editor variants)
  widgets/combo.rs            Combo (button + popup list)
  widgets/accordion.rs        AccordionSection
  widgets/popup.rs            PopupPanel (anchored, clamped, fade)
  widgets/dialog.rs           FthrDialog frame, MessageDialog, InputDialog
  widgets/scroll.rs           styled ScrollArea + smooth wheel
  widgets/section.rs          flat section header, hsep, label recipes
  shell/mod.rs                Shell (title strip + toolbar + nav rail + page stack + error bar)
  shell/title_strip.rs        logo, brand, Ko-fi, window buttons, drag/double-click
  shell/toolbar.rs            Record, capture summary, Source, Game detection, Hotkeys, status label
  shell/nav_rail.rs           Clips / Settings / editor session buttons
  shell/error_bar.rs          ErrorBar queue, actions, dismiss
  shell/window.rs             frameless resize edges, geometry persistence
  popups/capture_settings.rs  CaptureSettingsPopup
  popups/source.rs            SourcePopup
  popups/game_detection.rs    GameDetectionPopup
  popups/hotkeys.rs           HotkeyPopup
  settings/mod.rs             tab bar, sliding stack, category keyboard
  settings/general.rs … clip.rs, audio.rs, visuals.rs, customize.rs, performance.rs, shortcuts.rs
  dialogs/custom_clip_length.rs, export_preset.rs, diagnostic_report.rs
  state.rs                    AppState (settings store, theme, status, popups, dialogs), test fixtures
  main.rs                     binary: handle_probe_argv, eframe::run_native
crates/fthrust-ui/tests/      fidelity_*.rs per screen, widgets.rs, theme.rs, motion.rs, window.rs
```

---

### Task 1: Crate scaffold, assets, CI

**Files:** `crates/fthrust-ui/{Cargo.toml,src/lib.rs,src/main.rs}`, `assets/**` (copied), `Cargo.toml` (workspace deps: eframe, egui, egui_extras, egui_kittest, image already), `.github/workflows/ci.yml` (apt `mesa-vulkan-drivers libxkbcommon-dev libgtk-3-dev` is not needed; add `mesa-vulkan-drivers` and `WGPU_BACKEND=vulkan`), `deny.toml` as needed, `tools/check-assets.sh` (sha256 list).

**Interfaces:**
- Produces: `fthrust_ui::VERSION`, binary `fthrust --version` → `fthrust <VERSION>`; `assets/manifest.sha256` listing every asset with its reference hash; `tools/check-assets.sh` exits non-zero when an asset differs from `reference/FTHR_UI/assets`.

- [ ] Test `tests/smoke.rs`: `binary_prints_version`; `assets_match_reference` (reads `assets/manifest.sha256`, hashes each file, compares; skips with a message when `reference/` is absent, which is the CI case).
- [ ] RED → copy assets (`fonts/Oswald-{Regular,SemiBold,Bold}.ttf`, every `icons/*.svg` and `*.png`, `sounds/*.wav`, `fthr_logo.png`, `favicon.ico`, `gary.png`, `gatze.png`), write the manifest, scaffold, GREEN → commit `chore(ui): scaffold fthrust-ui and copy reference assets`.

---

### Task 2: Qt reference extractor and acceptance JSON

**Files:** `tools/render-qt-reference.py` (starts as a copy of `reference/tools/render_ui_preview.py`), `docs/reference/qt-screens/*.json`, new PNGs for screens the plan builds that have none.

**Interfaces:**
- Produces, per screen, `<screen>.json`:
  ```json
  {"screen": "settings", "size": [1440, 900], "pixels_per_point": 1,
   "widgets": [{"name": "settingsTabBar", "rect": [0, 100, 1440, 68],
                "samples": [{"role": "fill", "at": [720, 110], "rgb": "#101512"}],
                "text": {"content": "General", "px": 14, "weight": 400, "rect": [..], "baseline": 162}}]}
  ```
  Widget names are Qt object names (`topBar`, `winBtn`, `closeBtn`, `captureToolbarScroll`, `navigationPanel`, `homeBtn`, `settingsGearBtn`, `settingsTabBar`, `settingsTabBtn[General]`, `statusLabel`, …); unnamed widgets get `<class>[<text>]`. `samples` are taken from the rendered PNG at: fill = rect centre, border = (x, y + h/2) one pixel inside the left edge, text = the darkest pixel of the text rect, accent = a point the extractor names per widget class (checked tab bottom border, toggle knob, slider handle). `text` carries `QFontMetrics` ascent/descent and the label's global rect.
- Screens to capture: every existing PNG (JSON added), plus `general.png`, `performance.png`, `source-popup.png`, `game-detection-popup.png`, `hotkey-popup.png`, `dialog-message.png`, `dialog-question.png`, `dialog-input.png`, `error-bar-error.png`, `error-bar-warning.png`, `custom-clip-length.png`, `export-preset.png` at their natural sizes.

- [ ] Test `tools/test_render_qt_reference.py` (pytest under `.venv-reference`): the JSON for `settings.png` contains `settingsTabBar` with rect `[*, *, 1440, *]`, seven `settingsTabBtn[...]` entries in order, and every sample's `rgb` is one of the 25 palette values or `#ffffff`-family text; `general.png` size is `[1440, 900]`.
- [ ] RED → implement the extractor (walk `window.findChildren(QWidget)`, keep visible widgets with an object name or a text, map to global coordinates with `mapTo(window, …)`, sample the PNG, dump), render the missing screens by driving the same widgets the reference's main window exposes (`cap_settings_popup`, `source_popup`, `game_detection_popup`, `hotkey_popup`, `error_bar.push(...)`, `FthrMessageDialog.question`, …), GREEN → commit the tool, PNGs and JSONs: `docs(reference): Qt acceptance geometry and missing screens`.

---

### Task 3: Tokens and theme application

**Files:** `src/tokens.rs`, `src/theme.rs`; tests `tests/theme.rs`.

**Interfaces:**
- Produces: `pub enum Color { Bg, Surface1, Surface2, Surface3, ShellBg, ShellBg2, ShellDivider, CardBg, CardBgHi, CardBorder, Hairline, Border, BorderHi, Text, TextDim, TextMuted, TextGhost, Accent, AccentDim, AccentSoft, Error, ErrorSoft, Warning, Success, Delete }` (25, in `theme_manager.py` order) with `pub fn key(self) -> &'static str` (`"BG"`, `"SURFACE_1"`, …); `pub struct Tokens { pub colors: [Color32; 25], pub capture_card: CaptureCardColors (7), pub display_family: String, pub body_family: String, pub icon_tint_global: Color32, pub icon_tints: BTreeMap<String, Color32>, pub icon_overrides: BTreeMap<String, PathBuf> }` with `pub fn default_palette() -> Tokens`, `pub fn color(&self, c: Color) -> Color32`, `pub fn bar_bg/bar_fg/bar_fg_dim/bar_hairline(&self)` (= SHELL_BG/TEXT/TEXT_DIM/SHELL_DIVIDER); `pub mod size { pub const MICRO: f32 = 12.0; LABEL 14; BODY 15; BODY_L 17; TOOLBAR 15; H3 20; H2 26; H1 36; }`; `pub mod geom { TITLE_BAR_H 36; CAPTURE_BAR_H 64; CAPTURE_CONTROL_H 44; FILTER_H 60; BUTTON_H 38; INPUT_H 38; HW_BAR_H 36; NAV_W 94; NAV_BTN 78; RADIUS 6; RADIUS_SM 4; RADIUS_CARD 8; PAGE_INSET 24; ROW_GAP 12; SECTION_GAP 24; RESIZE_MARGIN 8; SPACE: [f32; 8] = [4,8,12,16,20,24,32,48] }`; `pub mod motion { HOVER_MS 120; SLIDE_MS 220; POPUP_FADE_MS 180; CARD_IN_MS 450; CARD_HOLD_MS 2400; CARD_OUT_MS 350; SCROLL_TAU_MS 35 }`.
- `theme::apply(tokens: &mut Tokens, theme: &fthrust_core::theme::Theme)`: colours (missing/invalid keep defaults), capture-card colours, fonts (`display`/`body`), `icon_tints` (`_global` + per icon), `icons` overrides (path under the theme icons dir), and `theme::load(paths: &FthrPaths) -> Tokens` (defaults when the file is absent or corrupt, per core's loader).

- [ ] Tests: `default_palette_matches_inventory` (all 25 hex values from inventory 2.1 and the capture-card defaults), `theme_overrides_only_valid_values` (a `theme.json` with `ACCENT: "#ff0000"`, `TEXT: "nope"`, no `BG` → accent red, text default, bg default), `corrupt_theme_file_gives_defaults` (Review Focus 1), `font_roles_default_to_oswald`.
- [ ] RED → implement → GREEN → commit `feat(ui): tokens and theme application`.

---

### Task 4: Fonts and text metrics

**Files:** `src/fonts.rs`; tests `tests/fonts.rs`.

**Interfaces:**
- Produces: `pub const FAMILY_400: &str = "oswald-400"; FAMILY_600 = "oswald-600"; FAMILY_700 = "oswald-700"`; `pub fn install(ctx: &egui::Context, tokens: &Tokens, theme_font_files: &[(String, PathBuf)])` (registers the three Oswald files as named families, a theme `.ttf/.otf` as `theme-<family>` when the role names it, keeps egui's default proportional family as fallback, and sets `FontFamily::Proportional` to body 400); `pub fn id(px: f32, weight: Weight) -> FontId` with `pub enum Weight { W400, W600, W700 }`; `pub fn text(s: impl Into<String>, px: f32, weight: Weight, color: Color32) -> RichText`; `pub fn measure(ctx: &Context, s: &str, px: f32, weight: Weight) -> TextMetrics { width, height, ascent, descent }` from a laid-out galley.

- [ ] Tests (kittest, no render): `oswald_families_are_registered` (a galley in each family has a non-zero width and the three widths differ for "FTHR Clips"), `pixel_size_is_logical_height` (`measure("Hg", 17.0, W600).height` within 1 px of Qt's reported line height for `SIZE_BODY_L` from `typography.json`), `theme_font_file_registers` (a copied Oswald file under a fake theme dir maps a role).
- [ ] RED → implement → GREEN → commit `feat(ui): Oswald fonts and text metrics`.

---

### Task 5: Icons

**Files:** `src/icons.rs`; tests `tests/icons.rs`.

**Interfaces:**
- Produces: `pub enum Icon { Clip, Sound, Visuals, General, Updates, Personalize, Performance, Home, Refresh, Play, Pause, Close, Minimize, Maximize, Shutdown, Dropdown, Shortcuts, Upload, Link, Delete, Camera, Increase, Decrease, StepBack, StepForward, Kofi, Logo, Favicon }` with `pub fn logical_name(self) -> &'static str` (the legacy `*.png` key: `clip.png`, `settings(general).png`, …) and `pub fn asset(self) -> &'static str` (`clips.svg`, `settings.svg`, `minimise.svg`, `plus.svg`, `chevronleft.svg`, `copylink.svg`, `kofi_symbol.png`, `fthr_logo.png`, …) per inventory 3 `ICON_ASSETS`; `pub struct IconCache` with `pub fn image(&mut self, ctx: &Context, icon: Icon, size_px: f32, tokens: &Tokens) -> egui::Image` (SVG rasterised at `size_px * ctx.pixels_per_point()` with `egui_extras`' svg loader, tinted by SourceIn with the theme tint (per-icon else `_global`), never tinted for `Favicon`/`Logo`/custom theme files unless a per-icon tint exists; the logo is RGB-inverted for the dark bar as `_load_logo_asset` does); `pub fn sizes() -> [f32; 5] = [14, 16, 18, 28, 36]`.

- [ ] Tests: `asset_map_matches_inventory` (every `Icon` maps to an existing file under `assets/icons` or `assets/`), `tint_is_source_in` (rasterise `close.svg` at 14 with tint `#20e3a0`: every opaque pixel is exactly the tint, alpha preserved), `rasterises_at_device_ratio` (size 18 at ppp 2 yields a 36 px texture; Review Focus 2), `custom_theme_icon_is_not_tinted`.
- [ ] RED → implement → GREEN → commit `feat(ui): tinted SVG icons`.

---

### Task 6: Fidelity harness

**Files:** `src/fidelity.rs`, `tests/common/mod.rs` (harness helpers), `tests/fidelity_harness.rs`.

**Interfaces:**
- Produces: `fidelity::{Marker { name: String, rect: Rect, samples: Vec<Sample { role: &'static str, at: Pos2, expect: Color }>, text: Option<TextRun { content: String, px: f32, weight: Weight }> }, Registry (per-frame Vec<Marker> stored in `ctx.data`), mark(ui: &Ui, name, rect) -> MarkerBuilder (`.fill(at, Color)`, `.border(Color)`, `.text(content, px, weight)`), take(ctx) -> Vec<Marker>}`; `tests/common`: `render_screen(screen: Screen, size: Vec2, ppp: f32, state: AppState) -> Rendered { image: RgbaImage, markers: Vec<Marker> }` using `egui_kittest::Harness::builder().with_size(size).with_pixels_per_point(ppp).build_ui(...)`, `harness.run()`, `harness.render()`; `compare(rendered: &Rendered, qt_json: &Path, tokens: &Tokens) -> Report { failures: Vec<String>, ssim: f64 }` applying the gate (box ±1 px, sampled colour exact against the token the marker names, text px/weight equal and galley height within 1 px of Qt ascent+descent, icon asset+size equal); `assert_fidelity!(rendered, "settings")` macro that prints the report and fails on any failure; `FTHRUST_FIDELITY_DUMP=dir` writes the egui PNG and a markers JSON next to it for `tools/fidelity-report.py`.

- [ ] Tests: `marker_registry_collects_per_frame`, `compare_flags_box_off_by_two` (synthetic JSON vs a marker 2 px off → one failure naming the edge), `compare_accepts_within_one_px`, `compare_flags_wrong_token`, `layout_equal_across_ppp` (a 300×100 ui with a marked button renders the same marker rects at ppp 1 and 2; Review Focus 2), `renders_on_this_gpu` (harness render returns an image of the requested size; this is the CI adapter check).
- [ ] RED → implement → GREEN → commit `feat(ui): fidelity harness against the Qt acceptance geometry`.

---

### Task 7: Widgets I — buttons, toggle, checkbox/radio, line edit, spin box, slider

**Files:** `src/widgets/{mod,buttons,toggle,check,input,slider,section}.rs`; tests `tests/widgets.rs`.

**Interfaces:**
- Produces (all `fn ui(ui: &mut Ui, tokens: &Tokens, ...) -> Response`, each drawing from the recipe in inventory 2.4 and registering a marker named by the caller):
  - `buttons::{primary(ui, t, text, name), outline(..), secondary(..), pill(..), pill_ghost(..), ghost(..), icon_button(ui, t, icons, Icon, size_px, name) with IconButtonStyle { hover_bg: Option<Color> }, window_button(46×36), top_bar_button(ui, t, icons, text, open: bool, stretch_weight, name) (44 px tall, SURFACE_3, 1px BORDER, r6, 14 px dropdown arrow at right rotated 180° when open)}`; hover transitions through `motion::Hover` (120 ms InOutSine colour blend; disabled when `reduce_ui_animations`).
  - `toggle::switch(ui, t, on: &mut bool, name) -> Response` (38×20, track inset (1,2,w−2,h−4) r8, ACCENT_SOFT/SURFACE_1, border ACCENT when on or focused else BORDER_HI, 12 px knob at x=5 or w−17, ACCENT/TEXT_DIM; disabled SURFACE_1/BORDER/TEXT_MUTED).
  - `check::{checkbox(ui, t, on: &mut bool, label, name), radio(ui, t, selected: bool, label, name)}` (14×14 box, 1 px TEXT border, BG fill, r4 / r7, hover ACCENT border, checked ACCENT fill; label 17 px, spacing 10).
  - `input::{line_edit(ui, t, text: &mut String, name) (SURFACE_2, 1 px BORDER, r6, padding 6×10, 15 px, hover BORDER_HI, focus ACCENT), number_spin(ui, t, value: &mut i64, range, step, suffix, name) (34 px tall, two 28 px stepper icon buttons `Decrease`/`Increase` inside at right, letter spacing 0.8)}`.
  - `slider::{settings_slider(ui, t, value: &mut f32, range, name) (2 px BORDER groove, ACCENT sub-page, 12×12 ACCENT handle r4, hover/pressed TEXT, 26 px hit height, click jumps), editor_slider(.., 10×10 r6)}`.
  - `section::{flat_header(ui, t, title) (Oswald 600 20 px + 1 px SHELL_DIVIDER rule to its right), hsep(ui, t), label_uppercase(ui, t, text) (600, 14 px), label_body(..)}`.

- [ ] Tests (kittest render of a 400×300 canvas with one widget each, sampled through markers): `primary_button_colors_and_padding` (fill ACCENT, text BG, hover TEXT fill after 200 ms, pressed ACCENT_DIM, disabled BORDER/TEXT_MUTED), `outline_button_hover_accent`, `toggle_geometry_and_states` (knob x 5 → 21 on toggle, track colours), `checkbox_states`, `line_edit_focus_border`, `number_spin_steps_and_clamps` (500..200000 step 500: click increase from 199_900 → 200_000, decrease from 500 stays), `slider_click_jumps_and_handle_colors`, `top_bar_button_arrow_rotates_when_open`, `hover_transition_respects_reduce_motion`.
- [ ] RED → implement → GREEN → commit `feat(ui): core widget set`.

---

### Task 8: Widgets II — combo, accordion, popup panel, dialog frame, scroll, tooltip, context menu

**Files:** `src/widgets/{combo,accordion,popup,dialog,scroll}.rs`, `src/motion.rs`; tests `tests/widgets2.rs`, `tests/motion.rs`.

**Interfaces:**
- `combo::Combo { items: Vec<String>, selected: usize }` `fn ui(ui, t, icons, &mut self, name) -> Response` (SURFACE_3, 1 px BORDER, r6, padding 6×12, 15 px, min 90×22, 22 px drop zone, 14 px arrow at right−8; popup list items 26 px, padding-left 10, selected SURFACE_3 + ACCENT text; wheel ignored when closed); `combo::with_user_data(items: Vec<(String, T)>)`.
- `accordion::Section { number: &str, title: &str, open: &mut bool }` (44 px header, ACCENT number, 17 px title, dropdown arrow; body margins 16 spacing 8; bg SURFACE_3, 1 px BORDER, header hover BORDER_HI).
- `popup::Panel::anchored(ui, t, anchor: Rect, min_width: f32, name) -> Option<PanelUi>` (SURFACE_2, 1 px BORDER_HI, r6, min width 300, placed under the anchor by `fthrust_core::window_geometry::popup_rect_for_anchor`, clamped to the screen with 8 px margin, 180 ms OutCubic fade except on Wayland (`ctx.os()`/an explicit `tokens.wayland` flag), closes on outside click and emits `closed`).
- `dialog::{Frame::modal(ctx, t, title, min_width, name) (BG, 1 px BORDER_HI, r6, header 38 px SURFACE_2 with uppercase-style title 14 px and 30×30 close with ERROR hover; body margins (16,16,16,8) spacing 10; action row margins (16,8,16,14) spacing 8; header drag moves it), Message::information/warning (non-modal, OK) / question (modal, No outline focused, Yes primary) -> Option<bool>, Input::prompt(..) -> Option<String>}`.
- `scroll::{area(ui, t, name) (8 px scrollbars, BG track with 4 px vertical margin, BORDER_HI handle r4 min 32, hover ACCENT, no arrows), SmoothScroll { target, pos } fn tick(&mut self, dt: f32)` per inventory 2.5 (angle-delta wheels only, `−Δ/120 · singleStep · wheelScrollLines`, τ 35 ms, stop |Δ| < 0.5; pixel deltas native)}`.
- `motion::{Ease { InOutSine, OutCubic, InCubic, OutBack }, Animation::new(duration_ms, ease), progress(now) with reduce_motion → 1.0, Hover (120 ms colour blend)}`; tooltip style (BG, 1 px ACCENT, 15 px, padding 4×8) and context-menu style (SURFACE_1, 1 px BORDER_HI, items padding 8×22, selected SURFACE_3/ACCENT) installed on the `egui::Style` by `tokens::install_style(ctx, &Tokens)`.

- [ ] Tests: `combo_popup_items_and_selection`, `combo_ignores_wheel_when_closed`, `accordion_toggles_body`, `popup_clamps_to_screen_with_8px_margin` (anchor at the right edge), `popup_fades_only_off_wayland`, `dialog_question_returns_choice_and_focuses_no`, `smooth_scroll_converges_with_tau_35` (after 35 ms the gap shrinks to 1/e), `easings_match_qt` (InOutSine(0.25) = 0.1464, OutCubic(0.5) = 0.875, OutBack(1) = 1 within 1e-4), `reduce_motion_jumps_to_end`.
- [ ] RED → implement → GREEN → commit `feat(ui): combo, accordion, popups, dialogs, scrolling, motion`.

---

### Task 9: Frameless window, title strip, geometry

**Files:** `src/shell/{mod,window,title_strip}.rs`, `src/state.rs`, `src/main.rs`; tests `tests/window.rs`, `tests/fidelity_shell.rs`.

**Interfaces:**
- `state::AppState { settings: SettingsStore, paths: FthrPaths, tokens: Tokens, icons: IconCache, status: StatusText, page: Page { Clips, Settings }, settings_tab: Tab, popups: PopupState, dialogs: DialogQueue, error_bar: ErrorBarState, wayland: bool, reduce_motion: bool }` with `AppState::for_tests(settings_json: Value) -> AppState` (temp `~/.fthr`, default theme).
- `window::{resize_edge_at(pos: Pos2, size: Vec2, margin: f32 = 8) -> Option<ResizeDirection>, apply_frame_commands(ctx, &Shell) (StartDrag on title-strip press, BeginResize on edge press, double-click toggles Maximized, disabled while maximised/fullscreen), restore_geometry(ctx, &SettingsStore) / persist_geometry(..) through fthrust_core::window_geometry (default 1100×720, default maximised, clamp to a visible monitor: Review Focus 3)}`.
- `title_strip::ui(ui, &mut AppState) -> TitleStripResponse { close, minimise, maximise, kofi }`: 36 px, margins (16,0,0,0) spacing 12, logo 28×28 inverted, brand "FTHR Clips" Oswald 700 17 px, stretch, Ko-fi outline button "Help us keep the app free and open for everyone" with the 26×22 icon (hidden when `show_kofi_widget` is false; opens `https://ko-fi.com/fthrclips`), three 46×36 window buttons with 14 px icons; markers `topBar`, `winBtn[min]`, `winBtn[max]`, `closeBtn`, `brandLabel`, `kofiBtn`.
- `main.rs`: `fthrust_media::capabilities::handle_probe_argv()` first; `eframe::run_native("FTHR Clips", NativeOptions { viewport: ViewportBuilder::default().with_decorations(false).with_app_id("fthrust").with_inner_size(1100×720).with_min_inner_size(800×560), renderer: Wgpu, … })`; `--version`.

- [ ] Tests: `resize_edges_8px` (corners and edges map; inside → None), `geometry_restore_clamps_to_monitor` (saved rect off-screen → clamped; maximised flag honoured), `title_strip_markers_match_qt` (fidelity against `settings.json`'s `topBar`, `winBtn*`, `closeBtn`, brand text run), `kofi_hidden_by_setting`.
- [ ] RED → implement → GREEN → commit `feat(ui): frameless window and title strip`.

---

### Task 10: Toolbar, navigation rail, page stack, error bar

**Files:** `src/shell/{toolbar,nav_rail,error_bar}.rs`, `src/shell/mod.rs`; tests `tests/fidelity_shell.rs` (extended), `tests/error_bar.rs`.

**Interfaces:**
- `toolbar::ui(ui, &mut AppState) -> ToolbarResponse { open_popup: Option<PopupKind> }`: 64 px `captureToolbarScroll` (horizontal scroll only when narrow), inner margins (14,6,14,6) spacing 8; `record_btn` "Record" (enabled; plan 5 wires it), `cap_btn` summary text from `capture_summary(&SettingsStore) -> String` (`"30s · 60 FPS · SOURCE · HIGH"` rules of `_update_summary`: clip length label, fps, resolution label or `SOURCE`, quality upper-case or `CUSTOM`), `source_btn` "Source"/"DESKTOP"/"WINDOW", `game_detection_btn` summary, `hotkey_btn` "Hotkeys", status label `statusLabel` right-aligned min width 110 (text `capitalize()`d, style active ACCENT / idle TEXT_MUTED / warning ERROR per `StatusText { text, style }`); stretch weights 3/2/2/1.
- `nav_rail::ui(ui, &mut AppState) -> Option<Page>`: 94 px, margins (8,16,8,12) spacing 12, `homeBtn` "Clips" (`Home` 18 px) and `settingsGearBtn` "Settings" (`General` 18 px) 78×78 text-under-icon exclusive; up to three editor session buttons `editorSidebarButton[n]` ("Editor N\n<stem elided>", right-click "Close editor") rendered from `state.editor_sessions: Vec<SessionSummary>` (plan 6 fills it).
- `shell::page_stack(ui, &mut AppState)` with the 220 ms InOutSine vertical slide between Clips (a placeholder frame until plan 5) and Settings; `shell::ui(ctx, &mut AppState)` composes strip, toolbar, row (rail + stack with 1 px SHELL_DIVIDER top border), error bar.
- `error_bar::{ErrorBarState::push(title, detail, level: Level { Error, Warning }, actions: Vec<Action { label, id }>, key: Option<String>), remove_by_key, dismiss_current (Linux semantics: only the current), queued_count, ui(ui, t, &mut self) -> Option<ActionId>}` (36 px, SURFACE_1, 1 px top border in the level colour, margins (20,0,20,0) spacing 10, "Notice" in the level colour, bold title 15 px 600, "—", detail 15 px, stretch, "+N MORE" 14 px TEXT_DIM, up to two outline action buttons in the level colour padding 3×10, 28 px close); `push` only shows titles that `fthrust_core::error_codes` maps, formatted through `format_error_title`, and respects `error_notifications_enabled`.

- [ ] Tests: `toolbar_and_rail_markers_match_qt` (fidelity against `settings.json` and `library.json` for `captureToolbarScroll`, `topClusterMain`, the five buttons' rects and texts, `statusLabel`, `navigationPanel`, `homeBtn`, `settingsGearBtn`), `capture_summary_text_rules` (`30s · 60 FPS · SOURCE · HIGH`, custom bitrate → `CUSTOM`, 1080p → `1080P`, 90 s → `1m 30s`), `status_label_capitalizes_and_styles`, `page_slide_takes_220ms_and_reduce_motion_jumps`, `error_bar_queue_and_dismiss_semantics` (two pushes → "+1 MORE"; dismiss shows the next; keyed push replaces; `remove_by_key`; unknown title ignored; `error_notifications_enabled=false` suppresses), `error_bar_markers_match_qt` (against `error-bar-error.json` and `error-bar-warning.json`).
- [ ] RED → implement → GREEN → commit `feat(ui): toolbar, navigation rail, page stack, error bar`.

---

### Task 11: Top-bar popups

**Files:** `src/popups/{capture_settings,source,game_detection,hotkeys}.rs`, `src/popups/mod.rs`; tests `tests/fidelity_popups.rs`, `tests/popups.rs`.

**Interfaces:**
- Each popup is `fn ui(panel: &mut PanelUi, &mut AppState) -> PopupResult { restart_needed: bool, changed_keys: Vec<&'static str> }`, editing `state.settings` through core's store (keys per inventory 1.2 and 4.1) and formatting labels exactly as the reference.
- `capture_settings`: min width 460, margins (20,16,20,16) spacing 12; rows label (`label_uppercase` TEXT 12 px, width 92) + control; profile buttons "Clips"/"Recording"; "Clip length" combo (`CLIP_LENGTHS` from core + custom lengths, labels `5s 10s 15s 30s 45s 1m 1m 30s 2m 3m 4m 5m`; a stored value outside the list is shown as its own label and kept until changed: Review Focus 4), "Add custom clip length…" outline → `dialogs::custom_clip_length`, custom lengths list with remove, hint "Set a separate shortcut for each length in Hotkeys.", memory warning when ≥ 120 s, "Framerate" (`FPS_VALUES`), "Resolution" (480p 720p 1080p 1440p Source), "Quality" (Low Medium High Custom), "Custom bitrate" spin 500–200000 step 500 " kbps" min width 190; "Apply" primary shown when a change needs a restart.
- `source`: margins (16,14,16,14); "Capture source" label; mode combo ["Desktop", "Window / Game"]; window row (combo min width 240 + 28×28 refresh icon button with 1 px TEXT border); monitor combo ("First Screen (Default)" + `"{name}  ({w}×{h} @ {Hz}Hz)"` from `state.monitors: Vec<MonitorInfo>`, user data = name).
- `game_detection`: min width 420, margins (18,14,18,16); header + toggle; divider; "Switch mode" + "AUTO-SWITCH"/"PROMPT FIRST"; "Fall back to desktop" + toggle; divider; "Manual games": running-window combo, "Refresh windows"/"Add manual game"/"Block detection" outline buttons, status label, rules list rows (name, BROWSE/FOLDER/crop/remove placeholders; actions wired in plan 5).
- `hotkeys`: min width 550, margins (18,14,18,16); "HOTKEYS" label; scroll height `min(540, rows·86)`; rows `hotkeyActionRow` (SURFACE_1, 1 px BORDER, r6; grid margins 12,10,12,10): action label uppercase 14 px width 108, "Keyboard"/"Controller" captions 12 px TEXT_DIM, binding button (min 100×30; text = formatted combo or "Press keys" while recording), "Clear" 46×30, controller button + Clear disabled; actions CAPTURE CLIP, CLIP <len> per custom length, SCREENSHOT, CONFIRM GAME, DISMISS GAME; the Linux note: portal active → "Your desktop manages these shortcuts…" + "EDIT IN DESKTOP SETTINGS", else the compositor instructions from `fthrust_core::hotkeys::setup_instructions(compositor)` in monospace. Recording uses `fthrust_core::navigation` chord formatting.

- [ ] Tests: `capture_popup_markers_match_qt` (against `capture-popup.json` at 460×363), `source_popup_markers_match_qt`, `game_detection_popup_markers_match_qt`, `hotkey_popup_markers_match_qt`, `clip_length_outside_list_is_shown_and_kept` (Review Focus 4: fps 75 → combo shows "75" and `settings.json` is untouched until a change), `apply_appears_only_for_restart_keys`, `hotkey_row_count_follows_custom_lengths`.
- [ ] RED → implement → GREEN → commit `feat(ui): capture, source, game detection and hotkey popups`.

---

### Task 12: Settings page frame, General and Clip tabs

**Files:** `src/settings/{mod,general,clip}.rs`; tests `tests/fidelity_settings.rs`, `tests/settings_tabs.rs`.

**Interfaces:**
- `settings::ui(ui, &mut AppState)`: tab bar `settingsTabBar` (SHELL_BG_2, 1 px bottom SHELL_DIVIDER, margins (24,0,24,0), centred row of seven `settingsTabBtn[<name>]` text-under-36 px-icon: General, Clip, Audio, Visuals, Customize, Performance, Shortcuts; transparent, 2 px transparent bottom border, TEXT_DIM 14 px, padding 14 20 10 20, min width 72; hover SURFACE_3 + TEXT; checked TEXT 600 with 2 px ACCENT bottom border), 1 px `settingsDivider`, content `settingsContent` margins (24,24,24,0) with a 220 ms horizontal slide between tabs; Shift+Right/Left cycles; `Tab` enum and `tab_for_key`.
- Shared page recipe: scroll area, page margins (0,0,12,32), `flat_header`, `hsep`, default label 17 px.
- `general::ui`: "System" (autostart checkbox disabled, "Show Ko-fi donation widget"), "Notifications" (error notifications checkbox, "Capture card" label + mode combo), "Export Presets" (`ExportPresetsWidget`: combo "name · Built-in" + New/Edit/Delete + detail label; built-ins and user entries from `fthrust_core::export_presets`), "Troubleshooting" (hint, "Export diagnostic report" button → `dialogs::diagnostic_report` with core's `DiagnosticsSession::export_zip`), "Anticheat Detection" checkbox, "Settings Presets" (combo + Load/Save/Delete through `fthrust_core::presets`), "App Credits" text (the reference's string).
- `clip::ui`: "Clip Settings"; "Folders" rows (label width 180, line edit, "BROWSE", "USE DEFAULT") for `clips_directory` and `recording_directory` (BROWSE opens the file portal in plan 5; here it is a no-op button); "Import Clips" ("Add folder", "Scan for clips", linked-folder rows with remove from `fthrust_core::library_ownership`); hsep; "Video Encoding" rows (label width 140): encoder combo (from `fthrust_media::capabilities::load_cached` else "Checking this device…"), codec combo, preset combo, "Active encoder" value, Apply / Redetect; "Watermark" checkbox; "Clip Editor" checkboxes `timeline_preview_strip_enabled`, `hide_deleted_timeline_segments`, `grid_show_edited_duration`.

- [ ] Tests: `settings_tab_bar_markers_match_qt` (against `settings.json`: seven tab rects, checked underline sample ACCENT, divider), `general_page_markers_match_qt` (`general.json`), `clip_page_markers_match_qt` (`clip-settings.json`), `tab_cycle_keys`, `export_presets_widget_lists_builtins_then_user`, `settings_presets_save_load_delete_roundtrip`.
- [ ] RED → implement → GREEN → commit `feat(ui): settings frame, General and Clip tabs`.

---

### Task 13: Audio and Visuals tabs

**Files:** `src/settings/{audio,visuals}.rs`; tests `tests/fidelity_settings.rs` (extended).

**Interfaces:**
- `audio::ui`: two columns (left margins (0,0,24,0), right (24,0,0,0)). Left "Microphone": "Input device" (label 100) combo + 26×26 `micRefreshBtn`, "Downmix stereo microphone to mono", "Microphone gain" slider 0–200 + "100%" (width 42) + gain warning, "Live level" `MicLevelMeter` (10 px bar, ACCENT fill, 2 px TEXT peak, MIN/MAX markers 7 pt bold; level fed from `state.mic_level: Option<f32>`, 0 here), loopback checkbox; "Gary Mode" panel (76×58 preview of `gary.png`, "Enable Gary Mode", "Choose"/"Default", warning text). Right "Notifications": "Enable notification sounds", "Monitor:" (label 150) combo, "Notification volume" + one slider per sound key (`sound_volume_{clip,screenshot,error,startup,upload_successful,upload_failed}`, label 150, value 42). Bottom: hsep, "Audio Capture": "Enable audio capture", "Playback device to capture" combo + Refresh (devices from `state.audio_devices`), hint, "separate audio" checkbox, mode hint.
- `visuals::ui`: "Visual Overlays" header; options column width 380: "Camera" ("Burn camera overlay into clips", Device (label 58) combo + 26×26 refresh), "Image layers" ("Burn image layers into clips", layer combo + count label 92, "Add images"/"Remove"/"Clear all", "Fit" combo (label 30), "Opacity" slider 10–100 (label 52, value 34)), "Keyboard overlay" block (window combo empty on Linux), "Preview background" path + "Choose image"/"Use default"; preview column: "Composite preview" + source label + `OverlayPreview` stage (BG, 1 px BORDER_HI frame; overlay boxes 2 px ACCENT border, 3 px selected, 20 px translucent header strip with 8 pt label, 13×13 corner grip; drag/resize updates `state.overlay_layout` only).

- [ ] Tests: `audio_page_markers_match_qt` (`audio.json`), `visuals_page_markers_match_qt` (`visuals.json`), `mic_meter_paints_peak_and_markers`, `overlay_box_drag_moves_within_stage`.
- [ ] RED → implement → GREEN → commit `feat(ui): Audio and Visuals tabs`.

---

### Task 14: Customize, Performance and Shortcuts tabs

**Files:** `src/settings/{customize,performance,shortcuts}.rs`, `src/widgets/color.rs` (swatch + picker); tests `tests/fidelity_settings.rs` (extended), `tests/customize.rs`, `tests/shortcuts.rs`.

**Interfaces:**
- `customize::ui`: info bar label (TEXT_MUTED 14 px, SURFACE_2, padding 12×16, r8) listing the supported formats; action bar: "Export theme" (secondary), "Import theme" (outline), stretch, "Reset all" (outline), "Apply theme" (primary); six accordions 01 Typography (display/body family combos from built-ins + imported, "Import font…"), 02 Colors (`colorPreview` mockup BG 1 px BORDER r8 + `ColorSwatch` per `CUSTOMIZABLE_COLOR_GROUPS` with the group labels; picker edits `state.theme_draft` and saves on every change, visuals change only on Apply), 03 Icon Colors (global tint + per-icon `IconTintRow`), 04 Icons (`IconRow` import/crop placeholder/reset), 05 Capture Card (seven tokens), 06 Sounds & Volumes (`SoundRow` preview/volume/import/reset; sounds apply immediately). Export/Import/Reset go through `fthrust_core::theme::{archive, reset}`; Apply calls `theme::apply` on `state.tokens` and `fonts::install`.
- `performance::ui`: "Clip exports" ("Allow simultaneous exports", limit combo, hint, "View export jobs" opens the plan-6 dialog placeholder), "Interface performance" (preset combo from core's `ui_performance` presets + "Custom", per-option checkboxes "<title> · <impact> impact" with detail labels), hsep, "Audio Processing" checkbox, "Video Encoding" row hidden unless NVENC is active.
- `shortcuts::ui`: "Beta" (ACCENT 17 px), hint, "Enable shortcuts", error label (DELETE), categories Application / Clips / Settings / Clip editor (headers ACCENT 17 px 600 with "Enable <category> shortcuts"), rows label 15 px + `BindingButton` ("Press keys…" while recording; chord captured from `ui.input()` and formatted/validated by `fthrust_core::navigation`, conflicts shown in the error label and the old binding kept: Review Focus 5) + "Clear" outline; "Restore default shortcuts".

- [ ] Tests: `customize_page_markers_match_qt` at 1440×900 (`customize.json`), `customize_layout_widths_match_qt` at 1920/2560/3840 (`customize-<w>.json`), `colors_and_typography_sections_match_qt` (`colors.json`, `typography.json`), `color_edit_saves_draft_but_applies_on_apply`, `performance_page_markers_match_qt` (`performance.json`), `shortcuts_page_markers_match_qt` (`shortcuts.json`), `binding_conflict_keeps_old_binding` (Review Focus 5), `restore_defaults_resets_bindings`.
- [ ] RED → implement → GREEN → commit `feat(ui): Customize, Performance and Shortcuts tabs`.

---

### Task 15: Dialogs

**Files:** `src/dialogs/{mod,custom_clip_length,export_preset,diagnostic_report}.rs`; tests `tests/fidelity_dialogs.rs`, `tests/dialogs.rs`.

**Interfaces:**
- `DialogQueue` (one modal at a time; non-modal messages stack) in `state`; `custom_clip_length::ui -> Option<u32>` (seconds input + warning; validation through `fthrust_core::capture_config::validate_clip_length`), `export_preset::ui(preset: &mut ExportPreset) -> Option<ExportPresetAction { Save, Cancel }>` (form: Name, Target file size MB 1–10000 step 0.1, "Limit resolution" + width×height spins 16–8192 step 2, "Limit frame rate" + FPS 1–240, Video codec combo ["H.264 (reviewed runtime)"], Audio kbps 32–512; Cancel / "Save preset"), `diagnostic_report::ui(zip: &Path)` (message, draggable ZIP button (drag via `egui` dnd payload with the path), "Done").

- [ ] Tests: `message_and_question_dialogs_match_qt` (`dialog-message.json`, `dialog-question.json`), `input_dialog_matches_qt` (`dialog-input.json`), `custom_clip_length_matches_qt_and_validates` (`custom-clip-length.json`; 0 and 901 rejected with the core message), `export_preset_dialog_matches_qt` (`export-preset.json`).
- [ ] RED → implement → GREEN → commit `feat(ui): generic and settings dialogs`.

---

### Task 16: Fidelity gate, docs, final gate

**Files:** `tests/fidelity_all.rs` (one test per screen JSON asserting the gate, SSIM printed), `docs/ui-fidelity.md` (how to regenerate Qt references, run the gate, read a report), `CHANGES.md`, roadmap row 4, `README.md` (run the UI).

- [ ] `fidelity_all` passes for every JSON in `docs/reference/qt-screens/` that this plan owns (shell, settings tabs, popups, dialogs, error bar); screens owned by later plans (`library*.json`, `clip-card*.json`, `record-hover.json`, `clip-editor.json`) are listed as `#[ignore]` with the owning plan in the ignore reason.
- [ ] Gate: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace && cargo deny check`; `tools/check-assets.sh` clean; CI green on lavapipe; push.
- [ ] Commit `chore(ui): fidelity gate, docs, roadmap`.

---

## Self-review notes

- Spec coverage: 6.4 tokens/theme → Task 3; fonts → 4; icons → 5; widgets → 7, 8; frameless window → 9; title strip/toolbar/rail/error bar → 9, 10; popups → 11; seven tabs → 12–14; dialogs → 15; snapshot harness and gate → 6, 16; spec 11.2 brand string → 9; 11.10 gate definition → 6; 11.11 missing screens captured first → 2. Playback, overlays, clips page, single instance and game detection wiring are plans 5 and 6 by the roadmap.
- Type consistency: `Tokens`/`Color` (3) used by every widget; `Weight` (4) used by `fidelity::TextRun` (6) and widgets; `Icon`/`IconCache` (5) used by buttons (7), title strip (9), tabs (12); `PanelUi` (8) consumed by popups (11); `AppState` (9) consumed by 10–15; `DialogQueue` (15) referenced by 9's state as a type defined in 15's module (declare it in `state.rs` in Task 9 with the dialog kinds as an enum, filled in 15).
- Review Focus 1–5 pinned in Tasks 3, 5/6, 9, 11, 14.
- Proportion: values are pinned where a test needs them; long option lists point at the inventory section that holds them.
