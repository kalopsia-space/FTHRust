# FTHRust Plan 1: Bootstrap and `fthrust-core` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the FTHRust repository and the `fthrust-core` crate that reads and writes every `~/.fthr` file byte-compatibly with the Python reference, plus the pure state machines the UI and engine share.

**Architecture:** One Cargo workspace. `fthrust-core` is a library with one module per on-disk format or state machine, no I/O beyond the filesystem, no async, no UI. Parity is proven by fixtures generated from the reference Python code by `tools/gen-reference-fixtures.py`, so every default, migration and format rule is tested against what the original actually produces rather than against a reading of its source.

**Tech Stack:** Rust 1.98 stable, serde + serde_json, rusqlite (bundled), md-5, sha2, regex, thiserror, tempfile (dev), zip, tracing + tracing-appender, nix (for uid/mode checks).

**Spec:** `docs/superpowers/specs/2026-10-22-fthrust-design.md`. Reference detail: `docs/reference/reference-ui-inventory.md` sections 3, 4, 5.3 to 5.6 and 6.1, 6.9. The reference checkout is at `reference/` after Task 1.

## Global Constraints

- Config root is `~/.fthr`, never XDG config dirs. Runtime dir is `$XDG_RUNTIME_DIR/fthrust`, fallback `~/.fthr/run`.
- Every JSON file written by the reference with `indent=2` is written with 2-space pretty printing; files the reference writes compact (`separators=(',',':')`) are written compact. Atomic writes go to `<name>.tmp` in the same directory then rename.
- Keys beginning with `_` are never persisted.
- Licence GPL-3.0-only. `LICENSE` is the reference's file, unchanged.
- No Qt, Python or C++ at runtime. Reference Python runs only inside `tools/` to generate fixtures.
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` and `cargo test --workspace` pass at the end of every task.
- Fixtures live in `crates/fthrust-core/tests/fixtures/` and are committed; the generator is re-runnable and must produce identical output.

## Review Focus

Inputs the spec implies but no reference test exercises, most likely to bite first:

1. A `settings.json` that is valid JSON but not an object (an array or a string): must quarantine to `.corrupt` and use defaults, not panic. Test added to Task 4.
2. A clips directory path that contains a symlink to a directory outside the root: library scan must not follow it and delete-by-ownership must classify the target as EXTERNAL. Test added to Task 13.
3. `hotkeys.json` with a non-string value for an action (number or null): loader must drop that action and keep the rest. Test added to Task 7.
4. `clip_metadata.sqlite3` locked by another process (the Python app still running): the store must return an error, never silently fall back to the legacy JSON. Test added to Task 14.
5. Theme `colors` entries that are not `#rrggbb` (named colours, 8-digit hex, garbage): loader must replace that token with the default and keep the rest. Test added to Task 9.

---

### Task 1: Workspace bootstrap, reference fetch, GitHub repo

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`, `LICENSE`, `README.md`, `CHANGES.md`, `deny.toml`, `.github/workflows/ci.yml`, `tools/fetch-reference.sh`, `tools/reference-venv.sh`, `crates/fthrust-core/Cargo.toml`, `crates/fthrust-core/src/lib.rs`

**Interfaces:**
- Produces: workspace root at `/home/kalopsia/fthrrst-clips`; `reference/` = FTHR-Clips at `aa0dc9b`; `.venv-reference/` with PySide6 importable; crate `fthrust-core` (empty).

- [ ] **Step 1: Initialise git and the workspace**

`git init -b main`. Root `Cargo.toml`: `[workspace] members = ["crates/*"]`, `resolver = "3"`, `[workspace.package] version = "2.0.0-alpha.1", edition = "2024", license = "GPL-3.0-only"`, `[workspace.dependencies]` pinning `serde = { version = "1", features = ["derive"] }`, `serde_json = { version = "1", features = ["preserve_order"] }`, `thiserror = "2"`, `regex = "1"`, `md-5 = "0.10"`, `sha2 = "0.10"`, `rusqlite = { version = "0.37", features = ["bundled"] }`, `zip = "4"`, `tempfile = "3"`, `tracing = "0.1"`, `tracing-subscriber = { version = "0.3", features = ["env-filter"] }`, `tracing-appender = "0.2"`, `nix = { version = "0.30", features = ["fs", "user"] }`, `time = { version = "0.3", features = ["formatting", "macros"] }`. `rust-toolchain.toml`: `channel = "1.98.1"`, components rustfmt, clippy. `.gitignore`: `/target`, `/reference`, `/.venv-reference`, `*.tmp`.

- [ ] **Step 2: Copy LICENSE and write README and CHANGES**

`LICENSE` copied from the reference checkout after Step 3 (so do Step 3 first, then return). `README.md`: name FTHRust, one paragraph from spec section 1, link to the spec, "Derived from FTHR Clips by FTHR-Community, GPL-3.0-only". `CHANGES.md`: heading `# Changes relative to FTHR Clips linux-2.0 (aa0dc9b)` and an empty `## Fixed` list.

- [ ] **Step 3: Write `tools/fetch-reference.sh`**

Clones `https://github.com/FTHR-Community/FTHR-Clips` into `reference/` if absent, `git -C reference fetch origin aa0dc9b` and `git -C reference checkout --detach aa0dc9b`; idempotent; exits non-zero if the resulting HEAD is not `aa0dc9b`. Run it. Expected: `reference/FTHR_UI/main.py` exists.

- [ ] **Step 4: Write `tools/reference-venv.sh`**

Creates `.venv-reference` with `python3 -m venv --system-site-packages`, installs `opencv-python-headless numpy sounddevice jeepney`, then verifies `import PySide6, cv2, numpy, sounddevice, jeepney`. Run it. Expected: prints `reference venv ok`.

- [ ] **Step 5: Create the empty crate and CI**

`crates/fthrust-core/Cargo.toml` with `[package] name = "fthrust-core"`, workspace version/edition/license, dependencies from workspace. `src/lib.rs` with `//! Shared on-disk formats and state machines for FTHRust.` and nothing else. `deny.toml`: licenses allow `GPL-3.0`, `MIT`, `Apache-2.0`, `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `Zlib`, `Unicode-3.0`, `MPL-2.0`; advisories deny unmaintained = warn. `.github/workflows/ci.yml`: on push and pull_request, ubuntu-24.04, steps: checkout, `dtolnay/rust-toolchain@1.98.1` with clippy+rustfmt, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, `cargo install cargo-deny --locked` then `cargo deny check`.

- [ ] **Step 6: Verify**

Run: `cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all succeed with no warnings.

- [ ] **Step 7: Commit and create the GitHub repo**

```bash
git add -A && git commit -m "chore: bootstrap FTHRust workspace"
gh repo create kalopsia-space/FTHRust --private --source=. --remote=origin --push
```
Expected: `gh repo view kalopsia-space/FTHRust --json visibility` prints `PRIVATE`.

---

### Task 2: Reference fixture generator

**Files:**
- Create: `tools/gen-reference-fixtures.py`, `crates/fthrust-core/tests/fixtures/*` (generated)

**Interfaces:**
- Produces fixture files, all JSON with `indent=2, sort_keys=True` unless noted:
  - `settings-defaults.json`: the non-underscore keys of `SettingsManager().settings` on a fresh HOME, with `clips_directory` and `recording_directory` rewritten to the literal strings `"$HOME/Videos/FTHR Clips"` and `"$HOME/Videos/FTHR Clips/Recordings"`.
  - `settings-migrations/<case>/input.json` and `expected.json` for cases: `legacy-no-clips-dir` (file `{}` → clips dir `$HOME/FTHR_Clips`), `retired-keys` (input has `multiband_audio_enabled`, `audio_categories`, `auto_crop_enabled`, `extended_clip_length`, `keyboard_overlay_enabled`, `hotkeys.save_extended_clip`), `recording-seed` (input has `framerate: 120, resolution: "1080p", bitrate_level: "high", custom_bitrate_kbps: 40000` and no `recording_*`), `image-overlay-singular` (input has `image_overlay_enabled: true, image_overlay_path: "/tmp/x.png", image_overlay_opacity: 50` and no `image_overlays`), `camera-legacy` (input has `camera_position: "top-left", camera_size: "small"` and no `camera_overlay_rect`), `nav-v1` (input `navigation_bindings_version: 1`, bindings `editor_next: "Right", editor_previous: "Left", editor_minimize: "Escape", editor_close: "Shift+Escape", editor_split: "Ctrl+S"`), `one-level-merge` (input `source_volumes: {"game": 50}` → expected keeps browser/music/discord 100), `unknown-keys-kept` (input `my_custom_key: 7`). The generator writes `input.json` to a temp HOME, constructs `SettingsManager`, and dumps the result.
  - `theme-defaults.json`: `{"colors": DEFAULT_COLORS, "capture_card": DEFAULT_CAPTURE_CARD_COLORS, "fonts": DEFAULT_FONTS, "icon_tint": DEFAULT_ICON_TINT, "customizable_icons": CUSTOMIZABLE_ICONS, "customizable_sounds": CUSTOMIZABLE_SOUNDS, "color_groups": CUSTOMIZABLE_COLOR_GROUPS, "legacy": {"v1": _LEGACY_COLORS, "v2": _V2_COLORS, "v3": _V3_COLORS, "cc_v1": ..., "cc_v2": ..., "cc_v3": ...}, "icon_assets": ui.icons.ICON_ASSETS}`.
  - `theme-migrations/<case>/input.json` and `expected.json`: `v1-untouched-defaults` (palette_version 1, colours equal to `_LEGACY_COLORS`), `v1-user-override` (palette_version 1, `ACCENT: "#123456"`), `v3-global-tint` (palette_version 3, `icon_tints: {"_global": "#b4bdc7"}`), `segoe-font` (typography_version 1, fonts body `Segoe UI`), `missing-keys` (colors has only `BG`). Expected is `ThemeManager().data` after load.
  - `navigation-bindings.json`: list of `[action, label, default, scope, category]` from `BINDING_DEFINITIONS`, plus `"normalize_cases"`: a list of `[input, output]` for inputs `"ctrl+s"`, `"Shift+Ctrl+S"`, `"esc"`, `"enter"`, `"win+x"`, `"Tab+S"`, `"cmd+q"`, `"ctrl+"`, `""`, `"F13"` (output `null` where the reference returns None/raises).
  - `hotkeys.json`: `{"actions": _HOTKEY_ACTIONS, "descriptions": {...}, "defaults": HotkeyManager defaults, "normalize_cases": [[in,out]...] for "ctrl+shift+s", "f9", "Shift+Ctrl+a", "page up", "WIN+F1", "ctrl+", "", "a"`, `"portal_trigger_cases": [[in,out]...]` for `"Ctrl+Shift+S"`, `"F9"`, `"Esc"`, `"Page Up"`, `"Scroll Lock"`, `"Win+A"`, `"Alt+Shift+F5"`, `"legacy_files": {...}` three example files: flat, nested keyboard/controller, legacy top-level `keyboard` map with `game_capture_accept`, each with the loader's resulting mapping}`.
  - `error-codes.json`: list of `{code, title, category, explanation}` in order.
  - `export-presets.json`: `{"builtin_discord": <dict>, "catbox_adhoc": <dict>, "validation_cases": [[input_dict, accepted_bool]...]}` for target_size_mb 0.5, 1, 10000, 10001; audio_bitrate_kbps 31, 32, 512, 513; target_fps 0, 1, 240, 241; safety_margin 0.79, 0.80, 0.99, 1.0; width without height.
  - `clip-files.json`: `{"completed_video_cases": [[name, bool]...]}` for `a.mp4`, `a.mkv`, `a.avi`, `a.mp4.partial`, `.a.partial.0123abcd.mp4`, `a.png`, `A.MP4`, `a.webm`; `"sanitize_cases"` for `Over:watch*?`, `CON`, `""`, 40-char name, `"Game With Spaces"`, `"../etc"`, `"\u0001ctl"`; `"partial_marker": "_clip_from_"`.
  - `diagnostics-redaction.json`: `[[input, output]...]` by running `_RedactingFilter` on: `Authorization: Bearer abc.def`, `token=abcdef123456`, `api_key: "xyz"`, `password=hunter2 user=bob`, `https://user:pw@host/path`, `plain text 123`, `secret=1`, `X-Auth-Header: foo`.
  - `window-geometry.json`: `restore_cases`: list of `{saved, screens, expected}` for: saved screen present, saved screen missing with overlap, no overlap → primary, rect too small, maximized flag, empty saved → centred default, using screens `[{name:"DP-3", x:0,y:0,w:2560,h:1440, primary:true},{name:"HDMI-1", x:2560,y:0,w:1920,h:1080}]`.
  - `capture-config.json`: `buffer_seconds_cases` `[[clip_length, custom_list, expected]...]` and `ring_budget_cases` `[[bitrate_kbps, buffer_s, ram_mb, expected_mb]...]` from `compute_buffer_seconds` and the Linux ring budget function for (25000, 32, 32768), (200000, 905, 8192), (500, 7, 1024).
  - `save-state.json`: scripted response sequences T1..T6 from `tests/test_save_state.py` with the expected state after each step.
  - `capture-health.json`: thresholds and the flag integers; `observe_sequences`: three sequences of `[connected, frame_count, flags, generation, t_seconds]` → expected `state` per step: healthy progress, stall past 8 s then recovery flag, new generation re-baseline.

- [ ] **Step 1: Write the generator**

`tools/gen-reference-fixtures.py` runs under `.venv-reference/bin/python` with `QT_QPA_PLATFORM=offscreen`, `sys.path.insert(0, "reference/FTHR_UI")`, redirects `HOME` to a fresh temp dir per case, and writes every file above to `crates/fthrust-core/tests/fixtures/`. It must not import `main.py`. Where a value is a Python object (dataclass, tuple), convert to plain JSON. Expected values in migration cases come from the reference classes, never typed by hand.

- [ ] **Step 2: Run it twice and diff**

Run: `.venv-reference/bin/python tools/gen-reference-fixtures.py && git add -A crates/fthrust-core/tests/fixtures && .venv-reference/bin/python tools/gen-reference-fixtures.py && git diff --exit-code crates/fthrust-core/tests/fixtures`
Expected: second run produces no diff; `settings-defaults.json` has 84 keys; `error-codes.json` has 23 entries; `navigation-bindings.json` has 22 definitions.

- [ ] **Step 3: Commit**

```bash
git add tools/gen-reference-fixtures.py crates/fthrust-core/tests/fixtures
git commit -m "test: generate parity fixtures from the FTHR Clips reference"
```

---

### Task 3: Paths and atomic JSON

**Files:**
- Create: `crates/fthrust-core/src/paths.rs`, `crates/fthrust-core/src/atomic_json.rs`, `crates/fthrust-core/src/error.rs`
- Modify: `crates/fthrust-core/src/lib.rs`
- Test: `crates/fthrust-core/tests/paths.rs`, `crates/fthrust-core/tests/atomic_json.rs`

**Interfaces:**
- Produces:
  - `pub enum CoreError { Io(std::io::Error), Json { path: PathBuf, source: serde_json::Error }, Corrupt { path: PathBuf, quarantined: PathBuf }, RuntimeDir(String), Sqlite(rusqlite::Error), Invalid(String) }` with `thiserror`, `pub type Result<T> = std::result::Result<T, CoreError>`.
  - `pub struct FthrPaths { home: PathBuf }` with `pub fn new(home: impl Into<PathBuf>) -> Self`, `pub fn from_env() -> Result<Self>` (`$HOME`, error if unset), and accessors returning `PathBuf`: `root()` = `home/.fthr`, `settings_file()`, `presets_file()`, `export_presets_file()`, `encoder_capabilities_file()`, `clip_metadata_db()`, `legacy_clip_metadata_json()`, `thumbnails_dir()`, `theme_dir()`, `theme_file()`, `theme_icons_dir()`, `theme_sounds_dir()`, `theme_fonts_dir()`, `logs_dir()`, `app_log()` (`fthr.log`), `crash_log()`, `engine_log()`, `diagnostics_dir()`, `uploader_dir()`, `uploader_settings_file()`, `upload_history_file()`, `legacy_upload_history_file()`, `playback_cache_dir()`, `hotkeys_file()`, `hotkeys_portal_cleared_file()`, `default_clips_directory()` = `home/Videos/FTHR Clips`, `legacy_clips_directory()` = `home/FTHR_Clips`.
  - `pub fn runtime_dir() -> Result<PathBuf>`: `$XDG_RUNTIME_DIR/fthrust` else `~/.fthr/run`; creates with mode 0700; errors `RuntimeDir` if the path is a symlink, not a directory, not owned by the current uid, or has group/other permission bits.
  - `pub fn write_json_atomic(path: &Path, value: &serde_json::Value, pretty: bool) -> Result<()>`: writes `path` with extension appended `.tmp` (`settings.json.tmp`), fsyncs, renames over `path`. Pretty = 2-space indent with a trailing newline; compact = `serde_json::to_string`.
  - `pub fn read_json(path: &Path) -> Result<Option<serde_json::Value>>`: `Ok(None)` when the file does not exist; `Err(Json)` when unparsable.

- [ ] **Step 1: Write the failing tests**

`tests/paths.rs`: `root_is_dot_fthr_under_home`, `every_accessor_is_under_root` (loop over all accessors except the two clips-dir ones, assert `starts_with(root)`), `default_clips_directory_matches_fixture` (equals `settings-defaults.json`'s `clips_directory` with `$HOME` substituted), `runtime_dir_refuses_group_writable` (set `XDG_RUNTIME_DIR` to a tempdir with a `fthrust` dir chmod 0770 → `Err(CoreError::RuntimeDir(_))`), `runtime_dir_creates_0700`.
`tests/atomic_json.rs`: `pretty_write_matches_python_indent2` (value `{"b":1,"a":[1,2]}` with `preserve_order` → bytes equal `"{\n  \"b\": 1,\n  \"a\": [\n    1,\n    2\n  ]\n}\n"`), `write_leaves_no_tmp_file`, `write_replaces_existing_content`, `read_missing_is_none`, `read_garbage_is_json_error`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fthrust-core --test paths --test atomic_json`
Expected: compile errors for missing modules.

- [ ] **Step 3: Implement `error.rs`, `paths.rs`, `atomic_json.rs`; export from `lib.rs`**

Use `nix::sys::stat` and `nix::unistd::getuid` for the runtime-dir checks. Pretty printing: `serde_json::to_writer_pretty` then push `\n`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p fthrust-core --test paths --test atomic_json`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fthrust-core && git commit -m "feat(core): paths, runtime dir, atomic JSON"
```

---

### Task 4: Settings store: defaults, load, save, quarantine

**Files:**
- Create: `crates/fthrust-core/src/settings/mod.rs`, `crates/fthrust-core/src/settings/defaults.rs`
- Test: `crates/fthrust-core/tests/settings_store.rs`

**Interfaces:**
- Consumes: `FthrPaths`, `write_json_atomic`, `read_json`.
- Produces:
  - `pub fn defaults(paths: &FthrPaths) -> serde_json::Map<String, Value>`: the 84 keys of the fixture, with `clips_directory` = `paths.default_clips_directory()` and `recording_directory` = that joined `Recordings`, both as strings.
  - `pub struct SettingsStore { paths: FthrPaths, data: Map<String, Value>, dirty: bool }`.
  - `pub fn load(paths: FthrPaths) -> Result<Self>`: missing file → defaults; parse error or non-object → rename to `settings.json.corrupt` and use defaults; object → `merge_one_level(defaults, loaded)` then `migrate` (Task 5) then, if migration changed anything, `save()`.
  - `pub fn merge_one_level(base: Map, over: Map) -> Map`: for each key in `over`, if both values are objects, union with `over` winning per child key; otherwise `over` replaces.
  - `pub fn get(&self, key: &str) -> Option<&Value>`, `get_bool(&self, key) -> Option<bool>`, `get_i64`, `get_f64`, `get_str(&self, key) -> Option<&str>`, `get_or_default(&self, key) -> &Value` (falls back to `defaults()` value, panics if the key is unknown to defaults; use only for keys present in the fixture).
  - `pub fn set(&mut self, key: &str, value: Value)`; `pub fn remove(&mut self, key: &str) -> Option<Value>`.
  - `pub fn save(&mut self) -> Result<()>`: pretty JSON of `data` without keys starting with `_`.
  - `pub fn clips_directory(&self) -> Result<PathBuf>` and `recording_directory(&self) -> Result<PathBuf>`: expand `$VAR` and `~`, canonicalise non-strictly (`path_clean`-style, no symlink resolution), error `Invalid` if relative or empty; `recording_directory` derives `<clips>/Recordings` when the key is absent or empty.
  - `pub fn clips_subdirectories() -> [&'static str; 5]` = `Desktop, Recordings, Exported, Shared, Screenshots`; `pub fn prepare_clips_storage(dir: &Path) -> Result<()>` creates root and the five.

- [ ] **Step 1: Write the failing tests**

`fresh_store_equals_fixture_defaults` (load with empty HOME, compare `data` to `settings-defaults.json` after substituting `$HOME`), `save_writes_indent2_and_skips_underscore_keys` (set `_runtime: 1`, save, read raw file, assert `_runtime` absent and the file ends with `}\n`), `garbage_file_is_quarantined` (write `{not json`, load, assert `settings.json.corrupt` exists with that content and data == defaults), `array_file_is_quarantined` (file `[1,2]`; Review Focus 1), `one_level_merge_keeps_sibling_children` (`source_volumes: {"game": 50}` → browser 100), `one_level_merge_does_not_recurse_two_levels` (`navigation_bindings: {"x": {"deep": 1}}` replaces the whole `x`), `unknown_keys_survive_round_trip`, `clips_directory_expands_env_and_tilde` (set `clips_directory` to `"~/vids/$FTHR_T"` with env var set), `relative_clips_directory_is_invalid`, `recording_directory_derives_from_clips_when_missing`, `prepare_clips_storage_creates_five_subdirs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fthrust-core --test settings_store`
Expected: compile errors.

- [ ] **Step 3: Implement `settings/defaults.rs` and `settings/mod.rs`**

`defaults.rs` builds the map in code with `serde_json::json!`, one entry per fixture key, in the same order as the fixture (the fixture is sorted; order does not matter for equality). Do not load the fixture at runtime. `migrate` is a stub returning `false` until Task 5.

- [ ] **Step 4: Run tests**

Run: `cargo test -p fthrust-core --test settings_store`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fthrust-core && git commit -m "feat(core): settings store with reference defaults and quarantine"
```

---

### Task 5: Settings migrations

**Files:**
- Create: `crates/fthrust-core/src/settings/migrate.rs`
- Modify: `crates/fthrust-core/src/settings/mod.rs`
- Test: `crates/fthrust-core/tests/settings_migrations.rs`

**Interfaces:**
- Produces: `pub(crate) fn migrate(data: &mut Map<String, Value>, paths: &FthrPaths, file_existed: bool) -> bool` (true if anything changed). Rules, in this order: (1) if `file_existed` and `clips_directory` absent → `paths.legacy_clips_directory()`; (2) remove retired keys `keyboard_overlay_*`, `mouse_overlay_*`, `input_overlay_preview_clip`, `multiband_audio_enabled`, `audio_categories`, `auto_crop_enabled`, `extended_clip_length`, and `hotkeys.save_extended_clip`; (3) seed `recording_framerate`, `recording_resolution`, `recording_bitrate_level`, `recording_custom_bitrate_kbps` from `framerate`, `resolution`, `bitrate_level`, `custom_bitrate_kbps` when absent; (4) when `image_overlays` absent and `image_overlay_path` non-empty, build one entry `{path, enabled, opacity, fit, rect}` from the singular keys; (5) when `camera_overlay_rect` absent, derive from `camera_position` and `camera_size` using the reference's `legacy_overlay_rect` table (copy the numbers from `reference/FTHR_UI/core/camera_overlay.py:28-58`); (6) navigation bindings: version `<2` map `editor_next: Right→Tab+Right`, `editor_previous: Left→Tab+Left`; `<4` swap `editor_minimize`/`editor_close` if they are still `Escape`/`Shift+Escape`, drop unknown actions; `<5` `editor_split: Ctrl+S→Ctrl+C`; then set `navigation_bindings_version` to 5.

- [ ] **Step 1: Write the failing test**

One test per directory in `fixtures/settings-migrations/`: a helper `run_case(name)` writes `input.json` as `settings.json` in a temp HOME, loads the store, and asserts `data` equals `expected.json` (with `$HOME` substituted in both). Also `migration_rewrites_file_once` (after load, file mtime changed and a second load changes nothing).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p fthrust-core --test settings_migrations`
Expected: FAIL on `retired-keys` and others.

- [ ] **Step 3: Implement `migrate.rs`**

- [ ] **Step 4: Run tests**

Run: `cargo test -p fthrust-core --test settings_migrations --test settings_store`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fthrust-core && git commit -m "feat(core): settings migrations matching the reference"
```

---

### Task 6: Navigation bindings

**Files:**
- Create: `crates/fthrust-core/src/navigation.rs`
- Test: `crates/fthrust-core/tests/navigation.rs`

**Interfaces:**
- Consumes: `SettingsStore`.
- Produces:
  - `pub struct BindingDefinition { pub action: &'static str, pub label: &'static str, pub default: &'static str, pub scope: Scope, pub category: &'static str }`, `pub enum Scope { Application, Main, Library, Settings, Editor }`, `pub const BINDING_DEFINITIONS: [BindingDefinition; 22]` in fixture order.
  - `pub fn normalize_binding(input: &str) -> Option<String>`: modifiers canonical order `Ctrl, Alt, Shift, Meta`; aliases `control→Ctrl`, `win|cmd|super→Meta`, `esc→Escape`, `enter→Return`; key part Title-cased, `F1..F24` upper; non-modifier chords allowed (`Tab+S`); empty key → `None`.
  - `pub struct NavigationBindings { bindings: BTreeMap<String, String>, categories_enabled: BTreeMap<String, bool>, enabled: bool }` with `pub fn from_settings(s: &SettingsStore) -> Self`, `pub fn write_to(&self, s: &mut SettingsStore)` (keys `navigation_bindings`, `shortcut_categories_enabled`, `shortcuts_enabled`), `pub fn get(&self, action) -> Option<&str>`, `pub fn set(&mut self, action: &str, combo: &str) -> std::result::Result<(), BindingConflict>`, `pub struct BindingConflict { pub action: String, pub other_action: String, pub combo: String }`. Conflict rule: reject when another action has the same normalised token set and (same scope, or either is `Application`, or the pair is `{Main, Settings}` or `{Main, Library}`). `pub fn is_category_enabled(&self, category) -> bool` (missing = true).

- [ ] **Step 1: Write the failing tests**

`definitions_match_fixture`, `normalize_cases_match_fixture` (loop over `normalize_cases`), `conflict_same_scope_rejected` (`editor_split` to `Ctrl+Z` conflicts with `editor_undo`), `conflict_application_scope_rejected` (`library_scroll_up` to `Tab+S`), `main_vs_editor_allowed` (`toggle_detection` to `Ctrl+Z` succeeds), `missing_category_is_enabled`, `round_trip_through_settings`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fthrust-core --test navigation`

- [ ] **Step 3: Implement `navigation.rs`**

- [ ] **Step 4: Run tests** — Expected: PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): navigation bindings with conflict rules"`

---

### Task 7: Global hotkey bindings file

**Files:**
- Create: `crates/fthrust-core/src/hotkeys.rs`
- Test: `crates/fthrust-core/tests/hotkeys.rs`

**Interfaces:**
- Produces:
  - `pub enum HotkeyAction { SaveClip, SaveScreenshot, StartRecording, StopRecording, ConfirmGameDetection, DismissGameDetection, SaveCustomClip(u32) }` with `pub fn id(&self) -> String` (`save_clip`, …, `save_custom_clip_<N>`), `pub fn parse(id: &str) -> Option<Self>` (also accepts legacy `game_capture_accept`/`game_capture_dismiss`; custom N must be 5..=900), `pub fn description(&self) -> String` from the fixture (`"Save N-second clip"` for custom).
  - `pub fn normalize_keyboard_combo(input: &str) -> Option<String>`: modifiers `Ctrl, Alt, Shift, Win` in that order, letters upper-cased, `F1..F24`, other keys Title-cased, joined by `+`.
  - `pub fn to_portal_trigger(combo: &str) -> String`: `Ctrl→CTRL, Alt→ALT, Shift→SHIFT, Win→LOGO`, key map from the fixture (`Esc→Escape`, `Page Up→Prior`, `Scroll Lock→Scroll_Lock`, single letters lower-cased).
  - `pub struct HotkeyBindings { map: BTreeMap<String, String> }` (action id → combo, empty string allowed) with `pub fn defaults() -> Self` (`save_clip=F9`, `save_screenshot=F12`, `confirm_game_detection=F8`, `dismiss_game_detection=F7`, recording ones empty), `pub fn load(paths: &FthrPaths) -> Result<(Self, bool /*needs_resave*/)>` accepting flat, nested `{keyboard, controller}` per action, legacy top-level `keyboard` map, dropping `save_extended_clip`, dropping non-string values (Review Focus 3), mapping legacy ids; `pub fn save(&self, paths) -> Result<()>` flat pretty JSON; `pub fn custom_clip_lengths(&self) -> Vec<u32>`; `pub fn sync_custom_clips(&mut self, lengths: &[u32])` (adds missing with empty combo, removes absent).
  - `pub fn load_portal_cleared(paths) -> Result<BTreeSet<String>>` and `pub fn save_portal_cleared(paths, &BTreeSet<String>) -> Result<()>` (sorted JSON list).

- [ ] **Step 1: Write the failing tests**

`defaults_match_fixture`, `normalize_cases_match_fixture`, `portal_trigger_cases_match_fixture`, one test per `legacy_files` entry asserting the loaded map and `needs_resave == true`, `non_string_value_is_dropped` (`{"save_clip": 9, "save_screenshot": "F12"}` → only screenshot), `custom_clip_out_of_range_is_dropped` (`save_custom_clip_4`, `save_custom_clip_901`), `portal_cleared_round_trip_sorted`.

- [ ] **Step 2: Run to verify failure** — `cargo test -p fthrust-core --test hotkeys`

- [ ] **Step 3: Implement `hotkeys.rs`**

- [ ] **Step 4: Run tests** — Expected: PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): hotkeys.json with legacy shapes and portal trigger mapping"`

---

### Task 8: Capture presets and export presets

**Files:**
- Create: `crates/fthrust-core/src/presets.rs`, `crates/fthrust-core/src/export_presets.rs`
- Test: `crates/fthrust-core/tests/presets.rs`, `crates/fthrust-core/tests/export_presets.rs`

**Interfaces:**
- Produces:
  - `pub const PRESET_KEYS: [&str; 8]` = `clip_length, framerate, resolution, bitrate_level, encoder_pref, codec_pref, encoder_preset, audio_capture_enabled`.
  - `pub struct PresetsStore { path: PathBuf }` with `pub fn new(paths: &FthrPaths) -> Self`, `pub fn list_names(&self) -> Result<Vec<String>>` (sorted), `pub fn load(&self, name) -> Result<Option<Map<String, Value>>>` (only `PRESET_KEYS`; `extended_clip_length`, `multiband_audio_enabled`, `audio_categories`, `auto_crop_enabled` stripped and the file rewritten if any were present), `pub fn save(&self, name, values: &Map) -> Result<()>` (keeps only `PRESET_KEYS`), `pub fn delete(&self, name) -> Result<bool>`. Unreadable file → empty map.
  - `#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)] pub struct ExportPreset { pub preset_id: String, pub name: String, pub target_size_mb: f64, pub target_width: Option<u32>, pub target_height: Option<u32>, pub target_fps: Option<u32>, pub video_codec: String, pub audio_bitrate_kbps: u32, pub built_in: bool, pub safety_margin: f64 }` with `pub fn validate(&self) -> std::result::Result<(), String>` enforcing the fixture ranges and `video_codec ∈ {h264, source}`; `pub fn builtin_discord() -> Self`; `pub fn catbox_adhoc() -> Self`; `pub fn slug(name: &str) -> String` (reference `_slug`).
  - `pub struct ExportPresetStore { path }` with `all()` (built-in first then customs in file order), `get(id)`, `custom()`, `save(preset)` (replaces by id, refuses `built_in` or id `discord`), `delete(id) -> Result<bool>`. File is a JSON list, pretty. Invalid or duplicate entries skipped on read.

- [ ] **Step 1: Write the failing tests**

presets: `round_trip`, `list_names_sorted`, `delete_missing_false`, `load_strips_retired_keys_and_rewrites_file`, `save_keeps_only_preset_keys`, `unreadable_file_is_empty`.
export: `builtin_discord_matches_fixture`, `catbox_adhoc_matches_fixture`, `validation_cases_match_fixture`, `discord_never_written_to_file`, `duplicate_id_skipped_on_read`, `save_replaces_by_id`, `width_without_height_rejected`.

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement both modules**

- [ ] **Step 4: Run tests** — `cargo test -p fthrust-core --test presets --test export_presets` PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): capture presets and export presets stores"`

---

### Task 9: Theme: defaults, load, migrations, tints

**Files:**
- Create: `crates/fthrust-core/src/theme/mod.rs`, `crates/fthrust-core/src/theme/defaults.rs`, `crates/fthrust-core/src/theme/migrate.rs`
- Test: `crates/fthrust-core/tests/theme.rs`

**Interfaces:**
- Produces:
  - `pub const COLOR_TOKENS: [&str; 25]` in fixture order; `pub fn default_colors() -> BTreeMap<String, String>`, `default_capture_card_colors()`, `default_fonts()`, `pub const DEFAULT_ICON_TINT: &str = "#a0aba5"`, `pub const CUSTOMIZABLE_ICONS: [(&str, &str); N]` (key, label), `CUSTOMIZABLE_SOUNDS: [&str; 6]`, `COLOR_GROUPS: [(&str, &[&str]); 6]`, `ICON_ASSETS: [(&str, &str); N]` (logical png → bundled svg), `pub fn legacy_palette(version: u8) -> BTreeMap<String, String>` for 1, 2, 3 and `legacy_capture_card(version)`.
  - `#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)] pub struct ThemeData { pub palette_version: u32, pub typography_version: u32, pub colors: BTreeMap<String,String>, pub icons: BTreeMap<String,String>, pub sounds: BTreeMap<String,String>, pub icon_tints: BTreeMap<String,String>, pub capture_card: BTreeMap<String,String>, pub fonts: BTreeMap<String,String>, pub font_files: BTreeMap<String,String> }` with `Default` = fixture defaults, versions 4 and 2.
  - `pub struct ThemeStore { paths: FthrPaths, pub data: ThemeData }` with `pub fn load(paths) -> Result<Self>` (creates `theme/{icons,sounds,fonts}`; missing file → defaults; parse failure → defaults without backup; merge `{defaults, loaded}` for colors/capture_card/fonts; apply migrations; invalid colour strings replaced by the default token (Review Focus 5); loaded file not rewritten), `pub fn save(&self) -> Result<()>` (pretty, `theme.json.tmp`), `pub fn color(&self, token) -> &str`, `pub fn capture_card_color(&self, key) -> &str` (`#ffffff` for unknown), `pub fn icon_tint(&self, logical: &str) -> &str` (per-file else `_global` else default; `favicon.ico` → never tinted, return `""`), `pub fn custom_icon_path(&self, logical) -> Option<PathBuf>`, `pub fn custom_sound_path(&self, key) -> Option<PathBuf>`, `pub fn set_custom_icon(&mut self, logical, source: &Path) -> Result<()>` (copies to icons dir as `<stem>.<source ext>`; accepted ext `.png .jpg .jpeg .bmp .ico .svg`), `set_custom_sound(&mut self, key, source)` (dest `<key><ext>`; ext `.mp3 .wav .ogg .flac .m4a .wma .aac`), `set_custom_font(&mut self, families: &[String], source)` (`.ttf .otf`), `custom_font_paths(&self) -> Vec<(String, PathBuf)>` (drops vanished files), `pub fn has_any_customization(&self) -> bool`, `pub fn reset_all(&mut self)` (data to defaults, files left on disk).
  - `pub fn is_hex_color(s: &str) -> bool` (`#` + 6 hex digits, case-insensitive).
  - Migrations in `migrate.rs`: per token, replace only when the stored value equals the previous shipped default for that version step; `_global` tint `#00ffaa`→TEXT_DIM at `<3`, `#b4bdc7`→`#a0aba5` at `<4`; fonts: unknown family → `Oswald`; `typography_version<2` and `Segoe UI` → `Oswald`. Built-in families `Oswald`, `Segoe UI`.

- [ ] **Step 1: Write the failing tests**

`defaults_match_fixture` (colors, capture_card, fonts, tint, lists), one test per `theme-migrations/<case>` comparing `data` to `expected.json`, `invalid_color_replaced_by_default` (`ACCENT: "green"`, `BG: "#aabbccdd"`), `favicon_never_tinted`, `unknown_capture_card_key_is_white`, `set_custom_icon_names_dest_by_stem_and_source_ext` (`play.png` from `foo.svg` → `icons/play.svg`, data `icons["play.png"] == "play.svg"`), `custom_font_paths_drop_missing_files`, `reset_all_keeps_files`.

- [ ] **Step 2: Run to verify failure** — `cargo test -p fthrust-core --test theme`

- [ ] **Step 3: Implement the theme module**

- [ ] **Step 4: Run tests** — PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): theme store with palette migrations"`

---

### Task 10: Theme export and import

**Files:**
- Create: `crates/fthrust-core/src/theme/archive.rs`
- Test: `crates/fthrust-core/tests/theme_archive.rs`

**Interfaces:**
- Produces: `impl ThemeStore { pub fn export_zip(&self, dest: &Path) -> Result<()>; pub fn import_zip(&mut self, src: &Path) -> Result<()> }`. Export: `theme.json` plus every referenced file under `icons/`, `sounds/`, `fonts/`. Import: zip must contain `theme.json` (else `Invalid`), validates object types, reads all three asset folders into memory first, then wipes the three dirs, writes assets, rebuilds `data` at versions 4/2 with unknown font families → `Oswald`, saves.

- [ ] **Step 1: Write the failing tests**

`export_import_round_trip_preserves_fonts_and_icons`, `import_without_theme_json_fails_and_changes_nothing`, `import_wipes_previous_assets`, `import_unknown_font_family_falls_back_to_oswald`.

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement with the `zip` crate**

- [ ] **Step 4: Run tests** — PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): theme zip export and import"`

---

### Task 11: Error codes

**Files:**
- Create: `crates/fthrust-core/src/error_codes.rs`
- Test: `crates/fthrust-core/tests/error_codes.rs`

**Interfaces:**
- Produces: `pub struct AppFailureCode { pub code: &'static str, pub title: &'static str, pub category: &'static str, pub explanation: &'static str }`, `pub const APP_FAILURE_CODES: [AppFailureCode; 23]`, `pub fn code_for_title(title: &str) -> Option<&'static AppFailureCode>` (exact title match), `pub fn format_error_title(title: &str) -> String` → `"[Error NNN] TITLE"` when known, else the title unchanged.

- [ ] **Step 1: Write the failing tests**

`table_matches_fixture_in_order`, `every_title_is_unique`, `format_known_title`, `format_unknown_title_unchanged`.

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement**

- [ ] **Step 4: Run tests** — PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): public error code registry"`

---

### Task 12: Clip file naming, partials, staged output, identity

**Files:**
- Create: `crates/fthrust-core/src/clip_files.rs`, `crates/fthrust-core/src/transactional_output.rs`, `crates/fthrust-core/src/media_identity.rs`
- Test: `crates/fthrust-core/tests/clip_files.rs`, `crates/fthrust-core/tests/transactional_output.rs`

**Interfaces:**
- Produces:
  - `pub const VIDEO_SUFFIXES: [&str; 3]` (`mp4 mkv avi`), `IMAGE_SUFFIXES: [&str; 3]` (`png jpg jpeg`), `pub const PARTIAL_MARKER: &str = "_clip_from_"`.
  - `pub fn is_completed_video_path(p: &Path) -> bool` (video suffix, case-insensitive, and not partial), `pub fn is_partial_name(name: &str) -> bool` (ends with `.partial` or contains `.partial.`), `pub fn is_fthr_post_processing_dir(name: &str) -> bool` (`Exported`, `Shared`, or starts with `.fthr-`).
  - `pub fn sanitize_folder_name(s: &str) -> String`: strip `\/:*?"<>|` and control chars, trim, max 32 chars, Windows reserved names (`CON PRN AUX NUL COM1-9 LPT1-9`) get `_game` appended, empty → `Unknown`.
  - `pub fn replay_clip_path(root: &Path, game: Option<&str>, at: time::OffsetDateTime) -> PathBuf`: `<root>/Desktop/desktop_clip_from_<%d%b%Y_%H-%M-%S>.mp4` or `<root>/<sanitized>/<sanitized>_clip_from_<ts>.mp4`; `pub fn unique_path(p: PathBuf) -> PathBuf` appends `_2`, `_3`… before the extension while the file exists.
  - `pub fn recording_path(dir, at) -> PathBuf` (`FTHR_recording_<%Y-%m-%d_%H-%M-%S>.mp4`), `pub fn screenshot_path(root, at) -> PathBuf` (`Screenshots/screenshot_from_<%Y%m%d_%H%M%S_%f>.png`, `%f` = microseconds 6 digits), `pub fn share_path(root, stem, kind: ShareKind, at, lossless: bool) -> PathBuf` (`Shared/<stem>_share_<%H-%M-%S>_<8 hex>.mp4`, `_discord_`, `.mkv` when lossless).
  - `pub fn cleanup_stale_partials(root: &Path, now: SystemTime, max_age: Duration) -> Result<usize>`: removes `*_clip_from_*.mp4.partial` and orphan `*.mp4.fthr-audio.json` (no sibling mp4) older than `max_age` (24 h in callers), never follows symlinks, continues past per-file errors.
  - `pub struct StagedOutput { final_path: PathBuf, staged: PathBuf }` with `pub fn new(final_path: &Path) -> Self` (staged = `.<stem>.partial.<uuid4 hex><suffix>` in the same dir), `pub fn staged_path(&self) -> &Path`, `pub fn commit(self) -> Result<PathBuf>` (rename; error if dirs differ), `pub fn discard(self)` (idempotent, ignores missing).
  - `#[derive(PartialEq, Eq, Hash, Clone, Debug)] pub struct FileIdentity { pub real_path: PathBuf, pub size: u64, pub mtime_ns: i128, pub ctime_ns: i128, pub ino: u64 }` with `pub fn file_identity(p: &Path) -> Result<FileIdentity>`.

- [ ] **Step 1: Write the failing tests**

`completed_video_cases_match_fixture`, `sanitize_cases_match_fixture`, `replay_path_desktop_format` (fixed timestamp 2026-08-25 14:03:07 → `desktop_clip_from_25Aug2026_14-03-07.mp4`), `replay_path_game_folder`, `unique_path_suffixes`, `screenshot_path_has_microseconds`, `cleanup_removes_only_old_fthr_partials` (create a 25 h old `x_clip_from_a.mp4.partial`, a fresh one, a `foo.mp4.partial` not ours, an orphan manifest; assert only the two old FTHR files are removed), `cleanup_does_not_follow_symlinks`, `staged_output_commit_and_discard`, `staged_output_is_hidden_and_partial` (name starts with `.` and `is_partial_name`), `file_identity_changes_on_write`.

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement the three modules**

- [ ] **Step 4: Run tests** — PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): clip naming, partial cleanup, staged output, file identity"`

---

### Task 13: Library index and ownership

**Files:**
- Create: `crates/fthrust-core/src/library_index.rs`, `crates/fthrust-core/src/library_ownership.rs`
- Test: `crates/fthrust-core/tests/library_index.rs`, `crates/fthrust-core/tests/library_ownership.rs`

**Interfaces:**
- Consumes: `clip_files`, `media_identity`.
- Produces:
  - `#[derive(Clone, Debug, PartialEq)] pub struct LibraryRecord { pub path: PathBuf, pub identity: PathBuf /* canonical */, pub root_ids: Vec<usize>, pub imported: bool, pub kind: MediaKind, pub size: u64, pub mtime_ns: i128 }`, `pub enum MediaKind { Video, Image }`.
  - `pub struct ScanRoots { pub owned: PathBuf, pub imported: Vec<PathBuf> }`.
  - `pub fn scan(roots: &ScanRoots, cancel: &AtomicBool, mut on_batch: impl FnMut(Vec<LibraryRecord>)) -> Result<Vec<LibraryRecord>>`: recursive, skips symlinks (Review Focus 2) and `is_fthr_post_processing_dir`, skips partials, de-dupes by `identity` merging `root_ids`, publishes batches of 64 as discovered, returns an empty Vec and publishes nothing further once `cancel` is set, caps watched directories at 256 (returned via `pub fn watched_dirs(roots) -> Vec<PathBuf>`).
  - `pub enum Filter { All, Clips, Screenshots, Imported }`, `pub enum Sort { Newest, Oldest, Longest }`, `pub fn apply(records: &[LibraryRecord], f: Filter, s: Sort) -> Vec<LibraryRecord>` (Longest = size descending, ties by newest).
  - `pub enum Ownership { FthrOwned, LinkedImported, External }`, `pub fn classify_media_path(p: &Path, owned_root: &Path, imported_roots: &[PathBuf]) -> Ownership` (resolve symlinks first, then prefix test; imported wins over owned), `pub fn delete_owned_media(p: &Path, owned_root, imported_roots) -> Result<()>` (refuses with `Invalid("Linked or external media is protected")` unless `FthrOwned`; also removes the adjacent `.fthr-audio.json`), `pub fn normalize_import_root(p: &Path) -> Result<PathBuf>` (resolve), `pub fn add_import_root(list: &mut Vec<PathBuf>, p) -> bool` (dedupe by resolved path).

- [ ] **Step 1: Write the failing tests**

index: `scan_finds_videos_and_images_with_kinds`, `scan_skips_partials_and_post_processing_dirs`, `scan_skips_symlinked_dirs` (symlink inside root to a dir outside; its files absent), `scan_dedupes_overlapping_roots` (imported root inside owned root → one record with both root ids), `cancelled_scan_publishes_no_further_batch`, `batches_published_before_end` (200 files → first batch callback before return), `sorts_and_filters`.
ownership: `owned_can_delete`, `imported_is_protected`, `external_is_protected`, `symlink_inside_owned_to_external_is_external` (Review Focus 2), `add_import_root_dedupes_equivalent_paths`, `delete_removes_sidecar_manifest`.

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement both modules** (use `std::fs::symlink_metadata` to skip symlinks before descending)

- [ ] **Step 4: Run tests** — PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): library scan and linked-folder protection"`

---

### Task 14: Clip metadata SQLite store

**Files:**
- Create: `crates/fthrust-core/src/clip_metadata.rs`
- Test: `crates/fthrust-core/tests/clip_metadata.rs`

**Interfaces:**
- Produces:
  - `#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)] pub struct ClipEntry { pub tag: String, pub description: String, #[serde(skip_serializing_if = "Option::is_none")] pub editor_draft: Option<EditorDraft> }`, `pub struct EditorDraft { pub version: u32, pub file: DraftFile { mtime_ns: i128, size: u64 }, pub updated_at: f64, pub state: serde_json::Value }` (state kept opaque here; the editor crate owns its schema).
  - `pub struct ClipMetadataStore { conn: rusqlite::Connection }` with `pub fn open(paths: &FthrPaths) -> Result<Self>`: opens `clip_metadata.sqlite3`, `PRAGMA synchronous=FULL`, `busy_timeout` 0 so a lock surfaces as `Sqlite` error (Review Focus 4); creates `entries(path TEXT PRIMARY KEY, value TEXT NOT NULL)`; if the table did not exist and `legacy_clip_metadata_json()` exists, imports it once and leaves it in place. `pub fn get(&self, path: &Path) -> Result<ClipEntry>` (default when absent), `pub fn update(&mut self, path, tag: Option<&str>, description: Option<&str>) -> Result<()>` (partial update inside `BEGIN IMMEDIATE`), `pub fn set_editor_draft(&mut self, path, draft: EditorDraft) -> Result<()>` then prune to 250 drafts by oldest `updated_at`, `pub fn clear_editor_draft(&mut self, path) -> Result<()>` (keeps tag/description), `pub fn editor_draft_if_current(&self, path, identity: &FileIdentity) -> Result<Option<EditorDraft>>` (returns `None` when `file` does not match), `pub fn rename_path(&mut self, from, to) -> Result<()>`. Value JSON is compact. Keys are the absolute path string as given.

- [ ] **Step 1: Write the failing tests**

`defaults_for_unknown_clip`, `partial_update_preserves_other_fields`, `value_json_is_compact` (read the raw `value` column; no spaces after `:` or `,`), `draft_bound_to_file_identity`, `clear_draft_preserves_details`, `draft_storage_bounded_to_250`, `legacy_json_imported_once_and_left_in_place`, `rename_moves_entry`, `locked_database_errors_not_fallback` (hold an exclusive transaction on a second connection; `update` returns `Err(CoreError::Sqlite(_))` and the legacy JSON is not read).

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement**

- [ ] **Step 4: Run tests** — PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): clip metadata sqlite store with drafts"`

---

### Task 15: Audio manifest sidecar

**Files:**
- Create: `crates/fthrust-core/src/audio_manifest.rs`
- Test: `crates/fthrust-core/tests/audio_manifest.rs`

**Interfaces:**
- Produces:
  - `#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)] pub struct AudioSource { pub source_uuid: String, pub stream_index: u32, pub source_type: SourceType, pub display_name: String, pub persistent_identity: String, pub icon_reference: Option<String>, pub first_active_100ns: i64, pub last_active_100ns: i64, pub sample_rate: u32, pub channels: u8, pub sample_format: String }`, `pub enum SourceType { Application, Microphone, System }` (serde lowercase).
  - `pub struct AudioManifest { pub manifest_version: u32, pub transaction_id: String, pub media_file: String, pub media_sha256: String, pub sources: Vec<AudioSource> }`.
  - `pub fn rename_clip(media: &Path, new_stem: &str) -> Result<PathBuf>`: validates the name (no `<>:"/\|?*`, no control chars, not a Windows reserved name, ≤ 255 bytes with extension, else `Invalid`), renames the media and its sidecar manifest, rewrites `media_file` inside the manifest, and rolls the media rename back if the sidecar step fails. Target collisions are refused, never overwritten.
  - `pub fn manifest_path_for(media: &Path) -> PathBuf` (`<media>.fthr-audio.json`), `pub fn validate(&self) -> std::result::Result<(), String>` (1..=10 sources, `stream_index >= 1` and unique, text fields match `^[A-Za-z0-9 ._+()'\-]{1,128}$`, `media_file` has no path separators, sample_rate 8000..=192000, channels 1..=8), `pub fn write(&self, media: &Path) -> Result<()>` (compact, sorted keys, ASCII-escaped, via `StagedOutput`), `pub fn read_verified(media: &Path) -> Result<Option<AudioManifest>>` (`None` when missing, invalid, or sha256 of the mp4 does not match), `pub fn rebind_after_media_replace(media: &Path) -> Result<()>` (re-hash and rewrite), `pub fn sha256_file(p: &Path) -> Result<String>`.

- [ ] **Step 1: Write the failing tests**

`round_trip_is_compact_sorted_ascii`, `mismatched_hash_reads_as_none`, `path_like_identity_rejected`, `duplicate_stream_index_rejected`, `rebind_updates_hash`, `source_type_serialises_lowercase`, `rename_moves_media_and_manifest_and_rewrites_media_file`, `rename_rolls_back_when_sidecar_fails` (make the manifest read-only directory), `rename_refuses_collision_and_reserved_names`.

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement** (sorted keys: serialise through `BTreeMap`-ordered struct fields or `serde_json::Value` with `preserve_order` off for this module via a canonicalising helper)

- [ ] **Step 4: Run tests** — PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): audio manifest sidecar bound to media hash"`

---

### Task 16: Thumbnail cache keys and pruning

**Files:**
- Create: `crates/fthrust-core/src/thumbnails.rs`
- Test: `crates/fthrust-core/tests/thumbnails.rs`

**Interfaces:**
- Produces: `pub fn thumbnail_key(canonical_path: &Path, size: u64, mtime_ns: i128) -> String` = hex md5 of `"{path}|{size}|{mtime_ns}|aspect-v4"`; `pub struct MetaV3 { pub duration: f64, pub width: u32, pub height: u32, pub fps: f64, pub video_bitrate: u64, pub total_bitrate: u64 }` with `pub fn to_text(&self) -> String` (`"{duration:.6} {w} {h} {fps:.6} {vb} {tb}"`) and `pub fn parse(s: &str) -> Option<Self>`; `pub struct ThumbnailPaths { pub jpg, pub meta, pub failed }` from `pub fn paths_for(dir, key)`; `pub fn failed_marker_is_fresh(p: &Path, now: SystemTime) -> bool` (30 s); `pub fn temp_path_for(final: &Path) -> PathBuf` (`<final>.tmp-<pid>-<tid>`); `pub fn prune(dir: &Path, now: SystemTime, last_prune: &mut Option<SystemTime>) -> Result<()>`: no-op within 30 s of last prune; group files by stem before `.meta-`; evict oldest groups by mtime until total ≤ 512 MiB and ≤ 8192 entries.

- [ ] **Step 1: Write the failing tests**

`key_matches_python_md5` (compute the expected hex in the test with the `md5` crate from the same string, and also assert a literal for `/a/b.mp4|10|20|aspect-v4`), `meta_round_trip_six_decimals`, `prune_evicts_oldest_groups_together`, `prune_rate_limited`, `failed_marker_expires`.

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement**

- [ ] **Step 4: Run tests** — PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): thumbnail cache keys, meta-v3, pruning"`

---

### Task 17: Window geometry

**Files:**
- Create: `crates/fthrust-core/src/window_geometry.rs`
- Test: `crates/fthrust-core/tests/window_geometry.rs`

**Interfaces:**
- Produces: `#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)] pub struct SavedGeometry { pub normal_geometry: Option<Rect>, pub screen_name: Option<String>, pub maximized: Option<bool>, pub saved_dpr: Option<f64>, pub saved_logical_dpi: Option<f64> }`, `pub struct Rect { pub x: i32, pub y: i32, pub width: u32, pub height: u32 }`, `pub struct Screen { pub name: String, pub available: Rect, pub primary: bool }`, `pub struct Placement { pub rect: Rect, pub screen_name: String, pub maximized: bool }`, `pub const MIN_WIDTH: u32 = 1100`, `MIN_HEIGHT: u32 = 720`, `pub fn restore(saved: &SavedGeometry, screens: &[Screen]) -> Option<Placement>` (`None` when `screens` empty), `pub fn popup_anchor(anchor: Rect, popup: (u32,u32), screen: Rect) -> (i32, i32)` (8 px margin, keeps inside the screen).

- [ ] **Step 1: Write the failing test**

`restore_cases_match_fixture` looping over `window-geometry.json`, plus `no_screens_is_none`, `default_is_maximized`.

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement**

- [ ] **Step 4: Run tests** — PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): window geometry persistence and restore"`

---

### Task 18: Capture config limits, save state machine, health, recovery tracker

**Files:**
- Create: `crates/fthrust-core/src/capture_config.rs`, `crates/fthrust-core/src/save_state.rs`, `crates/fthrust-core/src/capture_health.rs`, `crates/fthrust-core/src/recovery_tracker.rs`
- Test: `crates/fthrust-core/tests/capture_config.rs`, `tests/save_state.rs`, `tests/capture_health.rs`, `tests/recovery_tracker.rs`

**Interfaces:**
- Produces:
  - `pub const CLIP_LENGTHS: [u32; 11]` (5,10,15,30,45,60,90,120,180,240,300), `FPS_VALUES: [u32; 8]`, `pub fn validate_fps(v) -> Result<u32>` (15..=240), `validate_clip_length(v)` (5..=900), `validate_custom_bitrate_kbps(v)` (500..=200000), `validate_encoder_preset(v)` (1..=7); invalid values are errors, never clamped.
  - `#[derive(Clone, Debug, PartialEq)] pub struct CaptureConfig { pub fps: u32, pub buffer_seconds: u32, pub width: u32, pub height: u32, pub bitrate_kbps: u32, pub codec: Codec, pub preset: u8, pub monitor: String, pub scaling: Scaling, pub audio_enabled: bool, pub encoder: EncoderPref, pub microphone_endpoint_id: String, pub playback_endpoint_id: String, pub normal_clip_seconds: u32, pub crop: Option<CropRect> }`, enums `Codec { Auto, H264, Hevc, Av1 }`, `Scaling { Stretch, Fit }`, `EncoderPref { Auto, Vaapi, Nvenc, Software }`, `pub fn from_settings(s: &SettingsStore) -> Result<CaptureConfig>` (resolution strings `480p/720p/1080p/1440p/source` → width/height with 0 = source; bitrate level `low/medium/high/custom` → kbps table copied from `reference/FTHR_UI/core/capture_settings.py`), `pub fn compute_buffer_seconds(clip_length: u32, custom: &[u32]) -> u32` (= min(900, max(all)+2)), `pub fn ring_budget_mb(bitrate_kbps: u32, buffer_seconds: u32, ram_mb: u64) -> u64` (clamp 64..=16384 of min(expected*1.25+64, ram/2) where expected = bitrate_kbps*1000/8*(buffer+5)/1 MiB).
  - `pub struct ConfigTracker { state: ConfigState }`, `pub enum ConfigState { Active(CaptureConfig), Requested { active: Option<CaptureConfig>, requested: CaptureConfig }, Applying {..}, Failed { active: Option<CaptureConfig>, failed: CaptureConfig, reason: String } }` with `request(cfg)`, `applying()`, `applied()`, `failed(reason)`, `active(&self) -> Option<&CaptureConfig>` (a failed restart keeps the previous active config).
  - `pub struct SaveStateMachine { .. }`, `pub enum SaveState { Idle, Requested, WaitingForAck, EngineAccepted, Completed(PathBuf), Failed(String), TimeoutNotice }`, `pub enum EngineSaveEvent { SaveStarted, ClipSaved(PathBuf), SaveFailed(String) }`, methods `submit(now: Instant) -> bool` (false when busy), `is_busy()`, `on_event(ev, now) -> SaveState`, `tick(now) -> SaveState` (ack timeout 1.0 s → `Failed("no acknowledgement")`; completion timeout 60 s → `TimeoutNotice`, after which a late `ClipSaved` still yields `Completed`), `take_terminal() -> Option<SaveState>`.
  - `pub mod flags { pub const ACTIVE: u32 = 1; RECOVERING = 2; BACKEND_FAILED = 4; CONTENT_SUSPECT = 8; PAUSED = 16; }`, `pub enum HealthState { Initializing, Healthy, Degraded, ContentSuspect, Stalled, Failed, Recovering, Stopped }`, `pub struct HealthSnapshot { pub state, pub previous_state, pub reason: &'static str, pub changed: bool, pub request_recovery: bool, pub seconds_since_progress: f64, pub fresh_buffer_seconds: f64, pub paused: bool }` with `pub fn save_allowed(&self) -> bool`, `pub struct CaptureHealthMonitor` with `pub fn new(warning_after_s: f64 /*3.0*/, stalled_after_s: f64 /*8.0*/, budget_reset_s: f64 /*30.0*/) -> Self` and `pub fn observe(&mut self, connected: bool, frame_count: u64, flags: u32, generation: u64, now_s: f64) -> HealthSnapshot`, `pub fn evaluate_save_admission(snap: &HealthSnapshot, seconds: u32) -> std::result::Result<(), &'static str>` (reasons `"Capture is recovering…"` / `"Capture is not receiving new frames…"` copied verbatim from `reference/FTHR_UI/core/capture_health.py:381-401`), `pub struct ContentSuspicionTracker` with `pub fn observe_luma(&mut self, mean: f64, variance: f64) -> bool` (black-like mean ≤ 8 and var ≤ 6, uniform var ≤ 2, suspect after 12 consecutive).
  - `pub enum RecoveryKind { EngineLineage, EncoderInit, CaptureBackend }`, `pub struct RecoveryTracker` with `pub fn track(&mut self, key: &str, kind: RecoveryKind, process_gen: u64, backend_gen: u64)`, `pub fn observe(&mut self, snap: &HealthSnapshot, process_gen: u64, backend_gen: u64, hardware_active: bool, now_s: f64) -> Vec<String>` returning keys recovered: needs 3 progressing polls spanning ≥ 3.0 s in the same generation while `Healthy` (`ContentSuspect` accepted only for `CaptureBackend`); `EngineLineage` additionally needs `process_gen` newer than tracked; `EncoderInit` needs newer generation, `hardware_active` and `Healthy`; any non-healthy poll, generation change, frame-count decrease or disconnect resets the window.

- [ ] **Step 1: Write the failing tests**

config: `buffer_seconds_cases_match_fixture`, `ring_budget_cases_match_fixture`, `invalid_fps_is_error_not_clamped`, `every_clip_length_fits_ring` (for each `CLIP_LENGTHS`, `compute_buffer_seconds(len, &[]) <= 900`), `failed_restart_keeps_previous_active_config`.
save_state: one test per T1..T6 from `save-state.json` driving `on_event`/`tick` with the fixture's timestamps and asserting each expected state; `busy_rejects_second_submit`; `late_success_after_timeout_notice_is_completed`.
health: `observe_sequences_match_fixture`, `stall_requests_recovery_once_per_incident`, `budget_restored_after_30s_healthy`, `new_generation_rebaselines`, `backend_failed_is_failed_and_blocks_save`, `paused_is_degraded_with_paused_flag`, `content_suspect_needs_fresh_frames`, `suspicion_after_12_black_samples`.
recovery: `needs_three_polls_over_three_seconds`, `non_healthy_poll_resets_window`, `generation_change_resets`, `frame_count_decrease_resets`, `content_suspect_counts_only_for_backend_kind`, `engine_lineage_needs_newer_process_generation`.

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement the four modules**

- [ ] **Step 4: Run tests** — `cargo test -p fthrust-core --test capture_config --test save_state --test capture_health --test recovery_tracker` PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): capture config limits, save state machine, health monitor, recovery tracker"`

---

### Task 19: Diagnostics: redacting logs, crash log, field sessions

**Files:**
- Create: `crates/fthrust-core/src/diagnostics/mod.rs`, `diagnostics/redact.rs`, `diagnostics/logging.rs`, `diagnostics/session.rs`
- Test: `crates/fthrust-core/tests/diagnostics.rs`

**Interfaces:**
- Produces:
  - `pub fn redact(line: &str) -> String`: bearer tokens, `Authorization`/`X-Auth-*` header values, `token|api_key|apikey|password|passwd|secret|userhash|auth` `=`/`:` pairs, URL credentials → `[REDACTED]`, matching the fixture.
  - `pub fn init_logging(paths: &FthrPaths, app_version: &str) -> Result<LogGuard>`: `logs/fthr.log` with rotation at 2 MiB keeping `.1` and `.2`, line format `YYYY-MM-DD HH:MM:SS LEVEL    [thread] target: message` (level padded to 8), every line passed through `redact`, session banner `===== FTHR Clips <ver> started <ts> (linux) =====`, and stderr mirrored. `LogGuard` flushes on drop.
  - `pub fn init_crash_log(paths) -> Result<()>`: rotates `fthr-crash.log` to `.old` when > 1 MiB, writes banner `===== started <ts> pid N =====`, installs a signal handler for SIGSEGV/SIGABRT/SIGBUS/SIGFPE/SIGILL that writes the signal name and a backtrace (via `std::backtrace` captured in a panic hook plus `nix` signal handling) to that file.
  - `pub struct DiagnosticsSession { dir: PathBuf, .. }` with `pub fn start(paths, app_version) -> Result<Self>` (dir `session-<%Y%m%dT%H%M%SZ>-<8 hex>`, writes `active-session.json` marker, prunes to 5 sessions), `pub fn event(&self, category: &str, name: &str, fields: serde_json::Value)` (appends one JSON line `{ts, category, name, ...fields}` to `events.jsonl`, stops appending past 4 MiB), `pub fn engine_log_path(&self) -> PathBuf`, `pub fn write_summary(&self, summary: &serde_json::Value) -> Result<()>`, `pub fn export_zip(&self, dest: &Path, app_log: &Path) -> Result<()>` (adds `ui.log` = last 1 MiB of `fthr.log`, and `README.txt`).

- [ ] **Step 1: Write the failing tests**

`redaction_cases_match_fixture`, `log_line_format_and_banner`, `log_rotates_at_2mib_keeping_two`, `crash_log_rotates_over_1mib`, `session_dir_name_and_marker`, `events_jsonl_appends_and_caps`, `keeps_five_sessions`, `export_zip_contains_ui_log_tail_and_readme`.

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement** (tracing-subscriber `fmt` layer with a custom `MakeWriter` that redacts and rotates)

- [ ] **Step 4: Run tests** — PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): redacting logs, crash log, diagnostics sessions"`

---

### Task 20: Crate surface, docs, CI green, CHANGES

**Files:**
- Modify: `crates/fthrust-core/src/lib.rs`, `README.md`, `CHANGES.md`, `docs/superpowers/plans/2026-10-22-fthrust-roadmap.md`

- [ ] **Step 1: Finalise `lib.rs`**

`pub mod` for every module above, crate-level doc listing the `~/.fthr` files each module owns, `#![deny(missing_docs)]` with a one-line doc on every public item, `#![forbid(unsafe_code)]`.

- [ ] **Step 2: Record fixes**

`CHANGES.md` `## Fixed` entries: "Linux hotkeys.json loader drops non-string bindings instead of crashing", "Theme loader replaces invalid colour strings instead of propagating them", "Clip metadata store surfaces a locked database instead of falling back to stale JSON", "Settings file that is valid JSON but not an object is quarantined". Mark the roadmap row for plan 1 as shipped.

- [ ] **Step 3: Verify everything**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace && cargo deny check && .venv-reference/bin/python tools/gen-reference-fixtures.py && git diff --exit-code crates/fthrust-core/tests/fixtures`
Expected: all succeed; fixture regeneration produces no diff.

- [ ] **Step 4: Commit and push**

```bash
git add -A && git commit -m "chore(core): finalise crate surface and record fixes" && git push origin main
```
Expected: GitHub Actions run on `kalopsia-space/FTHRust` is green.
