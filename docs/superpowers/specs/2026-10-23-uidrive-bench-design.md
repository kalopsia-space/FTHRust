# UI drive bench: design

A test bench that lets the assistant operate the FTHRust UI unattended, on this
machine and in CI, without touching the user's desktop session.

## Intent

Plan 5 ended with a manual run that found four real bugs no unit test had
caught, and every attempt to check the UI by hand risked opening windows or
stealing focus on the user's main monitor. Plan 6 (the clip editor) is the most
interactive part of the rewrite. The assistant needs to start the app, look at
it, click and type in it, read what it shows, and loop on failures by itself.

## Goals

- **Hermetic.** The app runs inside a headless compositor on a private Wayland
  socket, with a private runtime directory and a scratch home. The user's
  compositor, keyboard, mouse, focus, screens, config, sockets and sounds are
  never involved. The user can forget the bench is running.
- **Operable by the assistant.** One command-line tool gives screenshots (PNG
  the assistant can view), pointer moves, clicks and drags, key chords, typed
  text, waits on screen state, and widget positions by name.
- **Scriptable.** Scenarios are shell scripts calling that tool; exit codes
  and saved screenshots make a failing step diagnosable without a person.
- **Reusable in CI.** The same bench runs on GitHub Actions with the software
  renderer, so a scenario failure blocks a merge.

## Non-goals and anti-goals

- Not a product feature. Nothing in the bench ships to users; the dump hook
  the app gains is inert unless a test-only environment variable is set, and
  the marker names the bench clicks are the fidelity names the UI already
  records (adding a name to a widget that lacks one is allowed and changes
  nothing visible).
- Not window management. Under cage the window is full-screen; the custom
  title bar's close, maximise and drag controls are not exercised by the bench
  (they are covered by the shell tests), and the geometry the app saves goes
  to the scratch home.
- Not pixel assertions. The fidelity suite already holds the screens to the Qt
  reference; the bench asserts on state (which widgets exist, what text shows,
  which page is open, which files appear), with screenshots for the assistant's
  eyes.
- Not Hyprland automation. No `hyprctl`, no `ydotool`/uinput, no `wtype` on the
  user's socket, nothing that reads or writes the user's `~/.config`.
- Anti-goal: a bench that silently falls back to the user's session when the
  headless compositor fails to start. It must refuse and say why.

## Constraints

- Machine: Arch Linux, Hyprland 0.56 (the user's session, on `wayland-1`),
  `cage` 0.3.1 (wlroots), `grim`, `wayland-info` installed. Vulkan works on
  the GPUs; CI uses lavapipe.
- Verified by spike on 2026-10-23: `WLR_BACKENDS=headless WLR_RENDERER=pixman
  WLR_LIBINPUT_NO_DEVICES=1 cage -- <app>` starts in under a second on the next
  free socket (`wayland-0` here) and advertises `xdg_wm_base` 5,
  `zwlr_screencopy_manager_v1` 3, `zwp_virtual_keyboard_manager_v1` 1,
  `zwlr_virtual_pointer_manager_v1` 2, `zwlr_foreign_toplevel_manager_v1` 3 and
  `wl_output` 4. It does not offer `zwlr_layer_shell_v1` or
  `ext_image_copy_capture_v1`.
- The app's sockets live under `$XDG_RUNTIME_DIR/fthrust/` (`runtime_dir()`
  honours that variable alone) and the single-instance lock under the home's
  `.fthr/`; a bench run must not collide with a real instance, so the bench
  gets its own runtime directory and home. The app's warning about a running
  Python engine (`/dev/shm/FTHR_SharedMemory_v4`) is only a log line.
- The synthetic engine backend exists (`FTHRUST_SYNTHETIC_BACKEND=1` with
  `FTHR_CAPTURE_BACKEND=synthetic`, used by the engine round-trip test): status
  reaches `CAPTURING` and saves produce real MP4s without a screen, a GPU or
  PipeWire. `--engine real` uses `FTHR_CAPTURE_BACKEND=screencopy` against
  cage's virtual output (cage offers `zwlr_screencopy`) and is best effort:
  without PipeWire the audio sources report unavailable and clips are video
  only.
- cage draws one application full-screen on its (virtual) output; the output
  size is set by the bench (default 1920×1080), so the app window is that size
  and `maximized`.

## How it is made (decided)

### Layout

`tools/uidrive/` is its own Cargo workspace (not a member of the rewrite's
workspace), with one binary crate `uidrive`, a `bench.sh` launcher and a
`scenarios/` directory. It depends on `wayland-client`, `wayland-protocols-wlr`,
`wayland-protocols-misc` (virtual keyboard), `xkbcommon` (keymap for the virtual
keyboard), `serde_json`, `image`, `clap`. It does not depend on any FTHRust
crate; it talks to the app only through files and the compositor.

### The bench launcher (`bench.sh`)

`bench.sh up [--release] [--engine synthetic|real] [--size WxH]`:

1. Creates `$BENCH` (default `/tmp/uidrive-<pid>`): `home/` (scratch `HOME`
   with `.fthr/settings.json` seeded from `--settings <file>` or a default
   that sets `notification_sounds_enabled=false` and
   `clips_directory=$BENCH/clips`, so scenarios know where files land), `rt/`
   (private `XDG_RUNTIME_DIR`, mode 0700), `clips/`, `shots/`, `logs/`.
   `--app <path>` names the app binary (default `target/debug/fthrust` of the
   rewrite checkout; the engine is found next to it as usual).
2. Starts `cage` headless with the pixman renderer, output size from `--size`,
   with the app as cage's application:
   `env -i HOME=$BENCH/home XDG_RUNTIME_DIR=$BENCH/rt PATH=... FTHRUST_UI_DUMP=$BENCH/ui.json
   FTHRUST_SYNTHETIC_BACKEND=<0|1> FTHR_CAPTURE_BACKEND=<synthetic|screencopy>
   RUST_LOG=info cage -- <app binary>`. Because cage inherits the private
   runtime directory, its socket (`wayland-0` under `$BENCH/rt`) is private
   too, and the app, grim and the driver all find it there. No symlinks into
   the user's runtime directory.
3. Optionally runs a private `dbus-daemon --session` and exports its address,
   so portal calls fail fast and deterministically instead of reaching the
   user's portal. Without it the app sees no session bus.
4. PipeWire: the private runtime directory has no PipeWire socket, so the
   engine's audio sources report unavailable and the sound player uses its
   null sink. This is deliberate; audio is not in scope for the bench.
5. Waits until `$BENCH/ui.json` has been written at least once (the app is up
   and painting), with a 30 s timeout; prints `BENCH=<dir>` and the socket
   name. `bench.sh down` kills the app and cage and keeps `shots/` and
   `logs/`; `bench.sh run <scenario.sh>` does up, scenario, down and prints
   the scenario's exit code.

### The app hook (`FTHRUST_UI_DUMP`)

When the variable names a file, the app writes it atomically (temp file plus
rename) at the end of every painted frame, throttled to at most 20 writes per
second and skipped when nothing changed. Content (JSON):

```json
{"frame": 123, "time": 12.5, "pixels_per_point": 1.0, "page": "clips",
 "status": "CAPTURING",
 "markers": [{"name": "QPushButton[Record]", "x": 12.0, "y": 8.0, "w": 96.0, "h": 36.0,
              "text": "Record"}, ...],
 "error_bar": {"title": "...", "detail": "..."} , "dialog": "Delete clip?"}
```

`markers` are the fidelity markers the UI already records (name, rect,
optional text; 101 registration sites today, including every button,
combo item, tab, dialog and the error bar), in logical pixels. The driver
checks `pixels_per_point` is 1.0 and refuses otherwise. The hook runs at the
end of `App::ui`, where the page, status text, error-bar entry and front
dialog are all on `AppState` and the pass's markers are taken from the
fidelity registry; "changed" means the JSON without `frame` and `time`
hashes differently. Every state change the app shows arrives through an
event that requests a repaint, so a painted frame (and a dump) follows it;
`wait` commands still carry a timeout. Widgets the scenarios need that have
no name yet (clip cards, for instance, as `clipCard[<file name>]`) get one as
part of the bench work; names must be unique per screen, and `find` offers
`--nth` for repeated names. The hook is behind the environment variable only
(no cargo feature).

### The driver (`uidrive`)

Reads `BENCH` from the environment (set by `bench.sh`) to find the socket, the
dump file, the screenshot folder and the logs. Commands, each exiting 0 on
success and 1 with a message on failure:

- `shot [name]` — captures the output with grim into `shots/<n>-<name>.png`
  and prints the path.
- `find <marker>` — prints the first marker whose name equals, or whose name
  starts with, the argument as `x y w h text`; `--all` lists every match.
- `click <marker | x,y> [--button right|middle] [--double]`,
  `move <marker | x,y>`, `drag <from> <to> [--steps N]` — virtual pointer
  (`motion_absolute` against the extent read from the bench output's
  `wl_output` mode), with a 30 ms settle between press and release. cage gives
  its application focus once mapped; the driver's first input waits for the
  dump file to exist, which implies a painted, mapped window.
- `key <chord>` (e.g. `Ctrl+Shift+S`, `F9`, `Escape`), `type <text>` — virtual
  keyboard with a US keymap from xkbcommon; `type` sends each character with
  the right modifiers.
- `wait <marker> [--timeout 10] [--gone]`, `wait-text <substring>`,
  `wait-status <text>` — poll the dump file; print what matched.
- `state` — the dump's page, status, error bar, dialog and marker count;
  `state --json` the whole file.
- `log [--engine] [-n 40]` — the tails of the app and engine logs.
- `hotkey <action>` — runs the app's own `--hotkey` client against the bench
  runtime directory (same as a compositor bind would).

### Scenarios

Shell scripts in `tools/uidrive/scenarios/`, each self-contained:
`smoke.sh` (app up, status reaches `CAPTURING`, open each Settings tab, open and
close each toolbar popup, screenshot each), `save_clip.sh` (hotkey, clip file
appears, Clips page shows one card), `record.sh` (Record button, stop, file
appears). Plan 6 adds editor scenarios. A scenario ends by printing `PASS` or
`FAIL <step>` and leaves its screenshots in `shots/`.

### CI

A `uidrive` job on GitHub Actions (Ubuntu runner): `apt-get install cage grim
mesa-vulkan-drivers libxkbcommon-dev` plus the rewrite's existing build
packages, builds the app and the driver, runs `smoke.sh` and `save_clip.sh`
with the synthetic engine, uploads `shots/` and `logs/` as artifacts when a
scenario fails. The app renders with wgpu on Vulkan through lavapipe, whose
Wayland presentation uses `wl_shm` buffers, which cage's pixman renderer
accepts. The driver's dependency versions are pinned in its own `Cargo.toml`
and the virtual keyboard's keymap is built with xkbcommon and handed over
through a memfd.

## Left to the builder

- Exact marker-name matching rules and the dump's throttle implementation.
- How `type` handles characters outside the US keymap (skip with a warning is
  acceptable).
- Whether `drag` needs intermediate motion events for egui (likely yes; pick a
  step count that egui's drag detection accepts).
- Timeouts and retry counts per command.
- The choice between `clap` subcommands and a tiny hand-rolled parser.
