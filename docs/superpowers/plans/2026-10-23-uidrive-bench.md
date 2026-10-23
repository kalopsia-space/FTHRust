# UI Drive Bench Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A headless bench (`cage` + a `uidrive` CLI + scenario scripts) that lets the assistant start, see and operate the FTHRust UI without touching the user's session, here and in CI.

**Architecture:** The app gains one inert hook: with `FTHRUST_UI_DUMP` set it writes its page, status, error bar, dialog and every fidelity marker to a JSON file after each painted frame. `tools/uidrive/` is a separate Cargo workspace whose binary talks to the bench's private Wayland socket through the virtual pointer and virtual keyboard protocols, screenshots with `grim`, and reads the dump for widget positions and waits. `bench.sh` builds the private environment and starts `cage` headless with the app.

**Tech Stack:** Rust 2024 (stable 1.98), `wayland-client` 0.31.15, `wayland-protocols-wlr` 0.3.12, `wayland-protocols-misc` 0.3.12, `xkbcommon` 0.8, `clap` 4, `serde_json` 1, `cage` 0.3.1 (wlroots headless + pixman), `grim`, bash.

**Spec:** `docs/superpowers/specs/2026-10-23-uidrive-bench-design.md`

## Global Constraints

- The bench never reads the user's `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, `HOME` or `DBUS_SESSION_BUS_ADDRESS`: the app and cage run under `env -i` with only the bench's variables; a start failure exits non-zero with a message and never falls back.
- No `hyprctl`, `ydotool`, uinput or `wtype`; input goes only through `zwlr_virtual_pointer_v1` and `zwp_virtual_keyboard_v1` on the bench socket.
- `tools/uidrive/` is its own workspace and depends on no FTHRust crate; it is not a member of the root workspace.
- The app hook is behind the `FTHRUST_UI_DUMP` environment variable only (no cargo feature); dumps are atomic (temp file + rename), at most 20 per second, skipped when unchanged.
- Dump coordinates are logical pixels with `pixels_per_point` 1.0; the driver refuses any other value.
- Marker names are the existing fidelity names; clip cards are already `clipCard[<index>]` with the file name as their text.
- Commands exit 0 on success and 1 with a one-line message on failure.

## Review Focus

1. A scenario reads the dump while the app is mid-write: the reader must never see a truncated JSON (atomic rename). Test in Task 1.
2. `wait <marker>` right after an action must not be satisfied by the frame painted before the action: `wait` compares `frame` against the value it saw when it started. Test in Task 2.
3. A bench output that is not 1920×1080 (CI may differ): absolute pointer motion uses the extent read from `wl_output`, never a constant. Test in Task 3.
4. A chord the driver does not know (`Ctrl+Bogus`) must fail with exit 1 and a message, not send nothing and succeed. Test in Task 2.
5. cage missing or failing to start: `bench.sh up` exits 1 naming the cause and leaves no app process behind. Test in Task 4.

---

## File structure

```
crates/fthrust-ui/src/ui_dump.rs          snapshot() and Writer (atomic, throttled, change-skipping)
crates/fthrust-ui/src/lib.rs              call the hook at the end of App::ui
crates/fthrust-ui/src/dialogs/mod.rs      title_of made pub
crates/fthrust-ui/tests/ui_dump.rs
tools/uidrive/Cargo.toml                  separate workspace, pinned deps
tools/uidrive/src/main.rs                 clap subcommands, exit codes
tools/uidrive/src/bench.rs                Bench::from_env (dir, socket, dump, shots, logs, app)
tools/uidrive/src/dump.rs                 Dump parse, find/--nth, wait helpers
tools/uidrive/src/keys.rs                 chord parsing, US keymap char→(keycode, shift)
tools/uidrive/src/wl.rs                   Session: outputs extent, virtual pointer/keyboard, grim
tools/uidrive/tests/{dump,keys}.rs
tools/uidrive/bench.sh                    up | down | run
tools/uidrive/scenarios/{selftest,smoke,save_clip,record}.sh
.github/workflows/ci.yml                  uidrive job
docs/superpowers/specs/...                (unchanged)
```

---

### Task 1: App dump hook

**Files:**
- Create: `crates/fthrust-ui/src/ui_dump.rs`
- Modify: `crates/fthrust-ui/src/lib.rs` (`pub mod ui_dump;`, call in `App::ui`), `crates/fthrust-ui/src/dialogs/mod.rs:153` (`pub fn title_of`)
- Test: `crates/fthrust-ui/tests/ui_dump.rs`

**Interfaces (Produces):**
- `ui_dump::snapshot(state: &AppState, markers: &[fidelity::Marker], pixels_per_point: f32, frame: u64, time: f64) -> serde_json::Value` with keys `frame`, `time`, `pixels_per_point`, `page` (`"clips"` | `"settings"`), `status` (`state.status.text`), `markers` (array of `{name, x, y, w, h, text?}`; `text` is `marker.text.content` when present), `error_bar` (`{title, detail, level}` from `state.error_bar.current()` or `null`), `dialog` (`dialogs::title_of(front modal)` or `null`).
- `ui_dump::Writer::new(path: PathBuf) -> Writer`; `Writer::write(&mut self, value: &Value) -> io::Result<bool>`: returns `Ok(false)` without writing when the value minus `frame`/`time` hashes equal to the last written one, or when less than 50 ms passed since the last write; otherwise writes `<path>.tmp` and renames.
- `ui_dump::from_env() -> Option<Writer>` (`FTHRUST_UI_DUMP`).
- `App` holds `dump: Option<Writer>` created in `App::new`; at the end of `App::ui` (after `shell::ui`), when `dump` is `Some`, it takes `fidelity::take(ctx)` and writes `snapshot(..)` with `ctx.cumulative_pass_nr()` and `ctx.input(|i| i.time)`.

- [ ] Step 1: Write the failing tests in `crates/fthrust-ui/tests/ui_dump.rs`:
  - `snapshot_carries_page_status_markers_and_dialog`: `AppState::for_tests`, `state.page = Page::Settings`, `state.status.text = "CAPTURING"`, `state.error_bar.push("CLIP WAS NOT SAVED", "disk full", Level::Error, vec![], None, true)`, `state.dialogs.push_modal(DialogKind::Message{title:"Hello", message:"x"})`; markers = `[Marker{name:"QPushButton[Record]", rect:(12,8,96,36), text: Some(run "Record")}]`; assert `v["page"]=="settings"`, `v["status"]=="CAPTURING"`, `v["markers"][0]["name"]=="QPushButton[Record]"`, `v["markers"][0]["w"]==96.0`, `v["markers"][0]["text"]=="Record"`, `v["error_bar"]["title"]=="CLIP WAS NOT SAVED"`, `v["dialog"]=="Hello"`, `v["pixels_per_point"]==1.0`.
  - `writer_is_atomic_and_skips_unchanged`: `Writer::new(tmp.join("ui.json"))`; `write(&v1)` → `Ok(true)`, file parses; no `.tmp` file remains; `write(&v1 with frame+1, time+1)` → `Ok(false)`; a changed status after 60 ms → `Ok(true)`; a second changed value within 10 ms → `Ok(false)` (throttle).
  - `from_env_is_none_without_variable`: with the variable unset → `None`; set to a temp path → `Some`.
- [ ] Step 2: Run `cargo test -p fthrust-ui --test ui_dump` → FAIL: unresolved import `fthrust_ui::ui_dump`.
- [ ] Step 3: Implement `ui_dump.rs` per the Interfaces block; make `dialogs::title_of` `pub`; add the `dump` field and the call in `App::ui`.
- [ ] Step 4: Run the test file → PASS; `cargo clippy -q -p fthrust-ui --all-targets -- -D warnings` clean; `cargo test -p fthrust-ui -q` green.
- [ ] Step 5: Commit `feat(ui): FTHRUST_UI_DUMP state dump for the drive bench`.

---

### Task 2: uidrive crate: bench env, dump reading, chords

**Files:**
- Create: `tools/uidrive/Cargo.toml` (`[workspace]` with no members + `[package] name = "uidrive"`, edition 2024, deps pinned: `wayland-client = "=0.31.15"`, `wayland-protocols-wlr = { version = "=0.3.12", features = ["client"] }`, `wayland-protocols-misc = { version = "=0.3.12", features = ["client"] }`, `wayland-protocols = { version = "=0.32.13", features = ["client"] }`, `xkbcommon = "0.8"`, `clap = { version = "4", features = ["derive"] }`, `serde_json = "1"`, `serde = { version = "1", features = ["derive"] }`), `tools/uidrive/src/{main,bench,dump,keys}.rs`, `tools/uidrive/.gitignore` (`target/`)
- Test: `tools/uidrive/tests/dump.rs`, `tools/uidrive/tests/keys.rs`

**Interfaces (Produces):**
- `bench::Bench { dir: PathBuf, socket: String, runtime_dir: PathBuf, dump: PathBuf, shots: PathBuf, logs: PathBuf }`; `Bench::from_env() -> Result<Bench, String>` reads `BENCH` (dir) and `BENCH_SOCKET` (socket name, default `wayland-0`); `runtime_dir = dir/rt`, `dump = dir/ui.json`, `shots = dir/shots`, `logs = dir/logs`.
- `dump::Dump { frame: u64, time: f64, pixels_per_point: f32, page: String, status: String, markers: Vec<Marker>, error_bar: Option<ErrorBar>, dialog: Option<String> }` (serde); `Marker { name, x, y, w, h, text: Option<String> }` with `fn center(&self) -> (f64, f64)`.
- `dump::read(path: &Path) -> Result<Dump, String>` (refuses `pixels_per_point != 1.0`); `dump::find<'a>(d: &'a Dump, query: &str, nth: usize) -> Option<&'a Marker>`: matches `name == query`, then `text == query`, then `name.starts_with(query)`, in that order, `nth` zero-based among the matches of the first non-empty tier.
- `dump::wait(path, pred: impl Fn(&Dump) -> bool, timeout: Duration, after_frame: Option<u64>) -> Result<Dump, String>`: polls every 50 ms, ignores dumps whose `frame <= after_frame`, errors `"timed out after Ns waiting for ..."`.
- `keys::Chord { mods: Mods { ctrl, shift, alt, logo }, keysym: u32, keycode: u32 }`; `keys::parse_chord(s: &str) -> Result<Chord, String>` accepting `Ctrl|Control`, `Shift`, `Alt`, `Super|Logo|Win`, keys `A`–`Z`, `0`–`9`, `F1`–`F24`, `Escape`, `Enter|Return`, `Tab`, `Space`, `Backspace`, `Delete`, `Left|Right|Up|Down`, `Home|End|PageUp|PageDown`, `Minus|Equal|Comma|Period|Slash`; unknown → `Err("unknown key 'Bogus'")`.
- `keys::char_key(c: char) -> Option<(u32 /*evdev keycode*/, bool /*shift*/)>` for printable ASCII on the US layout.
- `main.rs`: clap subcommands `shot`, `find`, `click`, `move`, `drag`, `key`, `type`, `wait`, `wait-text`, `wait-status`, `state`, `log`, `hotkey` (Task 3 fills the Wayland ones; this task wires `find`, `state`, `wait*`, `log`, `hotkey`). `hotkey <action>` runs `BENCH_APP --hotkey <action>` with `XDG_RUNTIME_DIR=<runtime_dir>` and `HOME=<dir>/home` (`BENCH_APP` from the environment).

- [ ] Step 1: Write failing tests: `tests/dump.rs` — `find_prefers_exact_then_text_then_prefix` (three markers `QPushButton[Record]`, `clipCard[0]` text `a.mp4`, `clipCard[1]` text `b.mp4`: `find("clipCard[1]",0)` → index 2; `find("b.mp4",0)` → index 2; `find("clipCard",1)` → index 2; `find("nope",0)` → None), `read_refuses_scaled_dumps` (`pixels_per_point` 2.0 → Err containing "pixels_per_point"), `wait_ignores_frames_before_the_action` (write a dump with frame 5 matching the predicate; `wait(.., after_frame: Some(5), timeout 300ms)` → Err timed out; write frame 6 → Ok). `tests/keys.rs` — `parse_chord_examples` (`Ctrl+Shift+S` → ctrl, shift, keysym `XKB_KEY_s`; `F9`; `Escape`; `super+Return`), `unknown_key_is_an_error` (`Ctrl+Bogus` → Err "unknown key 'Bogus'"), `char_key_us` (`'a'` → (30,false), `'A'` → (30,true), `'!'` → (2,true), `' '` → (57,false)).
- [ ] Step 2: Run `cd tools/uidrive && cargo test` → FAIL (crate does not exist / unresolved).
- [ ] Step 3: Implement `Cargo.toml`, `bench.rs`, `dump.rs`, `keys.rs` and the non-Wayland subcommands in `main.rs` (the Wayland ones print "not implemented" and exit 1 for now).
- [ ] Step 4: `cargo test` → PASS; `cargo clippy --all-targets -- -D warnings` clean.
- [ ] Step 5: Commit `feat(uidrive): bench env, dump reader, chords`.

---

### Task 3: Wayland session: input and screenshots

**Files:**
- Create: `tools/uidrive/src/wl.rs`
- Modify: `tools/uidrive/src/main.rs` (wire `shot`, `click`, `move`, `drag`, `key`, `type`)
- Test: `tools/uidrive/tests/wl_live.rs` (ignored unless `UIDRIVE_LIVE=1`)

**Interfaces:**
- Consumes: `Bench`, `Dump`, `find`, `Chord`, `char_key` from Task 2.
- Produces: `wl::Session::connect(bench: &Bench) -> Result<Session, String>` (connects to `runtime_dir/socket`, binds `wl_seat`, every `wl_output` (keeps the first's current mode as `extent: (u32, u32)`), `zwlr_virtual_pointer_manager_v1`, `zwp_virtual_keyboard_manager_v1`; creates the pointer and the keyboard with a US keymap from `xkbcommon::xkb::Keymap::new_from_names(.., "us", ..)` serialised with `get_as_string` and passed through a memfd); `move_to(&mut self, x: f64, y: f64)` (`motion_absolute(time, x, y, extent.0, extent.1)` + `frame`), `click(&mut self, button: Button, double: bool)` (press, 30 ms, release; double repeats after 80 ms), `drag(&mut self, from: (f64,f64), to: (f64,f64), steps: usize)` (press at `from`, `steps` motions, release at `to`, 10 ms apart), `key(&mut self, chord: &Chord)` (modifiers via `modifiers(depressed, 0, 0, 0)` then key press/release, modifiers cleared), `type_text(&mut self, s: &str) -> Result<(), String>` (per char through `char_key`; unknown char → Err naming it); `shot(bench: &Bench, name: &str) -> Result<PathBuf, String>` (runs `grim -o <first output name> shots/<NNN>-<name>.png` with `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR` set to the bench's).
- `main.rs`: `click`/`move`/`drag` accept `<marker>` (resolved through the dump's `find`, centre of the rect) or `x,y`; `key`, `type`, `shot`.

- [ ] Step 1: Write `tests/wl_live.rs::live_session_moves_and_types` (`#[ignore]`-free but returns early unless `UIDRIVE_LIVE=1`): starts `cage` headless (`WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1`) under a temp `XDG_RUNTIME_DIR` with `wayland-info` as the application, discovers the socket, `Session::connect` succeeds, `extent.0 > 0 && extent.1 > 0` (the headless output's mode; its exact size is cage's choice), `move_to(10.0, 10.0)`, `click(Left, false)`, `key(&parse_chord("Escape")?)`, `type_text("ab")` all return Ok; then kills cage.
- [ ] Step 2: Run `UIDRIVE_LIVE=1 cargo test --test wl_live` → FAIL (no `wl` module).
- [ ] Step 3: Implement `wl.rs` and wire the subcommands.
- [ ] Step 4: Run the live test → PASS; `cargo clippy --all-targets -- -D warnings` clean.
- [ ] Step 5: Commit `feat(uidrive): virtual pointer and keyboard, screenshots`.

---

### Task 4: bench.sh, scenarios, CI

**Files:**
- Create: `tools/uidrive/bench.sh`, `tools/uidrive/scenarios/{selftest,smoke,save_clip,record}.sh`, `tools/uidrive/README.md`
- Modify: `.github/workflows/ci.yml` (job `uidrive`), `docs/capture-and-library.md` (one paragraph pointing at the bench)

**Interfaces:**
- `bench.sh up [--release] [--engine synthetic|real] [--size WxH] [--app PATH] [--settings FILE]` → prints `BENCH=<dir>` and `BENCH_SOCKET=<name>` lines (eval-able), exits 1 with a message when cage is missing, the app binary is missing, or the dump does not appear within 30 s (and kills what it started). `bench.sh down` reads `BENCH` and kills the app and cage. `bench.sh run <scenario.sh> [up-options]` does up, exports `BENCH`, `BENCH_SOCKET`, `BENCH_APP`, `PATH` with `tools/uidrive/target/debug`, runs the scenario, does down, exits with the scenario's code.
- Environment for cage and the app (`env -i`): `HOME=$BENCH/home`, `XDG_RUNTIME_DIR=$BENCH/rt`, `PATH`, `FTHRUST_UI_DUMP=$BENCH/ui.json`, `FTHRUST_SYNTHETIC_BACKEND=1 FTHR_CAPTURE_BACKEND=synthetic` (synthetic) or `FTHR_CAPTURE_BACKEND=screencopy` (real), `WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1`, `RUST_LOG=info`; seeded `$BENCH/home/.fthr/settings.json` = `{"notification_sounds_enabled": false, "clips_directory": "$BENCH/clips"}` merged with `--settings`.
- Scenarios print `PASS` or `FAIL <step>`: `selftest.sh` (status reaches `CAPTURING` within 20 s; `shot start`; `click` the Settings navigation marker; `wait` page `settings`; `key Escape`; `shot settings`); `smoke.sh` (selftest plus each Settings tab marker clicked and screenshotted, each toolbar popup opened and closed with `Escape`); `save_clip.sh` (`hotkey save_clip`; a `.mp4` appears under `$BENCH/clips` within 15 s; `wait clipCard[0]`); `record.sh` (`click QPushButton[Record]`; `wait-status "STARTING RECORDING"` or `"PREPARING RECORDING QUALITY"`; after 3 s `click QPushButton[Record]` again; a `FTHR_recording_*.mp4` appears within 20 s).
- CI job `uidrive` (Ubuntu): installs `cage grim libxkbcommon-dev` with the existing packages, builds `fthrust-ui` and `fthrust-engine` (debug), builds `tools/uidrive`, runs `bench.sh run scenarios/smoke.sh` and `scenarios/save_clip.sh`, uploads `$BENCH/shots` and `$BENCH/logs` with `actions/upload-artifact` on failure.

- [ ] Step 1: Write `bench.sh` with a `--cage PATH` override, then test the failure path: `tools/uidrive/bench.sh up --cage /nonexistent` → exit 1, message contains `cage`, `pgrep -f "$BENCH"` finds nothing.
- [ ] Step 2: Write `scenarios/selftest.sh`; run `tools/uidrive/bench.sh run tools/uidrive/scenarios/selftest.sh` → `PASS` (the marker names for the navigation and tabs are read from the dump with `uidrive state --json` on the first run and pinned in the script).
- [ ] Step 3: Write `smoke.sh`, `save_clip.sh`, `record.sh`; run each through `bench.sh run` → `PASS` each.
- [ ] Step 4: Add the CI job and the README (how to run, how to read a failure, the env rules).
- [ ] Step 5: Commit `feat(uidrive): bench launcher, scenarios, CI job`.
