# Development

How to build FTHRust Clips, run its tests, and drive the real app from scripts.

- [Building](#building)
- [Running from a checkout](#running-from-a-checkout)
- [Tests](#tests)
- [UI fidelity tests](#ui-fidelity-tests)
- [The UI test bench](#the-ui-test-bench)
- [Checks and scripts](#checks-and-scripts)
- [CI](#ci)
- [Conventions](#conventions)

## Building

### Toolchain

`rust-toolchain.toml` pins **Rust 1.98.1** with rustfmt and clippy; `rustup` picks it up on its own. The workspace uses edition 2024.

### System packages

You need FFmpeg, PipeWire and Wayland development files, plus clang for bindgen.

**Ubuntu 24.04 / Debian**

```sh
sudo apt install pkg-config clang libclang-dev \
  libavcodec-dev libavformat-dev libavutil-dev libswscale-dev \
  libswresample-dev libavfilter-dev libavdevice-dev \
  libpipewire-0.3-dev libspa-0.2-dev \
  libwayland-dev libxkbcommon-dev
```

**Arch**

```sh
sudo pacman -S --needed base-devel clang ffmpeg pipewire wayland libxkbcommon
```

Your distribution's FFmpeg is fine for development. Release builds use a pinned FFmpeg instead; see [Packaging](packaging.md).

### Compiling

```sh
cargo build -p fthrust-ui -p fthrust-engine -p fthrust-uploader
```

That gives you three binaries in `target/debug/`: `fthrust` (the app), `fthrust-engine` and `fthrust-uploader`. Add `--release` for an optimised build. The uploader is optional; without it, uploads fail with a message saying it's missing.

## Running from a checkout

```sh
cargo run -p fthrust-ui
```

The app finds `fthrust-engine` and `fthrust-uploader` next to its own executable, so build them first, or point at others with `FTHRUST_ENGINE` and `FTHRUST_UPLOADER`.

**It uses your real `~/.fthr`.** A dev build shares settings, hotkeys and clips with an installed copy, and only one copy can run at a time. To keep them apart, run with a scratch home:

```sh
HOME=$(mktemp -d) cargo run -p fthrust-ui
```

Or use the [UI test bench](#the-ui-test-bench), which never touches your session at all.

Handy variables while developing:

| Variable | Effect |
|---|---|
| `RUST_LOG=debug` | App log detail (to stderr). |
| `FTHRUST_LOG=debug` | Engine log detail (to `~/.fthr/logs/engine.log`). |
| `FTHR_CAPTURE_BACKEND=ext\|wlr\|portal\|synthetic` | Forces a capture backend. `synthetic` draws a test pattern and needs no compositor support. |
| `FTHRUST_SYNTHETIC_BACKEND=1`, `FTHRUST_SYNTHETIC_SIZE=1920x1080` | Turns on the synthetic backend and sets its frame size. |
| `FTHRUST_UI_DUMP=path.json` | Writes every drawn widget's name, rectangle and text each frame. The bench reads this. |
| `FTHRUST_ASSETS=dir` | Loads fonts, icons and sounds from another folder. |

## Tests

```sh
cargo test --workspace
```

About 930 tests across the crates, as integration tests under `crates/*/tests/` plus unit tests. Some notes:

- `fthrust-ui` tests render real egui frames headlessly through wgpu. They need a Vulkan driver; CI uses Mesa's lavapipe with `WGPU_BACKEND=vulkan`.
- Font-measuring and fidelity tests need **Noto Sans Mono** (`fonts-noto-mono`), the monospace face the reference screenshots were taken with.
- `fthrust-media` tests encode real video with FFmpeg. Fixtures live in `crates/fthrust-media/tests/fixtures/`.
- Engine tests run the real engine against the synthetic backend, so no compositor is needed.

## UI fidelity tests

FTHRust's interface is a pixel-faithful port of the original Qt app, and that's tested rather than eyeballed.

Every custom widget registers a **marker** as it draws: its rectangle, colour samples (fill, edge, accent, text) and text runs, under the Qt object name it reproduces, for example `settingsTabBtn[General]` or `QPushButton[Record]`. `crates/fthrust-ui/src/fidelity.rs` collects them.

The reference side lives in `docs/reference/qt-screens/`: a PNG and a JSON per screen, generated from the original Qt app by `tools/render-qt-reference.py`. `tests/fidelity_all.rs` renders each screen in egui and compares it marker by marker against the Qt JSON with `assert_fidelity!`. Screens that aren't ported yet are `#[ignore]`d with a note.

Set `FTHRUST_FIDELITY_DUMP=<folder>` to write each screen's PNG and marker JSON there, for comparing by hand.

Regenerating the references needs the original app's source and a Python environment:

```sh
tools/fetch-reference.sh    # clones the original FTHR Clips at a pinned commit into reference/
tools/reference-venv.sh     # creates .venv-reference (PySide6 comes from your system Python)
```

## The UI test bench

`tools/uidrive` runs the real app inside a headless [cage](https://github.com/cage-kiosk/cage) compositor, with its own Wayland socket, home folder, runtime folder and D-Bus, and lets you drive it from the shell: screenshots, clicks, drags, typing, hotkeys, and waiting for things to appear. It never opens a window on your screen or reads your session.

You need `cage` and `grim`. Then:

```sh
cargo build -p fthrust-ui -p fthrust-engine
tools/uidrive/bench.sh run tools/uidrive/scenarios/save_clip.sh --engine real
```

Each scenario prints `PASS` or `FAIL <step>`, and leaves screenshots in `$BENCH/shots/` and logs in `$BENCH/logs/`.

| Scenario | Checks |
|---|---|
| `selftest.sh` | The bench itself: capture comes up and a click changes page. |
| `smoke.sh` | Every settings tab and toolbar popup opens and closes. |
| `save_clip.sh` | A hotkey saves a clip and the library shows it. |
| `record.sh` | A manual recording starts and stops. |
| `editor.sh` | Open, play, split, park, restore and close, by mouse and by keyboard. |
| `export.sh` | Edit, export through the queue, then share. |
| `overlays.sh` | Image overlays are burned into a saved clip. |
| `upload.sh` | Enable the uploader and upload to a local test server. |
| `clips_server.sh` | Upload to a real FTHR Clips Server (set `FTHR_CLIPS_SERVER` to its binary; skipped otherwise). |
| `docs_shots.sh` | Takes the screenshots in `docs/images` (run it through `tools/uidrive/docs-shots.sh`). |

Useful `bench.sh up` options:

| Option | Does |
|---|---|
| `--engine synthetic\|real` | A test pattern (the default), or real screen capture of the headless output. |
| `--release` | Uses the release build. |
| `--app PATH` | Runs another `fthrust`, such as one extracted from an AppImage. |
| `--settings FILE` | Merges a JSON file over the bench's settings. |
| `--pipewire` | Starts a private PipeWire and WirePlumber, so desktop audio comes up. |
| `--seed DIR` | Copies a clips folder into the bench before the app starts. |

Keep `BENCH_DIR` short: the Wayland socket path inside it must fit in 108 bytes. [`tools/uidrive/README.md`](../tools/uidrive/README.md) has the full command list and interactive use.

### Refreshing the docs screenshots

```sh
tools/uidrive/docs-shots.sh                 # renders demo clips, then shoots
tools/uidrive/docs-shots.sh --seed ~/clips  # or use a folder of real clips
```

This writes `docs/images/*.png`. It needs `ffmpeg`, `pipewire` and `wireplumber` on top of the bench's tools.

## Checks and scripts

| Script | Does |
|---|---|
| `tools/check-upload-isolation.sh` | Fails if `reqwest`, `russh`, `russh-sftp` or `keyring` end up in the app's or engine's dependency tree. Network and keyring code belongs in `fthrust-uploader` only. |
| `tools/check-assets.sh` | Checks bundled assets against their manifests (and against the original app's copies, if `reference/` exists). |
| `tools/check-version.sh` | The workspace version is the only version: every crate inherits it, a release tag must equal `v<version>`, and built binaries must report it. |
| `tools/gen-notices.sh [--check]` | Regenerates `THIRD_PARTY_NOTICES.md` with cargo-about, or checks it's current. |
| `tools/test-packaging.sh` | Tests the packaging scripts and gates. |
| `cargo deny check` | Licence allow-list, security advisories and crate sources, per `deny.toml`. |

`gen-notices.sh` needs `cargo install cargo-about --locked --features cli` at the version pinned in the script.

## CI

`.github/workflows/ci.yml` runs on every push and pull request, in two jobs:

- **check** (Ubuntu 24.04): `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, the full test suite, `cargo deny`, the isolation, asset, version and notices checks, and the packaging tests.
- **uidrive** (Ubuntu): builds release binaries and runs the `smoke`, `save_clip`, `export`, `overlays` and `upload` scenarios on the bench with a software renderer and a software encoder (there's no GPU on the runners). If one fails, the bench folder is uploaded as the `uidrive-bench` artifact.

`.github/workflows/release.yml` builds the AppImage; see [Packaging](packaging.md#releasing).

## Conventions

- **Formatting and lints:** `cargo fmt`, and clippy with warnings as errors.
- **One version:** bump `version` in the root `Cargo.toml`, and nowhere else.
- **Network code goes in the uploader.** If the app needs to talk to the internet, it goes through `fthrust-uploader`.
- **Shared formats live in `fthrust-core`.** Anything read from or written to `~/.fthr` stays byte-compatible with the original FTHR Clips files, and is tested against fixtures.
- **Commits** follow `type(scope): summary`, for example `feat(ui): …`, `fix(packaging): …`, `test(uidrive): …`.
