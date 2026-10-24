# uidrive: the UI drive bench

Runs the FTHRust UI inside a headless `cage` compositor on a private socket,
with a scratch home and runtime directory, and drives it from the command
line: screenshots, clicks, drags, keys, typed text, waits on screen state,
widget positions by name. Nothing here touches the user's session: no real
input devices, no window on any monitor, no reads of the user's
`WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR` or `HOME`.

Design: `docs/superpowers/specs/2026-10-23-uidrive-bench-design.md`.

## Run

```sh
cargo build -p fthrust-ui -p fthrust-engine        # the app and its engine
tools/uidrive/bench.sh run tools/uidrive/scenarios/smoke.sh
```

`bench.sh run` starts the bench, runs the scenario with `uidrive` on the
`PATH`, stops the bench and exits with the scenario's code. Screenshots and
logs stay under the bench directory it prints. For an interactive session:

```sh
eval "$(tools/uidrive/bench.sh up)"; export BENCH BENCH_SOCKET BENCH_APP
export PATH=$PWD/tools/uidrive/target/debug:$PATH
uidrive wait-status CAPTURING; uidrive shot; uidrive click settingsGearBtn
tools/uidrive/bench.sh down
```

Options for `up`: `--release`, `--engine synthetic|real`, `--size WxH` (the
synthetic frame size; cage's headless output is 1280×720), `--app PATH`,
`--settings FILE` (merged over the seeded settings), `--cage PATH`,
`--renderer auto|gles2|vulkan|pixman` (cage's renderer; `auto` is gles2 when a
GPU render node exists so the app's Vulkan driver gets dmabuf buffers, else
pixman for CI with lavapipe), `--pipewire` (a private PipeWire and
WirePlumber on the bench's runtime directory and bus, so desktop audio comes
up and the "DESKTOP AUDIO UNAVAILABLE" notice goes away), `--seed DIR` (copied
into the clips folder, timestamps kept, before the app starts).

## Commands

| Command | Does |
| --- | --- |
| `shot [name]` | grim → `shots/NNN-name.png`, prints the path |
| `find QUERY [--nth N] [--all]` | `x y w h text` of a marker (exact name, then exact text, then name prefix) |
| `click TARGET [--button right] [--double] [--nth N]`, `move`, `drag FROM TO [--steps N]` | virtual pointer; TARGET is a marker or `x,y` |
| `scroll TARGET DY` | wheel over a marker or `x,y` (positive DY scrolls down) |
| `key CHORD`, `type TEXT` | virtual keyboard (`Ctrl+Shift+S`, `F9`, `Escape`; US layout) |
| `wait QUERY [--gone] [--timeout S]`, `wait-text TEXT`, `wait-status TEXT` | poll the dump for a frame newer than now |
| `state [--json]`, `log [--engine] [-n N]`, `hotkey ACTION` | inspect; deliver a hotkey through the app's own client |

Widget names come from the app's `FTHRUST_UI_DUMP` file (`$BENCH/ui.json`):
`uidrive state --json` lists them. Useful ones: `homeBtn`, `settingsGearBtn`,
`settingsTabBtn[General]` … `settingsTabBtn[Shortcuts]`, `settingsPage[<tab>]`,
`TopBarButton[Source]`, `TopBarButton[Hotkeys]`, `source_popup`,
`hotkey_popup`, `game_popup`, `capture_popup`, `QPushButton[Record]`,
`clipCard[0]`, `filterBar`, `errorBar`.

## Scenarios

`scenarios/selftest.sh`, `smoke.sh`, `save_clip.sh`, `record.sh`, `editor.sh`,
`export.sh`, `overlays.sh`, `upload.sh`, `clips_server.sh`, `docs_shots.sh`. Each prints
`PASS` or `FAIL <step>`, takes a screenshot on failure and tails the app log.
`clips_server.sh` needs `FTHR_CLIPS_SERVER` (a fthr-clips-server binary) and
prints `SKIP` without it.
`scenarios/lib.sh` has `step`, `run`, `fail`, `names`, `wait_file`, `scroll_to`.

`docs-shots.sh` regenerates `docs/images`: it renders a few demo clips with
ffmpeg (or takes `--seed DIR`), runs `docs_shots.sh` with `--pipewire`, and
copies the named shots over.

## Reading a failure

1. `cat $BENCH/logs/app.log` (the app; `RUST_LOG=info`), `$BENCH/logs/engine.log`.
2. Open the last screenshot in `$BENCH/shots/`.
3. `uidrive state --json` (while the bench is up) for the widgets on screen.

## CI

The GitHub runner has no GPU encoder, so the CI scenarios pass
`--settings tools/uidrive/ci-settings.json` (software encoder). The
fidelity tests need Noto Sans Mono (`fonts-noto-mono`), the monospace face
the reference screenshots were taken with.

## Running scenarios against the AppImage

`--app` must point at a `fthrust` with `fthrust-engine` beside it, so give it
the image's `usr/bin/fthrust`, extracted or mounted:

```sh
(cd /tmp/ai && dist/FTHRust-*-x86_64.AppImage --appimage-extract)   # or --appimage-mount
BENCH_DIR=/tmp/uidrive-ai tools/uidrive/bench.sh run tools/uidrive/scenarios/save_clip.sh \
    --app /tmp/ai/squashfs-root/usr/bin/fthrust --engine real
```

Keep `BENCH_DIR` short: the Wayland socket path inside it must fit in 108
bytes. The "DESKTOP AUDIO UNAVAILABLE" notice is expected in every bench run
without `--pipewire`, because the private `XDG_RUNTIME_DIR` has no PipeWire
socket.
